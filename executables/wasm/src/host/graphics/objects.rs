use super::canvas::{IndexedCanvas, MAX_CANVAS_PIXELS};
use super::{
    error::{GraphicsError, Result},
    marshal::{
        ButtonMap, GridTracks, MAX_WIDGET_OPTIONS_BYTES_PER_VM, updated_widget_options_usage,
    },
};
use crate::host::translation::WasmUsize;
use alloc::{boxed::Box, collections::BTreeMap, vec::Vec};
use core::{cell::UnsafeCell, num::NonZeroU32, ptr::NonNull};
#[expect(deprecated, reason = "the host still drives the raw LVGL bindings")]
use xila::graphics::lvgl;
use xila::graphics::{
    Event, Manager,
    object_id::{self, ObjectTable},
};

pub const MAX_GRID_TRACKS_PER_VM: usize = 512;

// Accessed only under Manager's process-wide graphics lock. Boxes keep these
// addresses stable when GraphicsState moves. The observer never calls LVGL.
struct Sides(UnsafeCell<Vec<*mut Side>>);
unsafe impl Sync for Sides {}
static SIDES: Sides = Sides(UnsafeCell::new(Vec::new()));

fn freed(object: NonNull<lvgl::lv_obj_t>, id: NonZeroU32) {
    let raw = id.get();
    // SAFETY: the graphics lock serializes Side registration, callbacks and drop.
    // The observer never calls LVGL and never mutates SIDES.
    let sides = unsafe { &*SIDES.0.get() };
    for &side_ptr in sides {
        // SAFETY: every pointer in SIDES points to a live, Box-allocated Side
        // that is unregistered before being freed.
        let side = unsafe { &mut *side_ptr };
        // Keyed by pointer: another VM may hold the same number for another
        // object.
        if side.table.remove(raw, object) {
            // The id hook runs at the END of the destructor, after LVGL has
            // finished reading maps, grid tracks and widget buffers.
            if let Some(entry) = side.entries.remove(&raw) {
                side.widget_options_bytes -= usize::from(entry.option_bytes);
                if let Some(resource) = entry.resource {
                    match &resource {
                        Resource::Grid(grid) => side.grid_tracks -= grid.track_count(),
                        Resource::Canvas(canvas) => side.canvas_pixels -= canvas.len(),
                        Resource::GridWith { grid, inner } => {
                            side.grid_tracks -= grid.track_count();
                            if let Resource::Canvas(canvas) = &**inner {
                                side.canvas_pixels -= canvas.len();
                            }
                        }
                        _ => {}
                    }
                }
            }
            break;
        }
    }
}

#[derive(Default)]
struct Side {
    /// This VM's guest-visible objects, by the ids it issued.
    table: ObjectTable,
    entries: BTreeMap<u32, Entry>,
    grid_tracks: usize,
    widget_options_bytes: usize,
    canvas_pixels: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Window,
    FilePicker,
    Object,
    Label,
    ButtonMatrix,
    Checkbox,
    TabView,
    IndexedCanvas,
    Textarea,
    Keyboard,
    Arc,
    Bar,
    Slider,
    Switch,
    Spinner,
    Dropdown,
    Roller,
    Led,
    Calendar,
    Spinbox,
    Chart,
    Line,
    List,
    ListText,
    ListButton,
    Tileview,
    Tile,
    Button,
    Badge,
    Card,
}

struct ChartSeries {
    series: [Option<NonNull<lvgl::lv_chart_series_t>>; 8],
    count: u8,
}

enum Resource {
    Map(#[allow(dead_code)] ButtonMap),
    Grid(GridTracks),
    Canvas(IndexedCanvas),
    Calendar(#[allow(dead_code)] Vec<lvgl::lv_calendar_date_t>),
    Line(#[allow(dead_code)] Vec<lvgl::lv_point_precise_t>),
    Chart(Box<ChartSeries>),
    GridWith {
        grid: GridTracks,
        inner: Box<Resource>,
    },
}

#[derive(Default)]
struct Entry {
    kind: Option<Kind>,
    tile_position: Option<(u8, u8)>,
    option_bytes: u16,
    resource: Option<Resource>,
}

impl Entry {
    fn grid(&self) -> Option<&GridTracks> {
        match self.resource.as_ref()? {
            Resource::Grid(grid) | Resource::GridWith { grid, .. } => Some(grid),
            _ => None,
        }
    }

    fn set_grid(&mut self, tracks: GridTracks) {
        match self.resource.take() {
            None | Some(Resource::Grid(_)) => {
                self.resource = Some(Resource::Grid(tracks));
            }
            Some(Resource::GridWith { inner, .. }) => {
                self.resource = Some(Resource::GridWith {
                    grid: tracks,
                    inner,
                });
            }
            Some(other) => {
                self.resource = Some(Resource::GridWith {
                    grid: tracks,
                    inner: Box::new(other),
                });
            }
        }
    }

    fn inner_resource(&self) -> Option<&Resource> {
        match self.resource.as_ref()? {
            Resource::Grid(_) => None,
            Resource::GridWith { inner, .. } => Some(inner),
            other => Some(other),
        }
    }

    fn inner_resource_mut(&mut self) -> Option<&mut Resource> {
        match self.resource.as_mut()? {
            Resource::Grid(_) => None,
            Resource::GridWith { inner, .. } => Some(inner),
            other => Some(other),
        }
    }

    fn set_inner_resource(&mut self, resource: Resource) {
        match self.resource.take() {
            Some(Resource::Grid(grid)) => {
                self.resource = Some(Resource::GridWith {
                    grid,
                    inner: Box::new(resource),
                });
            }
            Some(Resource::GridWith { grid, .. }) => {
                self.resource = Some(Resource::GridWith {
                    grid,
                    inner: Box::new(resource),
                });
            }
            _ => {
                self.resource = Some(resource);
            }
        }
    }

    fn take_inner_resource(&mut self) -> Option<Resource> {
        match self.resource.take() {
            None => None,
            Some(Resource::Grid(grid)) => {
                self.resource = Some(Resource::Grid(grid));
                None
            }
            Some(Resource::GridWith { grid, inner }) => {
                self.resource = Some(Resource::Grid(grid));
                Some(*inner)
            }
            Some(other) => Some(other),
        }
    }
}

pub struct GraphicsState {
    pub(crate) manager: Option<&'static Manager>,
    side: Option<Box<UnsafeCell<Side>>>,
    /// Trusted native context, refreshed from GlobalStore at the VM boundary.
    pub(crate) file_system: Option<(
        &'static xila::virtual_file_system::VirtualFileSystem,
        xila::task::TaskIdentifier,
    )>,
}

impl GraphicsState {
    pub fn new(manager: Option<&'static Manager>) -> Self {
        Self {
            manager,
            side: None,
            file_system: None,
        }
    }

    /// Supply the caller identity when using the VM-independent dispatcher.
    /// The Wasmi adapter always supplies its own trusted store context.
    pub fn with_file_system(
        manager: Option<&'static Manager>,
        vfs: &'static xila::virtual_file_system::VirtualFileSystem,
        task: xila::task::TaskIdentifier,
    ) -> Self {
        let mut state = Self::new(manager);
        state.file_system = Some((vfs, task));
        state
    }

    pub(crate) fn initialize(&mut self) -> Result<()> {
        if self.side.is_some() {
            return Ok(());
        }
        let side = Box::new(UnsafeCell::new(Side::default()));
        // SAFETY: called only inside the manager session under the graphics lock.
        let sides = unsafe { &mut *SIDES.0.get() };
        if sides.try_reserve(1).is_err() {
            return Err(GraphicsError::ResourceLimit);
        }
        object_id::set_free_observer(freed).map_err(|_| GraphicsError::ResourceLimit)?;
        sides.push(side.get());
        self.side = Some(side);
        Ok(())
    }

    pub(crate) fn ensure_capacity(&self) -> Result<()> {
        self.side
            .as_ref()
            .map(|_| ())
            .ok_or(GraphicsError::ResourceLimit)
    }

    fn side(&self) -> &Side {
        // SAFETY: dispatcher holds the graphics lock; no borrow spans deletion.
        unsafe { &*self.side.as_ref().expect("initialized graphics").get() }
    }

    fn side_mut(&mut self) -> &mut Side {
        // SAFETY: as above. UnsafeCell permits the free observer to access the
        // stable allocation when no host resource borrow is outstanding.
        unsafe { &mut *self.side.as_ref().expect("initialized graphics").get() }
    }

    #[allow(clippy::useless_conversion)]
    fn raw(handle: WasmUsize) -> Result<u32> {
        u32::try_from(handle).map_err(|_| GraphicsError::InvalidHandle)
    }

    fn resolve_raw(&self, handle: WasmUsize) -> Result<(u32, *mut lvgl::lv_obj_t)> {
        let side = self.side.as_ref().ok_or(GraphicsError::InvalidHandle)?;
        let raw = Self::raw(handle)?;
        // A guest value is never dereferenced: it either names an object this
        // VM published and that is still alive, or it is not found.
        // SAFETY: dispatcher holds the graphics lock; no borrow spans deletion.
        let object = unsafe { &*side.get() }
            .table
            .find(raw)
            .ok_or(GraphicsError::InvalidHandle)?;
        Ok((raw, object.as_ptr()))
    }

    fn kind_of(&self, raw: u32, pointer: *mut lvgl::lv_obj_t) -> Result<Kind> {
        if let Some(kind) = self.side().entries.get(&raw).and_then(|e| e.kind) {
            return Ok(kind);
        }
        // Exact classes preserve distinctions such as Slider versus Bar and
        // Keyboard versus ButtonMatrix. Alias kinds live only in the side table.
        let class = unsafe { lvgl::lv_obj_get_class(pointer) };
        if core::ptr::eq(class, xila::graphics::get_window_class()) {
            return Ok(Kind::Window);
        }
        if core::ptr::eq(class, xila::graphics::get_file_picker_class()) {
            return Ok(Kind::FilePicker);
        }
        macro_rules! classes {
            ($($kind:ident => $class:ident),* $(,)?) => { $(
                if core::ptr::eq(class, &raw const lvgl::$class) { return Ok(Kind::$kind); }
            )* };
        }
        classes! {
            Object => lv_obj_class, Label => lv_label_class, Button => lv_button_class,
            ButtonMatrix => lv_buttonmatrix_class, Checkbox => lv_checkbox_class,
            TabView => lv_tabview_class, Textarea => lv_textarea_class, Keyboard => lv_keyboard_class,
            Arc => lv_arc_class, Bar => lv_bar_class, Slider => lv_slider_class,
            Switch => lv_switch_class, Spinner => lv_spinner_class, Dropdown => lv_dropdown_class,
            Roller => lv_roller_class, Led => lv_led_class, Calendar => lv_calendar_class,
            Spinbox => lv_spinbox_class, Chart => lv_chart_class, Line => lv_line_class,
            List => lv_list_class, Tileview => lv_tileview_class,
        }
        Err(GraphicsError::InvalidHandle)
    }

    pub(crate) fn object(
        &self,
        handle: WasmUsize,
        expected: Option<Kind>,
    ) -> Result<*mut lvgl::lv_obj_t> {
        let (raw, pointer) = self.resolve_raw(handle)?;
        if let Some(expected) = expected
            && self.kind_of(raw, pointer)? != expected
        {
            return Err(GraphicsError::InvalidHandle);
        }
        Ok(pointer)
    }

    pub(crate) fn object_with_kind(
        &self,
        handle: WasmUsize,
    ) -> Result<(*mut lvgl::lv_obj_t, Kind)> {
        let (raw, pointer) = self.resolve_raw(handle)?;
        Ok((pointer, self.kind_of(raw, pointer)?))
    }

    pub(crate) fn is_child_of(
        &self,
        child: WasmUsize,
        parent: WasmUsize,
        kind: Kind,
    ) -> Result<bool> {
        let child_pointer = self.object(child, Some(kind))?;
        let parent_pointer = self.object(parent, None)?;
        Ok(unsafe { lvgl::lv_obj_get_parent(child_pointer) } == parent_pointer)
    }

    pub(crate) fn tile_position_in_use(
        &self,
        view: WasmUsize,
        column: u8,
        row: u8,
    ) -> Result<bool> {
        let view_pointer = self.object(view, Some(Kind::Tileview))?;
        // Walk the view's own children and read each child's id once, instead
        // of resolving every side entry against the tree.
        // SAFETY: the view is live under the graphics lock; children are live.
        let count = unsafe { lvgl::lv_obj_get_child_count(view_pointer) };
        for index in 0..count {
            let child = unsafe { lvgl::lv_obj_get_child(view_pointer, index as _) };
            let Some(child) = NonNull::new(child) else {
                continue;
            };
            let Some(id) = (unsafe { self.side().table.id_of(child) }) else {
                continue;
            };
            if self.side().entries.get(&id.get()).is_some_and(|entry| {
                entry.kind == Some(Kind::Tile) && entry.tile_position == Some((column, row))
            }) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub(crate) fn set_tile_position(
        &mut self,
        handle: WasmUsize,
        position: (u8, u8),
    ) -> Result<()> {
        let raw = self.resolve_raw(handle)?.0;
        self.side_mut()
            .entries
            .entry(raw)
            .or_default()
            .tile_position = Some(position);
        Ok(())
    }

    /// The guest handle of an event's target: the id it carried when the event
    /// was queued, if it still names one of this VM's live objects. Native
    /// children carry no id, and an id never names a later object at a reused
    /// address.
    pub(crate) fn event_handle(&self, event: &Event) -> WasmUsize {
        event
            .target_id()
            .filter(|id| self.side.is_some() && self.side().table.find(id.get()).is_some())
            .map_or(0, |id| id.get() as WasmUsize)
    }

    pub(crate) fn handle_for(&self, pointer: *mut lvgl::lv_obj_t) -> WasmUsize {
        let Some(non_null) = NonNull::new(pointer) else {
            return 0;
        };
        if self.side.is_none() {
            return 0;
        }
        // SAFETY: dispatcher holds the graphics lock; `pointer` is live.
        unsafe { self.side().table.id_of(non_null) }.map_or(0, |id| id.get() as WasmUsize)
    }

    pub(crate) fn insert(
        &mut self,
        pointer: *mut lvgl::lv_obj_t,
        kind: Kind,
        _parent: Option<WasmUsize>,
    ) -> Result<WasmUsize> {
        let pointer = NonNull::new(pointer).ok_or(GraphicsError::NativeFailure)?;
        if self.side.is_none() {
            unsafe { lvgl::lv_obj_delete(pointer.as_ptr()) };
            return Err(GraphicsError::ResourceLimit);
        }
        // A fresh, never reissued id: events queued for an earlier object at
        // this address carry another id (or none) and cannot name this one.
        // SAFETY: the object was just created under the graphics lock.
        let id = match unsafe { self.side_mut().table.publish(pointer) } {
            Ok(id) => id.get(),
            Err(_) => {
                // Out of memory or ids: only this VM's creations fail.
                unsafe { lvgl::lv_obj_delete(pointer.as_ptr()) };
                return Err(GraphicsError::ResourceLimit);
            }
        };
        if matches!(
            kind,
            Kind::IndexedCanvas
                | Kind::ListText
                | Kind::ListButton
                | Kind::Tile
                | Kind::Badge
                | Kind::Card
        ) {
            self.side_mut().entries.entry(id).or_default().kind = Some(kind);
        }
        Ok(id as WasmUsize)
    }

    pub(crate) fn set_map(&mut self, handle: WasmUsize, map: ButtonMap) {
        let (raw, pointer) = self.resolve_raw(handle).expect("validated matrix");
        unsafe { lvgl::lv_buttonmatrix_set_map(pointer, map.as_ptr()) };
        self.side_mut()
            .entries
            .entry(raw)
            .or_default()
            .set_inner_resource(Resource::Map(map));
    }

    pub(crate) fn set_grid_tracks(&mut self, handle: WasmUsize, tracks: GridTracks) -> Result<()> {
        let (raw, pointer) = self.resolve_raw(handle)?;
        let previous = self
            .side()
            .entries
            .get(&raw)
            .and_then(Entry::grid)
            .map_or(0, GridTracks::track_count);
        let used = self.side().grid_tracks - previous + tracks.track_count();
        if used > MAX_GRID_TRACKS_PER_VM {
            return Err(GraphicsError::ResourceLimit);
        }
        unsafe {
            lvgl::lv_obj_set_grid_dsc_array(pointer, tracks.columns_ptr(), tracks.rows_ptr())
        };
        self.side_mut()
            .entries
            .entry(raw)
            .or_default()
            .set_grid(tracks);
        self.side_mut().grid_tracks = used;
        Ok(())
    }

    pub(crate) fn validate_widget_options_size(
        &self,
        handle: WasmUsize,
        new_bytes: usize,
    ) -> Result<()> {
        if new_bytes > MAX_WIDGET_OPTIONS_BYTES_PER_VM {
            return Err(GraphicsError::ResourceLimit);
        }
        let raw = self.resolve_raw(handle)?.0;
        let previous = self
            .side()
            .entries
            .get(&raw)
            .map_or(0, |entry| usize::from(entry.option_bytes));
        updated_widget_options_usage(self.side().widget_options_bytes, previous, new_bytes)?;
        Ok(())
    }

    pub(crate) fn validate_new_widget_options_size(&self, new_bytes: usize) -> Result<()> {
        if new_bytes > MAX_WIDGET_OPTIONS_BYTES_PER_VM {
            return Err(GraphicsError::ResourceLimit);
        }
        updated_widget_options_usage(self.side().widget_options_bytes, 0, new_bytes)?;
        Ok(())
    }

    pub(crate) fn commit_widget_options_size(
        &mut self,
        handle: WasmUsize,
        new_bytes: usize,
    ) -> Result<()> {
        if new_bytes > MAX_WIDGET_OPTIONS_BYTES_PER_VM {
            return Err(GraphicsError::ResourceLimit);
        }
        let raw = self.resolve_raw(handle)?.0;
        let previous = self
            .side()
            .entries
            .get(&raw)
            .map_or(0, |entry| usize::from(entry.option_bytes));
        let charge = previous.max(new_bytes);
        let used =
            updated_widget_options_usage(self.side().widget_options_bytes, previous, new_bytes)?;
        self.side_mut().entries.entry(raw).or_default().option_bytes = charge as u16;
        self.side_mut().widget_options_bytes = used;
        Ok(())
    }

    pub(crate) fn replace_calendar_highlights(
        &mut self,
        handle: WasmUsize,
        mut dates: Vec<lvgl::lv_calendar_date_t>,
    ) -> Result<(
        *mut lvgl::lv_calendar_date_t,
        Option<Vec<lvgl::lv_calendar_date_t>>,
    )> {
        let new_bytes = dates
            .len()
            .checked_mul(core::mem::size_of::<lvgl::lv_calendar_date_t>())
            .ok_or(GraphicsError::ResourceLimit)?;
        if new_bytes > MAX_WIDGET_OPTIONS_BYTES_PER_VM {
            return Err(GraphicsError::ResourceLimit);
        }
        let (raw, pointer) = self.resolve_raw(handle)?;
        if self.kind_of(raw, pointer)? != Kind::Calendar {
            return Err(GraphicsError::InvalidHandle);
        }
        let previous = self
            .side()
            .entries
            .get(&raw)
            .map_or(0, |entry| usize::from(entry.option_bytes));
        let charge =
            u16::try_from(previous.max(new_bytes)).map_err(|_| GraphicsError::ResourceLimit)?;
        let used =
            updated_widget_options_usage(self.side().widget_options_bytes, previous, new_bytes)?;
        let dates_pointer = if dates.is_empty() {
            core::ptr::null_mut()
        } else {
            dates.as_mut_ptr()
        };
        let side = self.side_mut();
        let entry = side.entries.entry(raw).or_default();
        let old = if dates.is_empty() {
            match entry.take_inner_resource() {
                Some(Resource::Calendar(old)) => Some(old),
                Some(other) => {
                    entry.set_inner_resource(other);
                    None
                }
                None => None,
            }
        } else {
            match entry.inner_resource_mut() {
                Some(Resource::Calendar(existing)) => Some(core::mem::replace(existing, dates)),
                _ => {
                    entry.set_inner_resource(Resource::Calendar(dates));
                    None
                }
            }
        };
        entry.option_bytes = charge;
        side.widget_options_bytes = used;
        Ok((dates_pointer, old))
    }

    pub(crate) fn chart_series_count(&self, handle: WasmUsize) -> Result<usize> {
        let (raw, pointer) = self.resolve_raw(handle)?;
        if self.kind_of(raw, pointer)? != Kind::Chart {
            return Err(GraphicsError::InvalidHandle);
        }
        match self
            .side()
            .entries
            .get(&raw)
            .and_then(Entry::inner_resource)
        {
            None => Ok(0),
            Some(Resource::Chart(chart)) => Ok(usize::from(chart.count)),
            Some(_) => Err(GraphicsError::InvalidHandle),
        }
    }

    pub(crate) fn add_chart_series(
        &mut self,
        handle: WasmUsize,
        series_pointer: NonNull<lvgl::lv_chart_series_t>,
    ) -> Result<u32> {
        let (raw, pointer) = self.resolve_raw(handle)?;
        if self.kind_of(raw, pointer)? != Kind::Chart {
            return Err(GraphicsError::InvalidHandle);
        }
        let entry = self.side_mut().entries.entry(raw).or_default();
        if entry.inner_resource().is_none() {
            entry.set_inner_resource(Resource::Chart(Box::new(ChartSeries {
                series: [None; 8],
                count: 0,
            })));
        }
        let Some(Resource::Chart(chart)) = entry.inner_resource_mut() else {
            return Err(GraphicsError::InvalidHandle);
        };
        let slot = usize::from(chart.count);
        if slot >= chart.series.len() {
            return Err(GraphicsError::ResourceLimit);
        }
        chart.series[slot] = Some(series_pointer);
        chart.count += 1;
        u32::try_from(slot).map_err(|_| GraphicsError::ResourceLimit)
    }

    pub(crate) fn chart_series(
        &self,
        handle: WasmUsize,
        slot: WasmUsize,
    ) -> Result<NonNull<lvgl::lv_chart_series_t>> {
        let (raw, pointer) = self.resolve_raw(handle)?;
        if self.kind_of(raw, pointer)? != Kind::Chart {
            return Err(GraphicsError::InvalidHandle);
        }
        let index = usize::try_from(slot).map_err(|_| GraphicsError::InvalidHandle)?;
        match self
            .side()
            .entries
            .get(&raw)
            .and_then(Entry::inner_resource)
        {
            Some(Resource::Chart(chart)) => chart
                .series
                .get(index)
                .copied()
                .flatten()
                .ok_or(GraphicsError::InvalidHandle),
            _ => Err(GraphicsError::InvalidHandle),
        }
    }

    pub(crate) fn replace_line_points(
        &mut self,
        handle: WasmUsize,
        points: Vec<lvgl::lv_point_precise_t>,
    ) -> Result<(
        *const lvgl::lv_point_precise_t,
        Option<Vec<lvgl::lv_point_precise_t>>,
    )> {
        let new_bytes = points
            .len()
            .checked_mul(core::mem::size_of::<lvgl::lv_point_precise_t>())
            .ok_or(GraphicsError::ResourceLimit)?;
        if new_bytes > MAX_WIDGET_OPTIONS_BYTES_PER_VM {
            return Err(GraphicsError::ResourceLimit);
        }
        let (raw, pointer) = self.resolve_raw(handle)?;
        if self.kind_of(raw, pointer)? != Kind::Line {
            return Err(GraphicsError::InvalidHandle);
        }
        let previous = self
            .side()
            .entries
            .get(&raw)
            .map_or(0, |entry| usize::from(entry.option_bytes));
        let charge =
            u16::try_from(previous.max(new_bytes)).map_err(|_| GraphicsError::ResourceLimit)?;
        let used =
            updated_widget_options_usage(self.side().widget_options_bytes, previous, new_bytes)?;
        let points_pointer = if points.is_empty() {
            core::ptr::null()
        } else {
            points.as_ptr()
        };
        let side = self.side_mut();
        let entry = side.entries.entry(raw).or_default();
        let old = if points.is_empty() {
            match entry.take_inner_resource() {
                Some(Resource::Line(old)) => Some(old),
                Some(other) => {
                    entry.set_inner_resource(other);
                    None
                }
                None => None,
            }
        } else {
            match entry.inner_resource_mut() {
                Some(Resource::Line(existing)) => Some(core::mem::replace(existing, points)),
                _ => {
                    entry.set_inner_resource(Resource::Line(points));
                    None
                }
            }
        };
        entry.option_bytes = charge;
        side.widget_options_bytes = used;
        Ok((points_pointer, old))
    }

    pub(crate) fn validate_canvas_pixels(&self, width: usize, height: usize) -> Result<()> {
        let length = width
            .checked_mul(height)
            .ok_or(GraphicsError::InvalidValue)?;
        if width == 0 || height == 0 || length > MAX_CANVAS_PIXELS {
            return Err(GraphicsError::InvalidValue);
        }
        if self.side().canvas_pixels + length > MAX_CANVAS_PIXELS {
            return Err(GraphicsError::ResourceLimit);
        }
        Ok(())
    }

    pub(crate) fn install_canvas(
        &mut self,
        handle: WasmUsize,
        mut canvas: IndexedCanvas,
    ) -> Result<()> {
        let length = canvas.len();
        if self.side().canvas_pixels + length > MAX_CANVAS_PIXELS {
            return Err(GraphicsError::ResourceLimit);
        }
        let (raw, pointer) = self.resolve_raw(handle)?;
        if self.kind_of(raw, pointer)? != Kind::IndexedCanvas {
            return Err(GraphicsError::InvalidHandle);
        }
        unsafe {
            canvas.attach(pointer)?;
        }
        let side = self.side_mut();
        side.entries
            .entry(raw)
            .or_default()
            .set_inner_resource(Resource::Canvas(canvas));
        side.canvas_pixels += length;
        Ok(())
    }

    pub(crate) fn canvas_mut(&mut self, handle: WasmUsize) -> &mut IndexedCanvas {
        let raw = self.resolve_raw(handle).expect("live canvas").0;
        let entry = self
            .side_mut()
            .entries
            .get_mut(&raw)
            .expect("canvas entry present");
        match entry.inner_resource_mut() {
            Some(Resource::Canvas(canvas)) => canvas,
            _ => panic!("canvas kind checked"),
        }
    }

    #[doc(hidden)]
    pub fn handle_count(&self) -> usize {
        let _lock = self.manager.map(Manager::synchronous_lock);
        self.side
            .as_ref()
            // SAFETY: graphics lock held when manager is present.
            .map_or(0, |side| unsafe { &*side.get() }.table.len())
    }

    #[doc(hidden)]
    pub fn side_entry_count(&self) -> usize {
        let _lock = self.manager.map(Manager::synchronous_lock);
        self.side
            .as_ref()
            // SAFETY: graphics lock held when manager is present.
            .map_or(0, |side| unsafe { &*side.get() }.entries.len())
    }

    #[doc(hidden)]
    pub fn resource_counters(&self) -> (usize, usize, usize) {
        let _lock = self.manager.map(Manager::synchronous_lock);
        self.side.as_ref().map_or((0, 0, 0), |side| {
            // SAFETY: graphics lock held when manager is present.
            let side = unsafe { &*side.get() };
            (
                side.grid_tracks,
                side.widget_options_bytes,
                side.canvas_pixels,
            )
        })
    }

    #[doc(hidden)]
    pub fn registered_state_count(manager: &Manager) -> usize {
        let _lock = manager.synchronous_lock();
        // SAFETY: graphics lock held.
        unsafe { &*SIDES.0.get() }.len()
    }
}

fn unregister_side(side: Option<Box<UnsafeCell<Side>>>) {
    let Some(side) = side else {
        return;
    };
    let target = side.get();
    // SAFETY: called only while holding the graphics lock.
    let sides = unsafe { &mut *SIDES.0.get() };
    if let Some(pos) = sides.iter().position(|&ptr| ptr == target) {
        sides.swap_remove(pos);
        if sides.is_empty() {
            *sides = Vec::new();
        }
    }
    drop(side);
}

impl Drop for GraphicsState {
    fn drop(&mut self) {
        let Some(manager) = self.manager else {
            return;
        };
        if self.side.is_none() {
            return;
        }
        let _ = manager.with_lvgl_sync(|_| {
            // Delete the VM's objects one at a time, re-reading the table after
            // each: deleting an object frees its descendants, whose entries
            // the free hook removes, so no stale pointer is ever visited.
            while let Some(object) = self.side().table.first() {
                let before = self.side().table.len();
                // SAFETY: the table holds only live objects, under the lock.
                unsafe { lvgl::lv_obj_delete(object.as_ptr()) };
                if self.side().table.len() == before {
                    break; // the free hook did not run: never loop forever
                }
            }
            // Private window roots ensure structural deletion cannot leave a
            // retained resource attached to an untracked shared object.
            unregister_side(self.side.take());
            Ok(())
        });
        if let Some(side) = self.side.take() {
            // No session (re-entrant drop or LVGL gone): the VM's objects may
            // still be alive and LVGL may still read their maps, grid tracks or
            // canvas buffers when it deletes them. Stop observing, and leak the
            // side table rather than free memory LVGL can still reach.
            let _lock = manager.synchronous_lock();
            let target = side.get();
            // SAFETY: graphics lock held.
            let sides = unsafe { &mut *SIDES.0.get() };
            if let Some(position) = sides.iter().position(|&pointer| pointer == target) {
                sides.swap_remove(position);
            }
            Box::leak(side);
        }
    }
}

#[cfg(target_pointer_width = "32")]
const _: () = {
    assert!(core::mem::size_of::<u32>() == 4);
    assert!(core::mem::size_of::<Entry>() == 24);
    assert!(core::mem::size_of::<Side>() == 40);
    assert!(core::mem::size_of::<Sides>() == 12);
    assert!(core::mem::size_of::<GraphicsState>() == 16);
    assert!(core::mem::size_of::<ChartSeries>() == 36);
};

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::size_of;

    #[test]
    fn host_type_sizes_match_expected_layouts() {
        assert_eq!(size_of::<u32>(), 4);
        assert_eq!(size_of::<lvgl::lv_calendar_date_t>(), 4);
        assert_eq!(size_of::<lvgl::lv_point_precise_t>(), 8);
        assert_eq!(size_of::<xila::task::TaskIdentifier>(), 4);
        #[cfg(target_pointer_width = "64")]
        {
            assert_eq!(size_of::<Entry>(), 40);
            assert_eq!(size_of::<Side>(), 80);
            assert_eq!(size_of::<Sides>(), 24);
            assert_eq!(size_of::<GraphicsState>(), 32);
        }
    }
}

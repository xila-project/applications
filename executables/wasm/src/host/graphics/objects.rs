use super::canvas::{IndexedCanvas, MAX_CANVAS_PIXELS};
use super::{
    error::{GraphicsError, Result},
    marshal::{
        ButtonMap, GridTracks, MAX_WIDGET_OPTIONS_BYTES_PER_VM, updated_widget_options_usage,
    },
};
use crate::host::translation::WasmUsize;
use alloc::{boxed::Box, collections::BTreeMap, vec::Vec};
use core::{cell::UnsafeCell, ptr::NonNull};
use xila::graphics::{
    Manager, Window, lvgl,
    lvgl_rust::{self as safe, WidgetExt},
};

pub const MAX_GRID_TRACKS_PER_VM: usize = 512;

// Accessed only under Manager's process-wide graphics lock. Boxes keep these
// addresses stable when GraphicsState moves. The observer never calls LVGL.
struct Owners(UnsafeCell<[*mut Side; safe::Owner::MAX as usize + 1]>);
unsafe impl Sync for Owners {}
static OWNERS: Owners = Owners(UnsafeCell::new(
    [core::ptr::null_mut(); safe::Owner::MAX as usize + 1],
));

fn freed(event: safe::Freed) {
    let Some(owner) = event.owner() else { return };
    // SAFETY: the graphics lock serializes owner registration, callbacks and drop.
    let side = unsafe { (*OWNERS.0.get())[owner.get() as usize].as_mut() };
    if let Some(side) = side {
        // The OBJ_ID hook runs at the END of the destructor, after LVGL has
        // finished reading maps, grid tracks and widget buffers.
        if let Some(entry) = side.entries.remove(&event.handle().into_raw()) {
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
    }
}

#[derive(Default)]
struct Side {
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
    owner: Option<safe::Owner>,
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
            owner: None,
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

    pub(crate) fn initialize(&mut self, lv: &mut safe::Lvgl) -> Result<()> {
        if self.owner.is_some() {
            return Ok(());
        }
        // SAFETY: called only inside the manager session.
        let owners = unsafe { &mut *OWNERS.0.get() };
        let index = (1..owners.len())
            .find(|&i| owners[i].is_null())
            .ok_or(GraphicsError::ResourceLimit)?;
        let side = Box::new(UnsafeCell::new(Side::default()));
        owners[index] = side.get();
        self.owner = safe::Owner::new(index as u8);
        self.side = Some(side);
        lv.set_free_observer(Some(freed));
        Ok(())
    }

    pub(crate) fn ensure_capacity(&self) -> Result<()> {
        self.owner.map(|_| ()).ok_or(GraphicsError::ResourceLimit)
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

    fn lv(&self) -> &safe::Lvgl {
        // SAFETY: all dispatcher helpers run inside the manager's session.
        unsafe { safe::Lvgl::assume() }
    }

    #[allow(clippy::useless_conversion)]
    fn raw(handle: WasmUsize) -> Result<u32> {
        u32::try_from(handle).map_err(|_| GraphicsError::InvalidHandle)
    }

    fn resolve_raw(&self, handle: WasmUsize) -> Result<(u32, *mut lvgl::lv_obj_t)> {
        let owner = self.owner.ok_or(GraphicsError::InvalidHandle)?;
        let raw = Self::raw(handle)?;
        let object = self
            .lv()
            .resolve_for::<safe::Obj>(raw, owner)
            .map_err(|_| GraphicsError::InvalidHandle)?;
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
        Ok(self.side().entries.iter().any(|(&raw, entry)| {
            entry.kind == Some(Kind::Tile)
                && entry.tile_position == Some((column, row))
                && self
                    .object(raw as WasmUsize, None)
                    .is_ok_and(|p| unsafe { lvgl::lv_obj_get_parent(p) } == view_pointer)
        }))
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

    pub(crate) fn event_handle_for(
        &self,
        root: *mut lvgl::lv_obj_t,
        target: *mut lvgl::lv_obj_t,
    ) -> WasmUsize {
        // Native children without handles can also appear in events and can
        // die without notifying our observer. Validate by address comparison
        // in the live tree, never by dereferencing a queued target pointer.
        struct Search {
            target: *mut lvgl::lv_obj_t,
            found: bool,
        }
        unsafe extern "C" fn visit(
            obj: *mut lvgl::lv_obj_t,
            data: *mut core::ffi::c_void,
        ) -> lvgl::lv_obj_tree_walk_res_t {
            let search = unsafe { &mut *data.cast::<Search>() };
            if obj == search.target {
                search.found = true;
                lvgl::lv_obj_tree_walk_res_t_LV_OBJ_TREE_WALK_END
            } else {
                lvgl::lv_obj_tree_walk_res_t_LV_OBJ_TREE_WALK_NEXT
            }
        }
        if target.is_null() {
            return 0;
        }
        let mut search = Search {
            target,
            found: false,
        };
        unsafe { lvgl::lv_obj_tree_walk(root, Some(visit), (&mut search as *mut Search).cast()) };
        if search.found {
            self.handle_for(target)
        } else {
            0
        }
    }

    pub(crate) fn handle_for(&self, pointer: *mut lvgl::lv_obj_t) -> WasmUsize {
        let Some(non_null) = NonNull::new(pointer) else {
            return 0;
        };
        let obj = unsafe { safe::Obj::from_raw(self.lv(), non_null) };
        obj.find_handle()
            .filter(|h| h.owner() == self.owner)
            .map_or(0, |h| h.into_raw() as WasmUsize)
    }

    pub(crate) fn insert(
        &mut self,
        pointer: *mut lvgl::lv_obj_t,
        kind: Kind,
        _parent: Option<WasmUsize>,
    ) -> Result<WasmUsize> {
        let pointer = NonNull::new(pointer).ok_or(GraphicsError::NativeFailure)?;
        // Clear pre-registration event targets at the capability birth boundary,
        // including a recycled native address. Shared screens are never tagged.
        let mut root = pointer.as_ptr();
        while !root.is_null() {
            if let Some(mut window) = unsafe { Window::from_raw(root) } {
                unsafe { window.as_mut().clear_event_targets_for(pointer.as_ptr()) };
                break;
            }
            root = unsafe { lvgl::lv_obj_get_parent(root) };
        }
        let obj = unsafe { safe::Obj::from_raw(self.lv(), pointer) };
        let owner = self.owner.ok_or(GraphicsError::ResourceLimit)?;
        let handle = match obj.handle_for(owner) {
            Ok(h) => h.into_raw(),
            Err(_) => {
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
            self.side_mut().entries.entry(handle).or_default().kind = Some(kind);
        }
        Ok(handle as WasmUsize)
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
}

impl Drop for GraphicsState {
    fn drop(&mut self) {
        let (Some(manager), Some(owner)) = (self.manager, self.owner) else {
            return;
        };
        let _ = manager.with_lvgl_sync(|lv| {
            lv.delete_owner(owner);
            // Private window roots ensure structural deletion cannot leave a
            // retained resource attached to an untracked shared object.
            unsafe {
                (*OWNERS.0.get())[owner.get() as usize] = core::ptr::null_mut();
            }
            Ok(())
        });
    }
}

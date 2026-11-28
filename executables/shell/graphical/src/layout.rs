use crate::error::{Error, Result};
use alloc::string::String;
use core::ffi::CStr;
use core::time::Duration;
use xila::file_system::{AccessFlags, Path};
use xila::graphics::{self, lvgl, symbol, theme};
use xila::local_time::{self, TimeZone};
use xila::log;
use xila::network::InterfaceKind;
use xila::virtual_file_system::{Directory, File};
use xila::{internationalization, network};

// Only the local time of day is displayed. Bound calendar conversion
// independently of the signed timestamp range, including far-future and
// pre-epoch corrections: the offset is added to the UTC second of the day.
fn write_clock_text(output: &mut String, current_time: Option<i64>, zone: &TimeZone) {
    output.clear();
    if let Some(seconds) = current_time {
        let offset = i64::from(zone.offset_at(seconds));
        let local = (seconds.rem_euclid(86_400) + offset).rem_euclid(86_400);
        let _ = internationalization::DateTimeFormatter::new(
            internationalization::date_time_locale(internationalization::locale!()),
        )
        .write(output, local, "%H:%M");
    } else {
        output.push_str("--:--");
    }
}

#[derive(Clone, Copy)]
struct SavedLocalStyle {
    prop: lvgl::lv_style_prop_t,
    value: Option<lvgl::lv_style_value_t>,
}

impl SavedLocalStyle {
    unsafe fn capture(object: *mut lvgl::lv_obj_t, prop: lvgl::lv_style_prop_t) -> Self {
        let mut value = core::mem::MaybeUninit::uninit();
        let found = unsafe {
            lvgl::lv_obj_get_local_style_prop(
                object,
                prop,
                value.as_mut_ptr(),
                lvgl::LV_STATE_DEFAULT,
            )
        };
        let value = (found == lvgl::lv_style_res_t_LV_STYLE_RES_FOUND)
            .then(|| unsafe { value.assume_init() });
        Self { prop, value }
    }

    unsafe fn restore(&self, object: *mut lvgl::lv_obj_t) {
        unsafe {
            match self.value {
                Some(value) => lvgl::lv_obj_set_local_style_prop(
                    object,
                    self.prop,
                    value,
                    lvgl::LV_STATE_DEFAULT,
                ),
                None => {
                    lvgl::lv_obj_remove_local_style_prop(object, self.prop, lvgl::LV_STATE_DEFAULT);
                }
            }
        }
    }
}

#[derive(Clone, Copy)]
struct ScreenStyleSnapshot([SavedLocalStyle; 4]);

impl ScreenStyleSnapshot {
    const PROPERTIES: [lvgl::lv_style_prop_t; 4] = [
        lvgl::_lv_style_id_t_LV_STYLE_WIDTH as lvgl::lv_style_prop_t,
        lvgl::_lv_style_id_t_LV_STYLE_HEIGHT as lvgl::lv_style_prop_t,
        lvgl::_lv_style_id_t_LV_STYLE_FLEX_FLOW as lvgl::lv_style_prop_t,
        lvgl::_lv_style_id_t_LV_STYLE_LAYOUT as lvgl::lv_style_prop_t,
    ];

    unsafe fn capture(screen: *mut lvgl::lv_obj_t) -> Self {
        Self(Self::PROPERTIES.map(|prop| unsafe { SavedLocalStyle::capture(screen, prop) }))
    }

    unsafe fn restore(&self, screen: *mut lvgl::lv_obj_t) {
        for saved in &self.0 {
            unsafe { saved.restore(screen) };
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LayoutWidget {
    Header,
    Clock,
    Tray,
    Network,
    Battery,
    Body,
}

impl LayoutWidget {
    /// # Safety
    /// The caller must hold the graphics lock and pass a valid LVGL parent.
    pub(crate) unsafe fn create(self, parent: *mut lvgl::lv_obj_t) -> *mut lvgl::lv_obj_t {
        unsafe {
            match self {
                Self::Header | Self::Tray | Self::Body => lvgl::lv_obj_create(parent),
                Self::Clock | Self::Network | Self::Battery => lvgl::lv_label_create(parent),
            }
        }
    }
}

/// Owns partially constructed layout widgets and the screen's saved styles
/// while [`Layout::build_layout`] holds the graphics lock. Dropping it without
/// calling [`Self::disarm`] deletes every widget created so far and restores
/// the screen's size and flex flow.
struct PartialLayout {
    window: *mut lvgl::lv_obj_t,
    saved_screen_style: ScreenStyleSnapshot,
    header: Option<*mut lvgl::lv_obj_t>,
    body: Option<*mut lvgl::lv_obj_t>,
    committed: bool,
}

impl PartialLayout {
    /// # Safety
    /// The caller must hold the graphics lock for the entire lifetime of the
    /// returned guard, and `window` must be a valid active screen pointer.
    unsafe fn new(window: *mut lvgl::lv_obj_t) -> Self {
        Self {
            window,
            saved_screen_style: unsafe { ScreenStyleSnapshot::capture(window) },
            header: None,
            body: None,
            committed: false,
        }
    }

    fn disarm(mut self) -> ScreenStyleSnapshot {
        self.committed = true;
        self.saved_screen_style
    }
}

impl Drop for PartialLayout {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        // SAFETY: `PartialLayout` is only created and dropped while the
        // graphics lock is held. Deleting `header` and `body`
        // also deletes their descendants (`clock`, `tray`, `network`,
        // `battery`).
        unsafe {
            if let Some(body) = self.body {
                lvgl::lv_obj_delete(body);
            }
            if let Some(header) = self.header {
                lvgl::lv_obj_delete(header);
            }
            self.saved_screen_style.restore(self.window);
        }
    }
}

pub struct Layout {
    context: &'static xila::executable::ExecutableContext,
    window: *mut lvgl::lv_obj_t,
    saved_screen_style: ScreenStyleSnapshot,
    header: *mut lvgl::lv_obj_t,
    body: *mut lvgl::lv_obj_t,
    clock: *mut lvgl::lv_obj_t,
    clock_string: String,
    _battery: *mut lvgl::lv_obj_t,
    network: *mut lvgl::lv_obj_t,
    last_update: Option<Duration>,
}

impl Drop for Layout {
    fn drop(&mut self) {
        // `self.window` is the display's active screen, not a container owned
        // by `Layout`. Hand future windows back to that screen in a session, so
        // concurrent `create_window` calls cannot attach new windows to
        // `self.body` as it is torn down.
        let graphics_manager = graphics::ffi_manager();
        let screen = self.window;
        let _ = graphics_manager.with_lvgl_sync(|lvgl| {
            // SAFETY: the session holds the graphics lock, and `screen` is the
            // display's active screen, which is owned by the display, is not
            // inside a window, and is never deleted by `Layout::drop`, so it
            // stays alive while it holds windows.
            unsafe { graphics_manager.set_window_parent(lvgl, screen) };
            Ok(())
        });
        let _ = graphics_manager.lock_function_sync(|| {
            // SAFETY: the graphics lock is held. Move remaining windows to
            // the screen so their owners remain responsible for deletion.
            unsafe {
                loop {
                    let window = lvgl::lv_obj_get_child(self.body, 0);
                    if window.is_null() {
                        break;
                    }
                    lvgl::lv_obj_set_parent(window, screen);
                }
                lvgl::lv_obj_delete(self.header);
                lvgl::lv_obj_delete(self.body);
                self.saved_screen_style.restore(screen);
            }
            Ok(())
        });
    }
}

/// Move every window that is a direct child of `screen` into `body`, keeping
/// their order, and leave the screen's other children in place.
///
/// # Safety
/// The caller must hold the graphics lock, and both objects must be live.
unsafe fn adopt_screen_windows(screen: *mut lvgl::lv_obj_t, body: *mut lvgl::lv_obj_t) {
    let mut index = 0;
    loop {
        let child = unsafe { lvgl::lv_obj_get_child(screen, index) };
        if child.is_null() {
            break;
        }
        // SAFETY: `child` is a live child of `screen`.
        if unsafe { graphics::Window::from_raw(child) }.is_some() {
            // Moving the window removes it from the screen's children, so
            // the next child takes its index.
            unsafe { lvgl::lv_obj_set_parent(child, body) };
        } else {
            index += 1;
        }
    }
}

impl Layout {
    pub const UPDATE_INTERVAL: Duration = Duration::from_secs(30);

    pub async fn run(&mut self) {
        let now = self.context.time_manager.monotonic_now();
        if self
            .last_update
            .is_some_and(|last| now - last < Self::UPDATE_INTERVAL)
        {
            return;
        }

        match self
            .context
            .time_manager
            .realtime_now()
            .and_then(|utc| utc.try_as_seconds())
        {
            Ok(seconds) => self.update_clock(Some(seconds)).await,
            Err(error) => {
                self.update_clock(None).await;
                log::error!("Failed to get current time: {}", error);
            }
        }

        if let Err(e) = self.update_network_icon().await {
            log::error!("Failed to update network icon: {}", e);
        }

        self.last_update = Some(now);
    }

    async fn get_interface_symbol(&self, file: &mut File<'_>) -> Result<Option<&CStr>> {
        let kind = match file
            .as_synchronous_file_mut()
            .control(network::GET_KIND, &())
        {
            Ok(kind) => kind,
            // HTTP(S) clients are protocol endpoints, not network interfaces.
            Err(xila::virtual_file_system::IoErrorKind::Unsupported) => return Ok(None),
            Err(error) => return Err(Error::FailedToOpenDirectory(error)),
        };
        let kind = InterfaceKind::try_from(kind).map_err(Error::FailedToOpenDirectory)?;
        let is_up = file
            .as_synchronous_file_mut()
            .control(network::IS_LINK_UP, &())
            .map_err(Error::FailedToOpenDirectory)?;
        let is_up = bool::try_from(is_up).map_err(|_| {
            Error::FailedToOpenDirectory(xila::virtual_file_system::IoErrorKind::InvalidArgument)
        })?;
        if !is_up {
            return Ok(None);
        }

        let symbol = match kind {
            InterfaceKind::WiFi => symbol::WIFI,
            InterfaceKind::Ethernet => symbol::NETWORK_WIRED,
            InterfaceKind::Unknown => c"?",
        };

        Ok(Some(symbol))
    }

    async fn get_network_symbol(&self) -> Result<&CStr> {
        // Browse the network interfaces in the /devices/network directory

        let virtual_file_system = &self.context.virtual_file_system;

        let task_manager = &self.context.task_manager;

        let task = task_manager.get_current_task_identifier().await;

        let mut directory = Directory::open(virtual_file_system, task, Path::NETWORK_DEVICES)
            .await
            .map_err(Error::FailedToOpenDirectory)?;

        while let Some(entry) = directory
            .read()
            .await
            .map_err(Error::FailedToOpenDirectory)?
        {
            if entry.name == "." || entry.name == ".." {
                continue;
            }

            let entry_path = entry.join_path(Path::NETWORK_DEVICES);

            if let Some(entry_path) = entry_path {
                let mut file = File::open(
                    virtual_file_system,
                    task,
                    &entry_path,
                    AccessFlags::Read.into(),
                )
                .await
                .map_err(Error::FailedToOpenDirectory)?;

                let symbol = self.get_interface_symbol(&mut file).await?;

                if let Some(symbol) = symbol {
                    return Ok(symbol);
                }
            }
        }

        Ok(c"")
    }

    async fn update_network_icon(&mut self) -> Result<()> {
        let symbol = self.get_network_symbol().await?;

        let graphics_manager = graphics::ffi_manager();
        graphics::lock!(graphics_manager, {
            unsafe {
                lvgl::lv_label_set_text_static(self.network, symbol.as_ptr());
            }
        });

        Ok(())
    }

    async fn update_clock(&mut self, current_time: Option<i64>) {
        // Read `TZ` again on each update, so that a change made in Settings
        // shows at the next update.
        let task = self
            .context
            .task_manager
            .get_current_task_identifier()
            .await;
        let zone = local_time::current(self.context.task_manager, task).await;
        let graphics_manager = graphics::ffi_manager();
        graphics::lock!(graphics_manager, {
            write_clock_text(&mut self.clock_string, current_time, &zone);
            self.clock_string.push('\0');

            unsafe {
                lvgl::lv_label_set_text_static(
                    self.clock,
                    self.clock_string.as_ptr() as *const core::ffi::c_char,
                );
            }
        });
    }

    #[cfg(test)]
    pub(crate) fn header_object(&self) -> *mut lvgl::lv_obj_t {
        self.header
    }

    #[cfg(test)]
    pub(crate) fn body_object(&self) -> *mut lvgl::lv_obj_t {
        self.body
    }

    pub fn get_windows_parent(&self) -> *mut lvgl::lv_obj_t {
        self.window
    }

    pub async fn new(context: &'static xila::executable::ExecutableContext) -> Result<Self> {
        let graphics_manager = graphics::ffi_manager();
        Self::new_with(context, graphics_manager, |widget, parent| {
            // SAFETY: `build_layout` invokes this callback only while holding
            // the graphics lock with a valid LVGL parent.
            unsafe { widget.create(parent) }
        })
        .await
    }

    pub(crate) async fn new_with(
        context: &'static xila::executable::ExecutableContext,
        graphics_manager: &'static graphics::Manager,
        create_widget: impl FnMut(LayoutWidget, *mut lvgl::lv_obj_t) -> *mut lvgl::lv_obj_t,
    ) -> Result<Self> {
        let layout = graphics::lock!(graphics_manager, {
            // SAFETY: the graphics lock is held for the entire call, so
            // `PartialLayout::drop` runs before the lock is released on error.
            unsafe { Self::build_layout(context, create_widget)? }
        });

        graphics_manager
            .with_lvgl(|lvgl| {
                // SAFETY: the session holds the graphics lock, the body is not
                // inside a window, and it lives as long as the layout, which
                // hands the windows still inside it back to the screen before
                // deleting it.
                unsafe { graphics_manager.set_window_parent(lvgl, layout.body) };
                Ok(())
            })
            .await?;

        // Windows that a previous shell left on the screen, or that were
        // created there before the line above, belong in the new body.
        // `create_window` now targets the body, so no window can be added to
        // the screen after this pass.
        graphics::lock!(graphics_manager, {
            // SAFETY: the graphics lock is held, and both objects are live.
            unsafe { adopt_screen_windows(layout.window, layout.body) }
        });

        Ok(layout)
    }

    /// # Safety
    /// The caller must hold the graphics lock for the duration of this call.
    unsafe fn build_layout(
        context: &'static xila::executable::ExecutableContext,
        mut create_widget: impl FnMut(LayoutWidget, *mut lvgl::lv_obj_t) -> *mut lvgl::lv_obj_t,
    ) -> Result<Self> {
        // - Create a window
        let window = unsafe { lvgl::lv_screen_active() };
        if window.is_null() {
            return Err(Error::FailedToCreateObject);
        }

        // Capture the screen's styles before modifying them so any early
        // error restores the screen and deletes every widget created so far.
        let mut partial = unsafe { PartialLayout::new(window) };

        unsafe {
            lvgl::lv_obj_set_size(window, lvgl::lv_pct(100), lvgl::lv_pct(100));
            lvgl::lv_obj_set_flex_flow(window, lvgl::lv_flex_flow_t_LV_FLEX_FLOW_COLUMN);
        }

        // - Create a header
        let header = create_widget(LayoutWidget::Header, window);
        if header.is_null() {
            return Err(Error::FailedToCreateObject);
        }
        partial.header = Some(header);
        unsafe {
            lvgl::lv_obj_set_size(header, lvgl::lv_pct(100), lvgl::LV_SIZE_CONTENT);
            // Flat status-bar chrome: keep the theme surface but drop the
            // card padding, border and radius so the height stays slim.
            lvgl::lv_obj_set_style_pad_ver(header, 4, lvgl::LV_STATE_DEFAULT);
            lvgl::lv_obj_set_style_pad_hor(header, 8, lvgl::LV_STATE_DEFAULT);
            lvgl::lv_obj_set_style_border_width(header, 0, lvgl::LV_STATE_DEFAULT);
            lvgl::lv_obj_set_style_radius(header, 0, lvgl::LV_STATE_DEFAULT);
            theme::set_surface(header, theme::Surface::Muted);
        }

        // - - Create a label for the clock
        let clock = create_widget(LayoutWidget::Clock, header);
        if clock.is_null() {
            return Err(Error::FailedToCreateObject);
        }
        unsafe {
            lvgl::lv_obj_set_align(clock, lvgl::lv_align_t_LV_ALIGN_CENTER);
        }

        // - - Create a flex tray for the right side
        let tray = create_widget(LayoutWidget::Tray, header);
        if tray.is_null() {
            return Err(Error::FailedToCreateObject);
        }
        unsafe {
            lvgl::lv_obj_set_size(tray, lvgl::LV_SIZE_CONTENT, lvgl::LV_SIZE_CONTENT);
            lvgl::lv_obj_set_flex_flow(tray, lvgl::lv_flex_flow_t_LV_FLEX_FLOW_ROW);
            lvgl::lv_obj_set_style_pad_all(tray, 0, lvgl::LV_STATE_DEFAULT);
            lvgl::lv_obj_set_style_border_width(tray, 0, lvgl::LV_STATE_DEFAULT);
            lvgl::lv_obj_set_style_radius(tray, 0, lvgl::LV_STATE_DEFAULT);
            lvgl::lv_obj_align(tray, lvgl::lv_align_t_LV_ALIGN_RIGHT_MID, 0, 0);
            lvgl::lv_obj_set_style_bg_opa(tray, lvgl::LV_OPA_TRANSP as _, lvgl::LV_STATE_DEFAULT);
        }

        // - - Create a label for the WiFi
        let network = create_widget(LayoutWidget::Network, tray);
        if network.is_null() {
            return Err(Error::FailedToCreateObject);
        }
        unsafe {
            lvgl::lv_label_set_text(network, c"".as_ptr());
        }

        // - - Create a label for the battery
        let battery = create_widget(LayoutWidget::Battery, tray);
        if battery.is_null() {
            return Err(Error::FailedToCreateObject);
        }
        unsafe {
            lvgl::lv_label_set_text_static(battery, symbol::BATTERY_3.as_ptr());
        }

        // - - Create a body object
        let body = create_widget(LayoutWidget::Body, window);
        if body.is_null() {
            return Err(Error::FailedToCreateObject);
        }
        partial.body = Some(body);
        unsafe {
            lvgl::lv_obj_add_flag(body, lvgl::lv_obj_flag_t_LV_OBJ_FLAG_EVENT_BUBBLE);
            lvgl::lv_obj_set_width(body, lvgl::lv_pct(100));
            lvgl::lv_obj_set_flex_grow(body, 1);
            // Edge-to-edge window area: the theme container would inset every
            // child window with card padding, border and radius.
            lvgl::lv_obj_set_style_pad_all(body, 0, lvgl::LV_STATE_DEFAULT);
            lvgl::lv_obj_set_style_border_width(body, 0, lvgl::LV_STATE_DEFAULT);
            lvgl::lv_obj_set_style_radius(body, 0, lvgl::LV_STATE_DEFAULT);
        }

        let saved_screen_style = partial.disarm();

        Ok(Self {
            context,
            window,
            saved_screen_style,
            header,
            body,
            clock,
            clock_string: String::with_capacity(6),
            _battery: battery,
            network,
            last_update: None,
        })
    }
}

#[cfg(test)]
mod clock_tests {
    use super::*;

    #[test]
    fn clock_text_handles_extreme_and_pre_epoch_utc_with_bounded_work() {
        let mut text = String::new();
        for (seconds, expected) in [
            (None, "--:--"),
            (Some(0), "00:00"),
            (Some(-1), "23:59"),
            (Some(i64::MAX), "15:30"),
            (Some(i64::MIN), "08:29"),
        ] {
            write_clock_text(&mut text, seconds, &local_time::utc());
            assert_eq!(text, expected);
        }
    }

    #[test]
    fn clock_text_is_local_across_daylight_saving_changes() {
        let paris = TimeZone::parse("CET-1CEST,M3.5.0,M10.5.0/3").unwrap();
        let mut text = String::new();
        for (seconds, expected) in [
            // 2026-03-29: 01:59 CET, then 03:00 CEST.
            (1_774_745_940, "01:59"),
            (1_774_746_000, "03:00"),
            // 2026-10-25: 02:59 CEST, then 02:00 CET again.
            (1_792_889_940, "02:59"),
            (1_792_890_000, "02:00"),
            // Before the epoch and at the extremes, with an offset.
            (-1, "00:59"),
        ] {
            write_clock_text(&mut text, Some(seconds), &paris);
            assert_eq!(text, expected, "{seconds}");
        }
        for seconds in [i64::MAX, i64::MIN] {
            write_clock_text(&mut text, Some(seconds), &paris);
            assert_eq!(text.len(), 5);
        }
        let kathmandu = TimeZone::parse("<+0545>-5:45").unwrap();
        write_clock_text(&mut text, Some(0), &kathmandu);
        assert_eq!(text, "05:45");
    }
}

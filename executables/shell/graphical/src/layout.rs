use crate::error::{Error, Result};
use alloc::string::String;
use core::ffi::CStr;
use core::ptr::null_mut;
use core::time::Duration;
use xila::file_system::{AccessFlags, Path};
use xila::graphics::{self, EventKind, lvgl, symbol, theme};
use xila::log;
use xila::network::InterfaceKind;
use xila::virtual_file_system::{Directory, File};
use xila::{internationalization, network};

// Only the time of day is displayed. Bound calendar conversion independently of
// the signed timestamp range, including far-future and pre-epoch corrections.
fn write_clock_text(output: &mut String, current_time: Option<i64>) {
    output.clear();
    if let Some(seconds) = current_time {
        let _ = internationalization::DateTimeFormatter::new(
            internationalization::date_time_locale(internationalization::locale!()),
        )
        .write(output, seconds.rem_euclid(86_400), "%H:%M");
    } else {
        output.push_str("--:--");
    }
}

const KEYBOARD_SIZE_RATIO: f64 = 3.0 / 1.0;

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
    Keyboard,
}

impl LayoutWidget {
    /// # Safety
    /// The caller must hold the graphics lock and pass a valid LVGL parent.
    pub(crate) unsafe fn create(self, parent: *mut lvgl::lv_obj_t) -> *mut lvgl::lv_obj_t {
        unsafe {
            match self {
                Self::Header | Self::Tray | Self::Body => lvgl::lv_obj_create(parent),
                Self::Clock | Self::Network | Self::Battery => lvgl::lv_label_create(parent),
                Self::Keyboard => lvgl::lv_keyboard_create(parent),
            }
        }
    }
}

/// Owns partially constructed layout widgets and the screen's saved styles
/// while [`Layout::build_layout`] holds the graphics lock. Dropping it without
/// calling [`Self::disarm`] deletes every widget created so far and restores
/// the screen's size, flex flow, and callbacks.
struct PartialLayout {
    window: *mut lvgl::lv_obj_t,
    saved_screen_style: ScreenStyleSnapshot,
    header: Option<*mut lvgl::lv_obj_t>,
    body: Option<*mut lvgl::lv_obj_t>,
    keyboard: Option<*mut lvgl::lv_obj_t>,
    screen_callbacks_attached: bool,
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
            keyboard: None,
            screen_callbacks_attached: false,
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
        // graphics lock is held. Deleting `header`, `body`, and `keyboard`
        // also deletes their descendants (`clock`, `tray`, `network`,
        // `battery`).
        unsafe {
            if let Some(keyboard) = self.keyboard {
                if self.screen_callbacks_attached {
                    lvgl::lv_obj_remove_event_cb_with_user_data(
                        self.window,
                        Some(screen_event_handler),
                        keyboard.cast(),
                    );
                    lvgl::lv_obj_remove_event_cb_with_user_data(
                        self.window,
                        Some(keyboard_event_handler),
                        keyboard.cast(),
                    );
                }
                lvgl::lv_obj_delete(keyboard);
            }
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
    keyboard: *mut lvgl::lv_obj_t,
    clock: *mut lvgl::lv_obj_t,
    clock_string: String,
    _battery: *mut lvgl::lv_obj_t,
    network: *mut lvgl::lv_obj_t,
    last_update: Option<Duration>,
}

impl Drop for Layout {
    fn drop(&mut self) {
        // `self.window` is the display's active screen, not a container owned
        // by `Layout`. Hand future windows back to that screen before taking
        // the graphics lock so concurrent `create_window` calls cannot attach
        // new windows to `self.body` as it is torn down.
        let graphics_manager = graphics::ffi_manager();
        let screen = self.window;
        // SAFETY: `screen` is the display's active screen, which is owned by
        // the display, is not inside a window, and is never deleted by
        // `Layout::drop`, so it stays alive while it holds windows.
        let _ = xila::task::block_on(unsafe { graphics_manager.set_window_parent(screen) });
        let _ = graphics_manager.lock_function_sync(|| {
            // SAFETY: the graphics lock is held. Detach `Layout`'s callbacks
            // from the screen before deleting `keyboard`, move every remaining
            // window out of `body` onto `screen` so each window's
            // `OwnedWindow` remains its sole deleter, delete only the widgets
            // `Layout::new` created, and restore the screen's saved styles.
            unsafe {
                lvgl::lv_obj_remove_event_cb_with_user_data(
                    screen,
                    Some(screen_event_handler),
                    self.keyboard.cast(),
                );
                lvgl::lv_obj_remove_event_cb_with_user_data(
                    screen,
                    Some(keyboard_event_handler),
                    self.keyboard.cast(),
                );
                loop {
                    let window = lvgl::lv_obj_get_child(self.body, 0);
                    if window.is_null() {
                        break;
                    }
                    lvgl::lv_obj_set_parent(window, screen);
                }
                lvgl::lv_obj_delete(self.header);
                lvgl::lv_obj_delete(self.body);
                lvgl::lv_obj_delete(self.keyboard);
                self.saved_screen_style.restore(screen);
            }
            Ok(())
        });
    }
}

/// Keyboard event handler.
///
/// # Safety
///
/// This function is unsafe because it dereferences raw pointers.
pub unsafe extern "C" fn keyboard_event_handler(event: *mut lvgl::lv_event_t) {
    unsafe {
        let code = lvgl::lv_event_get_code(event);
        let code = EventKind::from_lvgl_code(code);
        let keyboard = lvgl::lv_event_get_user_data(event) as *mut lvgl::lv_obj_t;

        if keyboard.is_null() {
            return;
        }

        match code {
            EventKind::Ready => {
                let focused_input = lvgl::lv_keyboard_get_textarea(keyboard);

                if focused_input.is_null() {
                    return;
                }

                lvgl::lv_obj_send_event(
                    focused_input,
                    EventKind::Ready.into_lvgl_code(),
                    null_mut(),
                );

                lvgl::lv_keyboard_set_textarea(keyboard, null_mut());
                lvgl::lv_obj_add_flag(keyboard, lvgl::lv_obj_flag_t_LV_OBJ_FLAG_HIDDEN);
            }
            EventKind::Cancel => {
                lvgl::lv_keyboard_set_textarea(keyboard, null_mut());
                lvgl::lv_obj_add_flag(keyboard, lvgl::lv_obj_flag_t_LV_OBJ_FLAG_HIDDEN);
            }

            _ => {}
        }
    }
}

/// Screen event handler.
///
/// # Safety
///
/// This function is unsafe because it dereferences raw pointers.
pub unsafe extern "C" fn screen_event_handler(event: *mut lvgl::lv_event_t) {
    unsafe {
        let code = lvgl::lv_event_get_code(event);
        let code = EventKind::from_lvgl_code(code);
        let target = lvgl::lv_event_get_target_obj(event);
        let keyboard = lvgl::lv_event_get_user_data(event) as *mut lvgl::lv_obj_t;

        if target.is_null() || keyboard.is_null() {
            return;
        }

        match code {
            EventKind::Focused if lvgl::lv_obj_has_class(target, &lvgl::lv_textarea_class) => {
                lvgl::lv_keyboard_set_textarea(keyboard, target);
                lvgl::lv_obj_remove_flag(keyboard, lvgl::lv_obj_flag_t_LV_OBJ_FLAG_HIDDEN);
                lvgl::lv_obj_move_foreground(keyboard);

                let width = lvgl::lv_obj_get_width(keyboard);
                lvgl::lv_obj_set_height(keyboard, (width as f64 / KEYBOARD_SIZE_RATIO) as i32);
            }
            EventKind::Defocused if lvgl::lv_obj_has_class(target, &lvgl::lv_textarea_class) => {
                lvgl::lv_keyboard_set_textarea(keyboard, null_mut());
                lvgl::lv_obj_add_flag(keyboard, lvgl::lv_obj_flag_t_LV_OBJ_FLAG_HIDDEN);
            }
            _ => {}
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
            Err(
                xila::virtual_file_system::Error::FileSystem(
                    xila::file_system::Error::UnsupportedOperation,
                )
                | xila::virtual_file_system::Error::UnsupportedOperation,
            ) => return Ok(None),
            Err(error) => return Err(Error::FailedToOpenDirectory(error)),
        };
        let is_up = file
            .as_synchronous_file_mut()
            .control(network::IS_LINK_UP, &())
            .map_err(Error::FailedToOpenDirectory)?;
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
        let graphics_manager = graphics::ffi_manager();
        graphics::lock!(graphics_manager, {
            write_clock_text(&mut self.clock_string, current_time);
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

    #[cfg(test)]
    pub(crate) fn keyboard_object(&self) -> *mut lvgl::lv_obj_t {
        self.keyboard
    }

    pub fn get_windows_parent(&self) -> *mut lvgl::lv_obj_t {
        self.window
    }

    pub async fn new(
        context: &'static xila::executable::ExecutableContext,
        show_keyboard: bool,
    ) -> Result<Self> {
        let graphics_manager = graphics::ffi_manager();
        Self::new_with(
            context,
            graphics_manager,
            show_keyboard,
            |widget, parent| {
                // SAFETY: `build_layout` invokes this callback only while holding
                // the graphics lock with a valid LVGL parent.
                unsafe { widget.create(parent) }
            },
        )
        .await
    }

    pub(crate) async fn new_with(
        context: &'static xila::executable::ExecutableContext,
        graphics_manager: &'static graphics::Manager,
        show_keyboard: bool,
        create_widget: impl FnMut(LayoutWidget, *mut lvgl::lv_obj_t) -> *mut lvgl::lv_obj_t,
    ) -> Result<Self> {
        let layout = graphics::lock!(graphics_manager, {
            // SAFETY: the graphics lock is held for the entire call, so
            // `PartialLayout::drop` runs before the lock is released on error.
            unsafe { Self::build_layout(context, show_keyboard, create_widget)? }
        });

        // SAFETY: the body is not inside a window, and it lives as long as
        // the layout, which hands the windows still inside it back to the
        // screen before deleting it.
        unsafe { graphics_manager.set_window_parent(layout.body) }.await?;

        Ok(layout)
    }

    /// # Safety
    /// The caller must hold the graphics lock for the duration of this call.
    unsafe fn build_layout(
        context: &'static xila::executable::ExecutableContext,
        show_keyboard: bool,
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

        // - Create a keyboard
        let keyboard = create_widget(LayoutWidget::Keyboard, window);
        if keyboard.is_null() {
            return Err(Error::FailedToCreateObject);
        }
        partial.keyboard = Some(keyboard);
        unsafe {
            lvgl::lv_obj_add_flag(keyboard, lvgl::lv_obj_flag_t_LV_OBJ_FLAG_HIDDEN);

            if show_keyboard {
                partial.screen_callbacks_attached = true;
                lvgl::lv_obj_add_event_cb(
                    window,
                    Some(screen_event_handler),
                    EventKind::Focused.into_lvgl_code(),
                    keyboard as *mut _,
                );
                lvgl::lv_obj_add_event_cb(
                    window,
                    Some(screen_event_handler),
                    EventKind::Defocused.into_lvgl_code(),
                    keyboard as *mut _,
                );
                lvgl::lv_obj_add_event_cb(
                    keyboard,
                    Some(keyboard_event_handler),
                    EventKind::Ready.into_lvgl_code(),
                    keyboard as *mut _,
                );
                lvgl::lv_obj_add_event_cb(
                    window,
                    Some(keyboard_event_handler),
                    EventKind::Cancel.into_lvgl_code(),
                    keyboard as *mut _,
                );
            }
        }

        let saved_screen_style = partial.disarm();

        Ok(Self {
            context,
            window,
            saved_screen_style,
            header,
            body,
            keyboard,
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
            write_clock_text(&mut text, seconds);
            assert_eq!(text, expected);
        }
    }
}

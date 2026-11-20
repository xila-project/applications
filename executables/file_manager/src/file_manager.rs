use crate::error::{Error, Result};
pub(crate) use alloc::{
    borrow::ToOwned,
    ffi::CString,
    string::{String, ToString},
    vec::Vec,
};
use core::ptr::null_mut;
use xila::graphics::fonts;
use xila::log;
use xila::task;
use xila::virtual_file_system::Directory;
use xila::{
    file_system::{Kind, Path, PathOwned},
    graphics::{
        self, EventKind, Key, lvgl,
        palette::{self, Hue},
    },
};
use xila::{graphics::OwnedWindow, internationalization::t};

pub struct FileManager {
    context: &'static xila::executable::ExecutableContext,
    graphics_manager: &'static graphics::Manager,
    window: OwnedWindow,
    toolbar: *mut lvgl::lv_obj_t,
    up_button: *mut lvgl::lv_obj_t,
    home_button: *mut lvgl::lv_obj_t,
    refresh_button: *mut lvgl::lv_obj_t,
    path_text_area: *mut lvgl::lv_obj_t,
    go_button: *mut lvgl::lv_obj_t,
    status: *mut lvgl::lv_obj_t,
    file_list: *mut lvgl::lv_obj_t,
    current_path: PathOwned,
    files: Vec<FileItem>,
    running: bool,
}

#[derive(Clone)]
pub struct FileItem {
    pub name: String,
    pub kind: Kind,
    pub size: u64,
}

impl FileManager {
    pub async fn new(context: &'static xila::executable::ExecutableContext) -> Result<Self> {
        let graphics_manager = graphics::ffi_manager();
        Self::new_at(context, graphics_manager, PathOwned::root()).await
    }

    async fn new_at(
        context: &'static xila::executable::ExecutableContext,
        graphics_manager: &'static graphics::Manager,
        initial_path: PathOwned,
    ) -> Result<Self> {
        let manager = graphics::lock!(graphics_manager, {
            let mut window = graphics_manager.create_window().await?;

            window.set_icon("Fm", palette::get(Hue::Cyan, palette::Tone::MAIN));

            let mut manager = Self {
                context,
                graphics_manager,
                window,
                toolbar: null_mut(),
                up_button: null_mut(),
                home_button: null_mut(),
                refresh_button: null_mut(),
                path_text_area: null_mut(),
                go_button: null_mut(),
                status: null_mut(),
                file_list: null_mut(),
                current_path: PathOwned::root(),
                files: Vec::new(),
                running: true,
            };

            if let Err(error) = manager.setup_ui(initial_path).await {
                // SAFETY: the graphics lock is held, and `manager` owns the
                // window. Delete it directly so `OwnedWindow::drop` does not
                // deadlock on the non-re-entrant lock already held here.
                unsafe {
                    lvgl::lv_obj_delete(manager.window.into_raw().as_ptr().cast());
                }
                return Err(error);
            }

            manager
        });

        Ok(manager)
    }

    async fn setup_ui(&mut self, initial_path: PathOwned) -> Result<()> {
        // Set up window layout for flex
        unsafe {
            lvgl::lv_obj_set_layout(
                self.window.as_object_mutable(),
                lvgl::lv_layout_t_LV_LAYOUT_FLEX,
            );
            lvgl::lv_obj_set_flex_flow(
                self.window.as_object_mutable(),
                lvgl::lv_flex_flow_t_LV_FLEX_FLOW_COLUMN,
            );
            lvgl::lv_obj_set_style_pad_all(
                self.window.as_object_mutable(),
                0,
                lvgl::LV_STATE_DEFAULT,
            );
            // Toolbar, status line and list form one continuous surface.
            lvgl::lv_obj_set_style_pad_row(
                self.window.as_object_mutable(),
                0,
                lvgl::LV_STATE_DEFAULT,
            );
        }

        self.create_toolbar().await?;
        self.create_status()?;
        self.create_file_list().await?;
        self.open(initial_path).await?;

        Ok(())
    }

    pub async fn handle_events(&mut self) -> bool {
        graphics::lock!(self.graphics_manager, {
            while let Some(event) = self.window.pop_event() {
                if let Err(e) = self.handle_event(event).await {
                    log::error!("{}", t!("errors.event_handling_failed", arg0 = e));
                }
            }
        });

        task::suspend(|ctx| {
            self.window.register_waker(ctx.waker());
        })
        .await;

        self.running
    }

    pub async fn handle_event(&mut self, event: graphics::Event) -> Result<()> {
        match event.code {
            EventKind::Delete | EventKind::CloseRequested
                if event.is_target(self.window.as_object()) =>
            {
                self.running = false;
            }
            EventKind::Clicked => {
                // Handle different button clicks
                if event.is_target(self.up_button) {
                    self.handle_up_click().await?;
                } else if event.is_target(self.home_button) {
                    self.handle_home_click().await?;
                } else if event.is_target(self.refresh_button) {
                    self.handle_refresh_click().await?;
                } else if event.is_target(self.go_button) {
                    self.handle_go_click().await?;
                } else {
                    // Handle file item clicks
                    self.handle_file_click(&event).await?;
                }
            }
            // Enter in the path field navigates like the Go button.
            EventKind::Key if event.is_target(self.path_text_area) => {
                if let Some(Key::Enter) = event.key {
                    self.handle_go_click().await?;
                }
            }
            _ => {}
        }

        Ok(())
    }

    async fn create_toolbar(&mut self) -> Result<()> {
        unsafe {
            self.toolbar = lvgl::lv_obj_create(self.window.as_object_mutable());
            if self.toolbar.is_null() {
                return Err(Error::FailedToCreateObject);
            }

            // Toolbar - content-height flex child of the window's column layout,
            // flattened to match the window's edge-to-edge chrome
            lvgl::lv_obj_set_size(self.toolbar, lvgl::lv_pct(100), lvgl::LV_SIZE_CONTENT);
            lvgl::lv_obj_set_style_border_width(self.toolbar, 0, lvgl::LV_STATE_DEFAULT);
            lvgl::lv_obj_set_style_radius(self.toolbar, 0, lvgl::LV_STATE_DEFAULT);
            lvgl::lv_obj_set_layout(self.toolbar, lvgl::lv_layout_t_LV_LAYOUT_FLEX);
            lvgl::lv_obj_set_flex_flow(self.toolbar, lvgl::lv_flex_flow_t_LV_FLEX_FLOW_ROW);
            lvgl::lv_obj_set_flex_align(
                self.toolbar,
                lvgl::lv_flex_align_t_LV_FLEX_ALIGN_START,
                lvgl::lv_flex_align_t_LV_FLEX_ALIGN_CENTER,
                lvgl::lv_flex_align_t_LV_FLEX_ALIGN_CENTER,
            );
            lvgl::lv_obj_remove_flag(self.toolbar, lvgl::lv_obj_flag_t_LV_OBJ_FLAG_SCROLLABLE);
            // Narrow windows keep the path field usable with tighter spacing.
            lvgl::lv_obj_update_layout(self.window.as_object_mutable());
            let compact = lvgl::lv_obj_get_width(self.window.as_object_mutable()) < 560;
            let spacing = if compact { 4 } else { 8 };
            lvgl::lv_obj_set_style_pad_all(self.toolbar, spacing, lvgl::LV_STATE_DEFAULT);
            lvgl::lv_obj_set_style_pad_column(self.toolbar, spacing, lvgl::LV_STATE_DEFAULT);

            // Up button
            self.up_button = lvgl::lv_button_create(self.toolbar);
            if self.up_button.is_null() {
                return Err(Error::FailedToCreateObject);
            }
            let up_label = lvgl::lv_label_create(self.up_button);
            lvgl::lv_label_set_text(
                up_label,
                lvgl::LV_SYMBOL_UP as *const _ as *const core::ffi::c_char,
            );
            lvgl::lv_obj_center(up_label);

            // Remove event handler - events bubble up to window

            // Home button
            self.home_button = lvgl::lv_button_create(self.toolbar);
            if self.home_button.is_null() {
                return Err(Error::FailedToCreateObject);
            }

            let home_label = lvgl::lv_label_create(self.home_button);
            lvgl::lv_label_set_text(
                home_label,
                lvgl::LV_SYMBOL_HOME as *const _ as *const core::ffi::c_char,
            );
            lvgl::lv_obj_center(home_label);

            // Remove event handler - events bubble up to window

            // Refresh button
            self.refresh_button = lvgl::lv_button_create(self.toolbar);
            if self.refresh_button.is_null() {
                return Err(Error::FailedToCreateObject);
            }

            let refresh_label = lvgl::lv_label_create(self.refresh_button);

            lvgl::lv_label_set_text(
                refresh_label,
                lvgl::LV_SYMBOL_REFRESH as *const _ as *const core::ffi::c_char,
            );
            lvgl::lv_obj_center(refresh_label);

            // Navigation is secondary to the content: only Go keeps the
            // primary style.
            for button in [self.up_button, self.home_button, self.refresh_button] {
                lvgl::lv_theme_shadcn_set_variant(
                    button,
                    lvgl::lv_theme_shadcn_variant_t_LV_THEME_SHADCN_VARIANT_OUTLINE,
                );
            }

            // Remove event handler - events bubble up to window

            // Path text area - use flex grow to take remaining space
            self.path_text_area = lvgl::lv_textarea_create(self.toolbar);
            if self.path_text_area.is_null() {
                return Err(Error::FailedToCreateObject);
            }

            lvgl::lv_obj_set_flex_grow(self.path_text_area, 1); // Take remaining space

            // Configure text area properties
            lvgl::lv_textarea_set_one_line(self.path_text_area, true);
            lvgl::lv_textarea_set_cursor_click_pos(self.path_text_area, true);

            // Go button
            self.go_button = lvgl::lv_button_create(self.toolbar);
            if self.go_button.is_null() {
                return Err(Error::FailedToCreateObject);
            }

            let go_label = lvgl::lv_label_create(self.go_button);
            lvgl::lv_label_set_text(
                go_label,
                lvgl::LV_SYMBOL_RIGHT as *const _ as *const core::ffi::c_char,
            );
            lvgl::lv_obj_center(go_label);

            if compact {
                for button in [
                    self.up_button,
                    self.home_button,
                    self.refresh_button,
                    self.go_button,
                ] {
                    lvgl::lv_obj_set_style_pad_hor(button, 10, lvgl::LV_STATE_DEFAULT);
                }
            }

            self.update_path_label();
        }

        Ok(())
    }

    /// One line under the toolbar: folder summary, selected file or error.
    fn create_status(&mut self) -> Result<()> {
        unsafe {
            self.status = lvgl::lv_label_create(self.window.as_object_mutable());
            if self.status.is_null() {
                return Err(Error::FailedToCreateObject);
            }
            lvgl::lv_label_set_text(self.status, c"".as_ptr());
            lvgl::lv_obj_set_width(self.status, lvgl::lv_pct(100));
            lvgl::lv_label_set_long_mode(
                self.status,
                lvgl::lv_label_long_mode_t_LV_LABEL_LONG_MODE_DOTS,
            );
            lvgl::lv_obj_set_style_text_font(
                self.status,
                fonts::get_font_small(),
                lvgl::LV_STATE_DEFAULT,
            );
            lvgl::lv_obj_set_style_pad_hor(self.status, 12, lvgl::LV_STATE_DEFAULT);
            lvgl::lv_obj_set_style_pad_top(self.status, 2, lvgl::LV_STATE_DEFAULT);
            lvgl::lv_obj_set_style_pad_bottom(self.status, 8, lvgl::LV_STATE_DEFAULT);
            graphics::theme::set_surface(self.status, graphics::theme::Surface::Card);
            lvgl::lv_obj_set_style_bg_opa(self.status, lvgl::LV_OPA_COVER, lvgl::LV_STATE_DEFAULT);
        }
        Ok(())
    }

    fn set_status(&self, message: &str, error: bool) {
        // Catalog text, names and paths contain no interior NULs.
        if let Ok(message) = CString::new(message) {
            unsafe {
                lvgl::lv_label_set_text(self.status, message.as_ptr());
                lvgl::lv_theme_shadcn_set_error(self.status, error);
                lvgl::lv_obj_set_style_text_opa(
                    self.status,
                    if error { lvgl::LV_OPA_COVER } else { MUTED },
                    lvgl::LV_STATE_DEFAULT,
                );
            }
        }
    }

    fn show_summary(&self) {
        let folders = self
            .files
            .iter()
            .filter(|file| file.kind == Kind::Directory)
            .count();
        self.set_status(
            &alloc::format!(
                "{}",
                t!(
                    "status.summary",
                    arg0 = folders,
                    arg1 = self.files.len() - folders
                )
            ),
            false,
        );
    }

    async fn create_file_list(&mut self) -> Result<()> {
        unsafe {
            self.file_list = lvgl::lv_list_create(self.window.as_object_mutable());
            if self.file_list.is_null() {
                return Err(Error::FailedToCreateObject);
            }

            // File list properties - use flex grow to fill remaining space
            lvgl::lv_obj_set_width(self.file_list, lvgl::lv_pct(100));
            lvgl::lv_obj_set_flex_grow(self.file_list, 1); // Take remaining vertical space

            // Ensure proper scrolling behavior
            lvgl::lv_obj_set_style_pad_all(self.file_list, 0, lvgl::LV_STATE_DEFAULT);
            // Edge-to-edge like the window: no card frame around the rows.
            lvgl::lv_obj_set_style_border_width(self.file_list, 0, lvgl::LV_STATE_DEFAULT);
            lvgl::lv_obj_set_style_radius(self.file_list, 0, lvgl::LV_STATE_DEFAULT);
        }

        Ok(())
    }

    /// Reads and sorts a directory without touching the interface, so a
    /// failed navigation leaves the current listing in place.
    async fn read_directory(&self, path: &Path) -> Result<Vec<FileItem>> {
        let task = self
            .context
            .task_manager
            .get_current_task_identifier()
            .await;
        let virtual_file_system = &self.context.virtual_file_system;
        let mut directory = Directory::open(virtual_file_system, task, path).await?;

        let mut files = Vec::new();
        while let Some(entry) = directory.read().await? {
            // Skip "." and ".." entries
            if entry.name == "." || entry.name == ".." {
                continue;
            }
            files.push(FileItem {
                name: entry.name.clone(),
                kind: entry.kind,
                size: entry.size,
            });
        }

        // Directories first, then files, each by name.
        files.sort_by(|a, b| match (a.kind, b.kind) {
            (Kind::Directory, Kind::Directory) => a.name.cmp(&b.name),
            (Kind::Directory, _) => core::cmp::Ordering::Less,
            (_, Kind::Directory) => core::cmp::Ordering::Greater,
            _ => a.name.cmp(&b.name),
        });
        Ok(files)
    }

    /// Shows `path`, or reports why it cannot be opened and stays in place.
    async fn open(&mut self, path: PathOwned) -> Result<()> {
        let files = match self.read_directory(&path).await {
            Ok(files) => files,
            Err(error) => {
                self.set_status(
                    &alloc::format!("{}", t!("status.open_failed", arg0 = path, arg1 = error)),
                    true,
                );
                return Err(error);
            }
        };
        self.clear_file_list();
        self.current_path = path;
        self.files = files;
        self.update_path_label();
        for i in 0..self.files.len() {
            self.create_file_item(i).await?;
        }
        unsafe {
            if self.files.is_empty() {
                lvgl::lv_list_add_text(self.file_list, t!(c"status.empty").as_ptr());
            }
            lvgl::lv_obj_scroll_to_y(self.file_list, 0, false);
            lvgl::lv_obj_set_state(
                self.up_button,
                lvgl::lv_state_t_LV_STATE_DISABLED,
                self.current_path.go_parent().is_none(),
            );
        }
        self.show_summary();
        Ok(())
    }

    async fn load_directory(&mut self) -> Result<()> {
        self.open(self.current_path.clone()).await
    }

    async fn create_file_item(&mut self, index: usize) -> Result<()> {
        unsafe {
            let file = &self.files[index];

            let icon_symbol = match file.kind {
                Kind::Directory => lvgl::LV_SYMBOL_DIRECTORY,
                _ => lvgl::LV_SYMBOL_FILE,
            };

            let name_cstring = CString::new(file.name.clone()).unwrap();
            let button = lvgl::lv_list_add_button(
                self.file_list,
                icon_symbol.as_ptr() as *const core::ffi::c_void,
                name_cstring.as_ptr(),
            );

            if button.is_null() {
                return Err(Error::FailedToCreateObject);
            }

            // Directories lead somewhere; files show how large they are.
            let detail = lvgl::lv_label_create(button);
            if detail.is_null() {
                return Err(Error::FailedToCreateObject);
            }
            let detail_text = match file.kind {
                Kind::Directory => CString::from(
                    core::ffi::CStr::from_bytes_with_nul(lvgl::LV_SYMBOL_RIGHT).unwrap_or_default(),
                ),
                _ => CString::new(format_size(file.size)).unwrap_or_default(),
            };
            lvgl::lv_label_set_text(detail, detail_text.as_ptr());
            lvgl::lv_obj_set_style_text_font(
                detail,
                fonts::get_font_small(),
                lvgl::LV_STATE_DEFAULT,
            );
            lvgl::lv_obj_set_style_text_opa(detail, MUTED, lvgl::LV_STATE_DEFAULT);
            lvgl::lv_obj_set_flex_align(
                button,
                lvgl::lv_flex_align_t_LV_FLEX_ALIGN_START,
                lvgl::lv_flex_align_t_LV_FLEX_ALIGN_CENTER,
                lvgl::lv_flex_align_t_LV_FLEX_ALIGN_CENTER,
            );
            lvgl::lv_obj_set_style_pad_right(button, 16, lvgl::LV_STATE_DEFAULT);

            // Store the file index in the button's user data for identification
            lvgl::lv_obj_set_user_data(button, index as *mut core::ffi::c_void);
        }

        Ok(())
    }

    fn clear_file_list(&mut self) {
        unsafe {
            // Clear the list content - lvgl:: list automatically manages its children
            lvgl::lv_obj_clean(self.file_list);
        }
        self.files.clear();
    }

    fn update_path_label(&self) {
        unsafe {
            if !self.path_text_area.is_null() {
                let path_text = CString::new(self.current_path.to_string()).unwrap();
                lvgl::lv_textarea_set_text(self.path_text_area, path_text.as_ptr());
            } else {
                log::error!("Path text area is null!");
            }
        }
    }

    async fn handle_file_click(&mut self, event: &graphics::Event) -> Result<()> {
        // This runs under the graphics lock. Only live list buttons can
        // supply a file index.
        let file = unsafe { clicked_list_button(self.file_list, event) }.and_then(|button| {
            self.files
                .get(unsafe { lvgl::lv_obj_get_user_data(button) } as usize)
        });

        if let Some(file) = file {
            if file.kind == Kind::Directory {
                if let Some(new_path) = self.current_path.clone().join(Path::from_str(&file.name)) {
                    self.open(new_path).await?;
                }
            } else {
                // No application association yet: describe the file.
                self.set_status(
                    &alloc::format!("{} - {}", file.name, format_size(file.size)),
                    false,
                );
            }
        }

        Ok(())
    }

    async fn handle_up_click(&mut self) -> Result<()> {
        if let Some(parent_path) = self.current_path.go_parent() {
            self.open(parent_path.to_owned()).await?;
        }
        Ok(())
    }

    async fn handle_home_click(&mut self) -> Result<()> {
        self.open(PathOwned::root()).await
    }

    async fn handle_refresh_click(&mut self) -> Result<()> {
        self.load_directory().await?;
        Ok(())
    }

    async fn handle_go_click(&mut self) -> Result<()> {
        unsafe {
            if !self.path_text_area.is_null() {
                // Get the text from the text area
                let text_ptr: *const core::ffi::c_char =
                    lvgl::lv_textarea_get_text(self.path_text_area);
                if !text_ptr.is_null() {
                    // Convert C string to Rust string
                    let text_cstr = core::ffi::CStr::from_ptr(text_ptr);
                    if let Ok(path_str) = text_cstr.to_str() {
                        // An invalid or unreadable path is reported in the
                        // status line and the entered text is kept for editing.
                        match PathOwned::new(path_str.to_string()) {
                            Some(new_path) => self.open(new_path).await?,
                            None => self.set_status(
                                &alloc::format!(
                                    "{}",
                                    t!("errors.invalid_entered_path", arg0 = path_str)
                                ),
                                true,
                            ),
                        }
                    }
                }
            }
        }

        Ok(())
    }

    pub async fn refresh(&mut self) -> Result<()> {
        self.load_directory().await
    }

    pub async fn navigate_to(&mut self, path: &Path) -> Result<()> {
        self.open(path.to_owned()).await
    }

    pub fn get_current_path(&self) -> &Path {
        &self.current_path
    }

    pub fn is_running(&self) -> bool {
        self.running
    }
}

/// Opacity of secondary text; dimming follows the palette.
const MUTED: u8 = 150;

/// Binary-prefixed size with at most one decimal, such as `512 B` or `1.5 MiB`.
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KiB", "MiB", "GiB", "TiB"];
    if bytes < 1024 {
        return alloc::format!("{bytes} B");
    }
    // Tenths of the current unit, rounded to nearest.
    let mut tenths = (bytes as u128 * 10 + 512) / 1024;
    let mut unit = 0;
    while tenths >= 10_240 && unit + 1 < UNITS.len() {
        tenths = (tenths + 512) / 1024;
        unit += 1;
    }
    if tenths >= 1000 || tenths.is_multiple_of(10) {
        alloc::format!("{} {}", (tenths + 5) / 10, UNITS[unit])
    } else {
        alloc::format!("{}.{} {}", tenths / 10, tenths % 10, UNITS[unit])
    }
}

/// The live list button of `list` a queued click was aimed at.
///
/// # Safety
///
/// The caller holds the graphics lock and `list` is a live object.
unsafe fn clicked_list_button(
    list: *mut lvgl::lv_obj_t,
    event: &graphics::Event,
) -> Option<*mut lvgl::lv_obj_t> {
    unsafe {
        let count = lvgl::lv_obj_get_child_count(list);
        (0..count)
            .map(|index| lvgl::lv_obj_get_child(list, index as i32))
            .find(|&child| {
                event.is_target(child)
                    && lvgl::lv_obj_check_type(child, &lvgl::lv_list_button_class)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::{clicked_list_button, format_size};
    use xila::graphics::{EventKind, Window, lvgl};

    /// A double tap on a folder queues two clicks; the first rebuilds the
    /// list, and the second must not open whatever LVGL put at the old
    /// button's address.
    #[test]
    fn a_stale_click_cannot_pick_a_rebuilt_list_entry() {
        #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
        let _test_lock = xila::task::test_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        unsafe {
            lvgl::lv_init();
            lvgl::lv_display_create(320, 240);
            let screen = lvgl::lv_obj_create(core::ptr::null_mut());
            let mut window = Window::new(screen).unwrap();
            let window = window.as_mut();
            let list = lvgl::lv_list_create(window.as_object_mutable());
            let folder = lvgl::lv_list_add_button(list, core::ptr::null(), c"folder".as_ptr());
            while window.pop_event().is_some() {}
            for _ in 0..2 {
                lvgl::lv_obj_send_event(
                    folder,
                    lvgl::lv_event_code_t_LV_EVENT_CLICKED,
                    core::ptr::null_mut(),
                );
            }

            let mut clicks = core::iter::from_fn(|| window.pop_event())
                .filter(|event| event.code == EventKind::Clicked);
            let first = clicks.next().unwrap();
            assert_eq!(clicked_list_button(list, &first), Some(folder));

            // Opening the folder cleans the list and builds it again.
            lvgl::lv_obj_clean(list);
            let rebuilt = lvgl::lv_list_add_button(list, core::ptr::null(), c"entry".as_ptr());
            let second = clicks.next().unwrap();
            assert!(!second.is_target(rebuilt));
            assert_eq!(clicked_list_button(list, &second), None);
            drop(clicks);

            lvgl::lv_obj_delete(window.as_object_mutable());
        }
    }

    #[test]
    fn sizes_use_binary_prefixes() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(1023), "1023 B");
        assert_eq!(format_size(1024), "1 KiB");
        assert_eq!(format_size(1536), "1.5 KiB");
        assert_eq!(format_size(250_000), "244 KiB");
        assert_eq!(format_size(5 * 1024 * 1024), "5 MiB");
        assert_eq!(format_size(3 * 1024 * 1024 * 1024 / 2), "1.5 GiB");
    }

    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    mod setup_failure {
        use alloc::boxed::Box;
        use core::{
            task::{Context, Poll},
            time::Duration,
        };
        use std::sync::OnceLock;
        use xila::{
            file_system::{
                self, ControlCommand, ControlCommandIdentifier, DirectBaseOperations,
                DirectCharacterDevice, MountOperations, PathOwned, Size,
            },
            graphics::{self, GET_RESOLUTION, Point, SET_DRAWING_AREA, WAS_RESIZED},
            shared::AnyByLayout,
        };

        use super::super::FileManager;

        struct HeadlessScreen;
        impl MountOperations for HeadlessScreen {}
        impl DirectCharacterDevice for HeadlessScreen {}
        impl DirectBaseOperations for HeadlessScreen {
            fn read(
                &self,
                _: &mut Context<'_>,
                _: &mut [u8],
                _: Size,
            ) -> Poll<file_system::Result<usize>> {
                Poll::Ready(Err(file_system::IoErrorKind::Unsupported))
            }

            fn write(
                &self,
                _: &mut Context<'_>,
                bytes: &[u8],
                _: Size,
            ) -> Poll<file_system::Result<usize>> {
                Poll::Ready(Ok(bytes.len()))
            }

            fn control(
                &self,
                _: &mut Context<'_>,
                command: ControlCommandIdentifier,
                _: &AnyByLayout,
                output: &mut AnyByLayout,
            ) -> Poll<file_system::Result<()>> {
                Poll::Ready(match command {
                    GET_RESOLUTION::IDENTIFIER => GET_RESOLUTION::cast_output(output)
                        .map(|resolution| *resolution = Point::new(320, 240)),
                    WAS_RESIZED::IDENTIFIER => {
                        WAS_RESIZED::cast_output(output).map(|resized| *resized = false)
                    }
                    SET_DRAWING_AREA::IDENTIFIER => Ok(()),
                    _ => Err(file_system::IoErrorKind::Unsupported),
                })
            }
        }

        async fn headless_graphics_manager() -> &'static graphics::Manager {
            static MANAGER: OnceLock<&'static graphics::Manager> = OnceLock::new();
            if let Some(manager) = MANAGER.get() {
                return manager;
            }
            let time = Box::leak(Box::new(
                xila::time::Manager::new(&testing::TimeDevice).unwrap(),
            ));
            let manager: &'static graphics::Manager = Box::leak(Box::new(
                graphics::initialize_with_input_devices(time, &HeadlessScreen, &[], 1024, false)
                    .await,
            ));
            graphics::set_ffi_manager(manager);
            MANAGER.get_or_init(|| manager)
        }

        #[test]
        fn file_manager_setup_failure_deletes_window_without_deadlocking() {
            let _test_lock = xila::task::test_lock()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let (sender, receiver) = std::sync::mpsc::channel();
            let handle = std::thread::spawn(move || {
                let task_manager = xila::task::reset_test_manager();
                let executor = xila::task::instantiate_test_executor!();
                let mut spawner_id = 0;
                executor.run(|spawner, exec| {
                    spawner_id = task_manager.register_spawner(spawner).unwrap();
                    xila::task::block_on(async move {
                        task_manager
                            .spawn_on(
                                spawner,
                                xila::task::Manager::ROOT_TASK_IDENTIFIER,
                                "file_manager_setup_failure",
                                Some(spawner_id),
                                async move |_| {
                                    let standard = testing::initialize(false, false).await;
                                    let manager = headless_graphics_manager().await;
                                    let baseline_windows =
                                        manager.get_window_count().await.unwrap();
                                    let path = PathOwned::new("/does-not-exist".into()).unwrap();
                                    let result =
                                        FileManager::new_at(standard.context, manager, path).await;
                                    assert!(
                                        result.is_err(),
                                        "opening a nonexistent initial path must fail"
                                    );
                                    assert!(
                                        manager.try_lock().is_some(),
                                        "graphics lock must be released after FileManager setup error"
                                    );
                                    assert_eq!(
                                        manager.get_window_count().await.unwrap(),
                                        baseline_windows
                                    );
                                    let _ = sender.send(());
                                    exec.stop();
                                },
                            )
                            .await
                    })
                    .unwrap();
                });
                let _ = task_manager.unregister_spawner(spawner_id);
            });

            receiver
                .recv_timeout(Duration::from_secs(5))
                .unwrap_or_else(|_| {
                    // A deadlocked worker keeps the graphics lock forever, so later
                    // tests in this binary would block on it: exit instead of panicking.
                    std::eprintln!(
                        "FileManager::new_at hung (likely deadlocked on the graphics lock)"
                    );
                    std::process::exit(1)
                });
            handle.join().unwrap();
        }
    }
}

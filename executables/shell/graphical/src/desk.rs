use core::{ffi::c_void, mem::forget};

use crate::{
    error::{Error, Result, ShortcutDirectoryFailure},
    icon::{create_icon, create_show_applications_button},
    shortcut::{SHORTCUT_PATH, Shortcut},
};
use alloc::{
    collections::btree_map::BTreeMap,
    ffi::CString,
    string::{String, ToString},
    vec,
    vec::Vec,
};
use xila::graphics::{self, Color, EventKind, Logo, LogoStyle, OwnedWindow, Point, lvgl};
use xila::log::{self, error, warning};
use xila::virtual_file_system::{Directory, IoErrorKind, ignore_already_exists_error};
use xila::{executable, graphics::theme};
use xila::{executable::Standard, graphics::Event};
use xila::{file_system::Kind, graphics::symbol};

pub const WINDOWS_PARENT_CHILD_CHANGED: graphics::EventKind = graphics::EventKind::Custom2;

pub(crate) struct DeskWidgets {
    tile_view: *mut lvgl::lv_obj_t,
    desk_tile: *mut lvgl::lv_obj_t,
    drawer_tile: *mut lvgl::lv_obj_t,
    dock: *mut lvgl::lv_obj_t,
    main_button: *mut lvgl::lv_obj_t,
    logo: Option<*mut lvgl::lv_obj_t>,
}

pub struct Desk {
    context: &'static xila::executable::ExecutableContext,
    graphics_manager: &'static graphics::Manager,
    windows_parent: *mut lvgl::lv_obj_t,
    window: OwnedWindow,
    tile_view: *mut lvgl::lv_obj_t,
    drawer_tile: *mut lvgl::lv_obj_t,
    desk_tile: *mut lvgl::lv_obj_t,
    dock: *mut lvgl::lv_obj_t,
    main_button: *mut lvgl::lv_obj_t,
    /// The background logo, absent when the desk is too small for it. Only
    /// the tests read it; the desk window owns and deletes the object.
    #[cfg_attr(not(test), allow(dead_code))]
    logo: Option<*mut lvgl::lv_obj_t>,
    dock_menu: Option<*mut lvgl::lv_obj_t>,
    dock_menu_target_window: Option<usize>,
    dock_menu_maximize_button: Option<*mut lvgl::lv_obj_t>,
    dock_menu_close_button: Option<*mut lvgl::lv_obj_t>,
    shortcuts: BTreeMap<*mut lvgl::lv_obj_t, String>,
    /// The shortcut directory failure already reported through the desk
    /// event error handler. The drawer scan reruns on every drawer
    /// opening, so a given failure is not reported again; the field is
    /// cleared when the directory opens, so a failure after a recovery is
    /// reported again.
    reported_shortcut_directory_failure: Option<ShortcutDirectoryFailure>,
}

impl Drop for Desk {
    fn drop(&mut self) {
        // `Drop::drop` runs before field drops, so `self.window` is still live.
        // Detach `event_handler` from `windows_parent` before `OwnedWindow::drop`
        // deletes the desk window and emits `CHILD_DELETED` up to the screen.
        let windows_parent = self.windows_parent;
        let window = self.window.as_object_mutable();
        let _ = self.graphics_manager.lock_function_sync(|| {
            // SAFETY: the graphics lock is held, and `windows_parent` (the
            // active screen) outlives `Desk`.
            unsafe {
                lvgl::lv_obj_remove_event_cb_with_user_data(
                    windows_parent,
                    Some(event_handler),
                    window.cast(),
                );
            }
            Ok(())
        });
    }
}

unsafe extern "C" fn event_handler(event: *mut lvgl::lv_event_t) {
    unsafe {
        let raw_code = lvgl::lv_event_get_code(event);
        let code = EventKind::from_lvgl_code(raw_code);

        if code == EventKind::ChildCreated || code == EventKind::ChildDeleted {
            let target = lvgl::lv_event_get_target(event) as *mut lvgl::lv_obj_t;
            let target_parent = lvgl::lv_obj_get_parent(target);

            let current_target = lvgl::lv_event_get_current_target(event) as *mut lvgl::lv_obj_t;

            // If the event is not for the current target, ignore it (not the parent window)
            if target_parent != current_target {
                return;
            }

            let desk = lvgl::lv_event_get_user_data(event) as *mut lvgl::lv_obj_t;

            lvgl::lv_obj_send_event(
                desk,
                WINDOWS_PARENT_CHILD_CHANGED as u32,
                target as *mut c_void,
            );
        }
    }
}

impl Desk {
    const DOCK_ICON_SIZE: Point = Point::new(40, 40);
    const DRAWER_ICON_SIZE: Point = Point::new(48, 48);
    const MAIN_BUTTON_SIZE: Point = Point::new(32, 32);
    /// Width of one drawer tile: the icon plus room for a short name.
    const DRAWER_TILE_WIDTH: i32 = 88;
    const DRAWER_GAP: i32 = 8;
    const DOCK_BOTTOM_MARGIN: i32 = 14;

    /// Side padding and gap that center whole drawer columns in `width`.
    const fn drawer_grid(width: i32) -> (i32, i32) {
        let gap = Self::DRAWER_GAP;
        let step = Self::DRAWER_TILE_WIDTH + gap;
        let columns = if (width - gap) / step > 1 {
            (width - gap) / step
        } else {
            1
        };
        let used = columns * step - gap;
        let side = if width > used { (width - used) / 2 } else { 0 };
        (side, gap)
    }

    pub const HOME_EVENT: EventKind = EventKind::Custom1;

    pub fn get_window_object(&mut self) -> *mut lvgl::lv_obj_t {
        self.window.as_object_mutable()
    }

    #[cfg(test)]
    pub(crate) fn logo_object(&self) -> Option<*mut lvgl::lv_obj_t> {
        self.logo
    }

    #[cfg(test)]
    pub(crate) fn dock_object(&self) -> *mut lvgl::lv_obj_t {
        self.dock
    }

    /// Whether the dock, and with it the desk, is hidden.
    ///
    /// LVGL may update the object's flags from another executor thread, so the
    /// flag is read under the graphics lock.
    pub async fn is_hidden(&self) -> bool {
        graphics::lock!(self.graphics_manager, {
            // SAFETY: the graphics lock is held, and `dock` is owned by the
            // desk window, which lives as long as `self`.
            unsafe { lvgl::lv_obj_has_flag(self.dock, lvgl::lv_obj_flag_t_LV_OBJ_FLAG_HIDDEN) }
        })
    }

    pub async fn new(
        context: &'static xila::executable::ExecutableContext,
        windows_parent: *mut lvgl::lv_obj_t,
    ) -> Result<Self> {
        let graphics = graphics::ffi_manager();
        Self::new_with(context, graphics, windows_parent, Self::build_widgets).await
    }

    pub(crate) async fn new_with(
        context: &'static xila::executable::ExecutableContext,
        graphics: &'static graphics::Manager,
        windows_parent: *mut lvgl::lv_obj_t,
        build: impl FnOnce(&mut OwnedWindow, &mut graphics::GraphicsGuard<'_>) -> Result<DeskWidgets>,
    ) -> Result<Self> {
        let mut guard = graphics.lock().await;
        // - Create a window in a session
        let mut window = guard.with_lvgl(|lvgl| graphics.create_window(lvgl))??;

        let widgets = match build(&mut window, &mut guard) {
            Ok(widgets) => widgets,
            Err(error) => {
                // Delete the window in the current session instead of letting
                // `OwnedWindow::drop` queue it behind the held lock.
                let _ = guard.with_lvgl(|lvgl| window.delete(lvgl));
                return Err(error);
            }
        };

        // Register on `windows_parent` only once every fallible setup step
        // has succeeded, so `Desk::drop` is guaranteed to detach it.
        unsafe {
            lvgl::lv_obj_add_event_cb(
                windows_parent,
                Some(event_handler),
                EventKind::All as u32,
                window.as_object().cast(),
            );
        }

        let desk = Self {
            context,
            graphics_manager: graphics,
            windows_parent,
            window,
            tile_view: widgets.tile_view,
            desk_tile: widgets.desk_tile,
            drawer_tile: widgets.drawer_tile,
            dock: widgets.dock,
            main_button: widgets.main_button,
            logo: widgets.logo,
            dock_menu: None,
            dock_menu_target_window: None,
            dock_menu_maximize_button: None,
            dock_menu_close_button: None,
            shortcuts: BTreeMap::new(),
            reported_shortcut_directory_failure: None,
        };

        Ok(desk)
    }

    fn build_widgets(
        window: &mut OwnedWindow,
        guard: &mut graphics::GraphicsGuard<'_>,
    ) -> Result<DeskWidgets> {
        guard
            .with_lvgl(|lvgl| window.set_icon(lvgl, "De", Color::BLACK))
            .map_err(|_| Error::FailedToCreateObject)?;

        unsafe {
            // Edge-to-edge window content: the shared window class already
            // flattens border and radius, only the container padding remains.
            lvgl::lv_obj_set_style_pad_all(window.as_object_mutable(), 0, lvgl::LV_STATE_DEFAULT);
        }

        // - Create a tile view
        let tile_view = unsafe {
            let tile_view = lvgl::lv_tileview_create(window.as_object_mutable());

            if tile_view.is_null() {
                return Err(Error::FailedToCreateObject);
            }

            lvgl::lv_obj_set_style_bg_opa(tile_view, lvgl::LV_OPA_0, lvgl::LV_STATE_DEFAULT);
            lvgl::lv_obj_set_scrollbar_mode(
                tile_view,
                lvgl::lv_scrollbar_mode_t_LV_SCROLLBAR_MODE_OFF,
            );

            tile_view
        };

        // - Create the desk tile
        let desk_tile = unsafe {
            let desk = lvgl::lv_tileview_add_tile(tile_view, 0, 0, lvgl::lv_dir_t_LV_DIR_BOTTOM);

            if desk.is_null() {
                return Err(Error::FailedToCreateObject);
            }

            desk
        };

        // - Create the drawer tile
        let drawer_tile = unsafe {
            let drawer = lvgl::lv_tileview_add_tile(tile_view, 0, 1, lvgl::lv_dir_t_LV_DIR_TOP);

            if drawer.is_null() {
                return Err(Error::FailedToCreateObject);
            }

            lvgl::lv_obj_set_flex_flow(drawer, lvgl::lv_flex_flow_t_LV_FLEX_FLOW_ROW_WRAP);
            // Side padding centers as many whole columns as fit, so the
            // last row stays aligned with the others.
            lvgl::lv_obj_update_layout(tile_view);
            let (side, gap) = Self::drawer_grid(lvgl::lv_obj_get_width(tile_view));
            lvgl::lv_obj_set_style_pad_hor(drawer, side, lvgl::LV_STATE_DEFAULT);
            lvgl::lv_obj_set_style_pad_ver(drawer, 16, lvgl::LV_STATE_DEFAULT);
            lvgl::lv_obj_set_style_pad_column(drawer, gap, lvgl::LV_STATE_DEFAULT);
            lvgl::lv_obj_set_style_pad_row(drawer, gap, lvgl::LV_STATE_DEFAULT);

            drawer
        };

        // - Create a dock
        let dock = unsafe {
            let dock = lvgl::lv_obj_create(desk_tile);

            if dock.is_null() {
                return Err(Error::FailedToCreateObject);
            }

            theme::set_surface(dock, theme::Surface::Muted);

            lvgl::lv_obj_set_align(dock, lvgl::lv_align_t_LV_ALIGN_BOTTOM_MID);
            // Float above the home indicator instead of sitting under it.
            lvgl::lv_obj_set_y(dock, -Self::DOCK_BOTTOM_MARGIN);
            lvgl::lv_obj_set_size(dock, lvgl::LV_SIZE_CONTENT, lvgl::LV_SIZE_CONTENT);
            lvgl::lv_obj_set_flex_flow(dock, lvgl::lv_flex_flow_t_LV_FLEX_FLOW_ROW);
            // Floating dock bar: the theme container style supplies the card
            // padding, radius, border and icon gap; only the surface is set.
            // Center items so the smaller main button aligns with the icons.
            lvgl::lv_obj_set_flex_align(
                dock,
                lvgl::lv_flex_align_t_LV_FLEX_ALIGN_CENTER,
                lvgl::lv_flex_align_t_LV_FLEX_ALIGN_CENTER,
                lvgl::lv_flex_align_t_LV_FLEX_ALIGN_CENTER,
            );

            dock
        };

        // - Create the main button
        let main_button = unsafe { create_show_applications_button(dock, Self::MAIN_BUTTON_SIZE)? };

        // - Create the logo once the dock is laid out, so it fits above it.
        let logo = unsafe { Self::create_background_logo(window.as_object_mutable(), dock)? };

        Ok(DeskWidgets {
            tile_view,
            desk_tile,
            drawer_tile,
            dock,
            main_button,
            logo,
        })
    }

    /// Largest logo scale factor, the 128 px logo of large displays.
    const LOGO_MAXIMUM_FACTOR: u8 = 4;
    /// Space kept between the logo and the edges of the free area.
    const LOGO_MARGIN: i32 = 8;

    /// Scale factor and top offset of a logo centered in the area of
    /// `width` by `height` pixels above `dock_reserved` pixels of dock, or
    /// `None` when not even the smallest logo fits.
    fn background_logo_placement(width: i32, height: i32, dock_reserved: i32) -> Option<(u8, i32)> {
        let free_height = height - dock_reserved;
        let available = Point::new(
            (width - 2 * Self::LOGO_MARGIN).max(0) as i16,
            (free_height - 2 * Self::LOGO_MARGIN).clamp(0, i16::MAX as i32) as i16,
        );
        let factor = Logo::get_factor(available).min(Self::LOGO_MAXIMUM_FACTOR);
        if factor == 0 {
            return None;
        }
        let size = Logo::BASE_SIZE.get_y() as i32 * factor as i32;
        Some((factor, (free_height - size) / 2))
    }

    /// Outline the logo in the background of the desk window, in the
    /// palette's input color like the outline controls, sized to the space
    /// above the dock so it never overlaps the dock on compact displays.
    ///
    /// # Safety
    /// The caller must hold the graphics lock, and `window` and `dock` must be
    /// live, with `dock` inside `window`.
    unsafe fn create_background_logo(
        window: *mut lvgl::lv_obj_t,
        dock: *mut lvgl::lv_obj_t,
    ) -> Result<Option<*mut lvgl::lv_obj_t>> {
        unsafe {
            lvgl::lv_obj_update_layout(window);
            // The dock grows to the application icons once windows open.
            let dock_height = lvgl::lv_obj_get_height(dock).max(
                Self::DOCK_ICON_SIZE.get_y() as i32
                    + lvgl::lv_obj_get_style_pad_top(dock, lvgl::LV_PART_MAIN)
                    + lvgl::lv_obj_get_style_pad_bottom(dock, lvgl::LV_PART_MAIN)
                    + 2 * lvgl::lv_obj_get_style_border_width(dock, lvgl::LV_PART_MAIN),
            );
            let Some((factor, top)) = Self::background_logo_placement(
                lvgl::lv_obj_get_content_width(window),
                lvgl::lv_obj_get_content_height(window),
                dock_height + Self::DOCK_BOTTOM_MARGIN,
            ) else {
                return Ok(None);
            };

            let logo = Logo::new_with_style(window, factor, Color::BLACK, LogoStyle::Outline)?;
            logo.follow_palette_color(theme::PaletteColor::Input);
            let logo_inner_object = logo.get_inner_object();
            forget(logo); // Prevent the logo from being dropped

            lvgl::lv_obj_align(logo_inner_object, lvgl::lv_align_t_LV_ALIGN_TOP_MID, 0, top);
            lvgl::lv_obj_add_flag(
                logo_inner_object,
                lvgl::lv_obj_flag_t_LV_OBJ_FLAG_OVERFLOW_VISIBLE,
            );
            // Stay behind the tile view, which holds the dock and the drawer.
            lvgl::lv_obj_move_to_index(logo_inner_object, 0);

            Ok(Some(logo_inner_object))
        }
    }

    unsafe fn create_drawer_shortcut(
        &mut self,
        entry_name: &str,
        name: &str,
        icon_color: Color,
        icon_string: &str,
        drawer: *mut lvgl::lv_obj_t,
    ) -> Result<()> {
        let icon = unsafe {
            // Every tile has the same width, so the drawer reads as a grid:
            // the icon over its name, with no frame of its own.
            let container = lvgl::lv_obj_create(drawer);

            if container.is_null() {
                return Err(Error::FailedToCreateObject);
            }

            lvgl::lv_obj_set_size(container, Self::DRAWER_TILE_WIDTH, lvgl::LV_SIZE_CONTENT);
            lvgl::lv_obj_set_style_bg_opa(container, lvgl::LV_OPA_0, lvgl::LV_STATE_DEFAULT);
            lvgl::lv_obj_set_style_border_width(container, 0, lvgl::LV_STATE_DEFAULT);
            lvgl::lv_obj_set_style_pad_all(container, 4, lvgl::LV_STATE_DEFAULT);
            lvgl::lv_obj_set_style_pad_row(container, 6, lvgl::LV_STATE_DEFAULT);
            lvgl::lv_obj_remove_flag(container, lvgl::lv_obj_flag_t_LV_OBJ_FLAG_SCROLLABLE);
            lvgl::lv_obj_set_flex_flow(container, lvgl::lv_flex_flow_t_LV_FLEX_FLOW_COLUMN);
            lvgl::lv_obj_set_flex_align(
                container,
                lvgl::lv_flex_align_t_LV_FLEX_ALIGN_START,
                lvgl::lv_flex_align_t_LV_FLEX_ALIGN_CENTER,
                lvgl::lv_flex_align_t_LV_FLEX_ALIGN_CENTER,
            );

            let icon = create_icon(container, icon_color, icon_string, Self::DRAWER_ICON_SIZE)?;

            let label = lvgl::lv_label_create(container);

            if label.is_null() {
                return Err(Error::FailedToCreateObject);
            }

            let name = CString::new(name).map_err(Error::NullCharacterInString)?;

            lvgl::lv_label_set_text(label, name.as_ptr());
            // Long names end with dots instead of widening the tile.
            lvgl::lv_obj_set_width(label, lvgl::lv_pct(100));
            lvgl::lv_label_set_long_mode(label, lvgl::lv_label_long_mode_t_LV_LABEL_LONG_MODE_DOTS);
            lvgl::lv_obj_set_style_text_align(
                label,
                lvgl::lv_text_align_t_LV_TEXT_ALIGN_CENTER,
                lvgl::LV_STATE_DEFAULT,
            );
            lvgl::lv_obj_set_style_text_font(
                label,
                xila::graphics::fonts::get_font_small(),
                lvgl::LV_STATE_DEFAULT,
            );

            icon
        };

        self.shortcuts.insert(icon, entry_name.to_string());

        Ok(())
    }

    async unsafe fn create_drawer_interface(&mut self, drawer: *mut lvgl::lv_obj_t) -> Result<()> {
        unsafe {
            let task = self
                .context
                .task_manager
                .get_current_task_identifier()
                .await;

            let virtual_file_system = &self.context.virtual_file_system;

            // Missing parents are not created on purpose, the seed image
            // provides the shared configuration tree. The opening attempt
            // stays so that the drawer can still load when the directory is
            // readable even though its creation failed.
            let creation = virtual_file_system
                .create_directory(task, &SHORTCUT_PATH)
                .await;
            let opening = Directory::open(virtual_file_system, task, SHORTCUT_PATH).await;

            let mut buffer: Vec<u8> = vec![];

            let shortcuts_directory = match classify_shortcut_directory_scan(creation, opening) {
                Ok(directory) => {
                    // The drawer can still load, so a creation failure that
                    // did not prevent opening is not reported, and a later
                    // failure is reported again.
                    self.reported_shortcut_directory_failure = None;
                    directory
                }
                Err(failure) => {
                    // The scan reruns on every drawer opening: report a given
                    // failure only once. The caller routes the error through
                    // the desk event error handler, which reports both causes
                    // of the failure in one message.
                    if !is_new_shortcut_directory_failure(
                        self.reported_shortcut_directory_failure,
                        failure,
                    ) {
                        return Ok(());
                    }
                    self.reported_shortcut_directory_failure = Some(failure);
                    return Err(Error::FailedToReadShortcutDirectory(failure));
                }
            };

            for shortcut_entry in shortcuts_directory {
                if shortcut_entry.kind != Kind::File {
                    continue;
                }

                if !shortcut_entry.name.ends_with(".json") {
                    continue;
                }

                match Shortcut::read(self.context, &shortcut_entry.name, &mut buffer).await {
                    Ok(shortcut) => {
                        self.create_drawer_shortcut(
                            &shortcut_entry.name,
                            &shortcut.name,
                            shortcut.get_icon_color(),
                            &shortcut.icon_string,
                            drawer,
                        )?;
                    }
                    Err(e) => {
                        error!("Failed to read shortcut {}: {e:?}", shortcut_entry.name);
                        continue;
                    }
                }
            }

            // Shell presentation is separate from signed package metadata:
            // read bundle-local shortcut.json files without parsing a package manifest.
            // A fresh scan on drawer opening makes new apps visible without reboot.
            let bundles_path = xila::file_system::Path::APPLICATION_BUNDLES;
            match Directory::open(virtual_file_system, task, bundles_path).await {
                Ok(bundles) => {
                    for bundle in bundles {
                        // Skip ".", "..", and installer staging such as ".incoming-1".
                        if bundle.kind != Kind::Directory || bundle.name.starts_with('.') {
                            continue;
                        }
                        let Some(directory) = bundles_path.append(&bundle.name) else {
                            continue;
                        };
                        let Some(shortcut_path) = directory.append("shortcut.json") else {
                            continue;
                        };
                        match Shortcut::read_from_path(self.context, &shortcut_path, &mut buffer)
                            .await
                        {
                            Ok(shortcut) => {
                                self.create_drawer_shortcut(
                                    shortcut_path.as_str(),
                                    &shortcut.name,
                                    shortcut.get_icon_color(),
                                    &shortcut.icon_string,
                                    drawer,
                                )?;
                            }
                            Err(error) => {
                                warning!(
                                    "Skipping bundle shortcut at {}: {error:?}",
                                    shortcut_path.as_str()
                                );
                            }
                        }
                    }
                }
                // A fresh image has no bundle directory yet: nothing to scan.
                Err(IoErrorKind::NotFound) => {}
                Err(error) => {
                    warning!(
                        "Failed to open the application bundles directory {}: {error:?}",
                        bundles_path.as_str()
                    );
                }
            }

            Ok(())
        }
    }

    async fn execute_shortcut(&mut self, shortcut_name: &str) -> Result<()> {
        let task = self
            .context
            .task_manager
            .get_current_task_identifier()
            .await;

        let mut buffer = vec![];

        let mut shortcut = if shortcut_name.starts_with('/') {
            Shortcut::read_from_path(
                self.context,
                xila::file_system::Path::from_str(shortcut_name),
                &mut buffer,
            )
            .await?
        } else {
            Shortcut::read(self.context, shortcut_name, &mut buffer).await?
        };
        if shortcut_name.starts_with('/') {
            let bundle = xila::file_system::Path::from_str(shortcut_name)
                .go_parent()
                .ok_or(Error::FailedToGetShortcutFilePath)?;
            shortcut.resolve_bundle_paths(bundle)?;
        }

        let standard = Standard::open(
            &"/devices/standard_io/null",
            &"/devices/standard_io/null",
            &"/devices/standard_io/null",
            task,
            self.context.virtual_file_system,
            self.context,
        )
        .await
        .map_err(Error::FailedToOpenStandardFile)?;

        executable::execute(
            self.context,
            &*shortcut.command,
            shortcut.arguments,
            standard,
            None,
        )
        .await
        .map_err(Error::FailedToExecuteShortcut)?;

        Ok(())
    }

    unsafe fn open_dock_menu(
        &mut self,
        _dock_icon: *mut lvgl::lv_obj_t,
        window_identifier: usize,
    ) -> Result<()> {
        unsafe {
            self.close_dock_menu();

            let menu = lvgl::lv_list_create(self.window.as_object_mutable());

            if menu.is_null() {
                return Err(Error::FailedToCreateObject);
            }

            lvgl::lv_obj_set_size(menu, 160, lvgl::LV_SIZE_CONTENT);

            lvgl::lv_obj_align_to(
                menu,
                _dock_icon,
                lvgl::lv_align_t_LV_ALIGN_OUT_TOP_MID,
                0,
                0,
            );
            lvgl::lv_obj_set_scrollbar_mode(menu, lvgl::lv_scrollbar_mode_t_LV_SCROLLBAR_MODE_OFF);

            let maximize_button =
                lvgl::lv_list_add_button(menu, symbol::UP.as_ptr() as _, c"Maximize".as_ptr());

            if maximize_button.is_null() {
                lvgl::lv_obj_delete_async(menu);
                return Err(Error::FailedToCreateObject);
            }

            lvgl::lv_obj_set_width(maximize_button, lvgl::lv_pct(100));

            let close_button =
                lvgl::lv_list_add_button(menu, symbol::CLOSE.as_ptr() as _, c"Close".as_ptr());

            if close_button.is_null() {
                lvgl::lv_obj_delete_async(menu);
                return Err(Error::FailedToCreateObject);
            }

            lvgl::lv_obj_set_width(close_button, lvgl::lv_pct(100));

            self.dock_menu_maximize_button = Some(maximize_button);
            self.dock_menu = Some(menu);
            self.dock_menu_close_button = Some(close_button);
            self.dock_menu_target_window = Some(window_identifier);

            Ok(())
        }
    }

    unsafe fn close_dock_menu(&mut self) {
        unsafe {
            if let Some(menu) = self.dock_menu.take() {
                lvgl::lv_obj_delete_async(menu);
            }

            self.dock_menu_target_window = None;
            self.dock_menu_maximize_button = None;
            self.dock_menu_close_button = None;
        }
    }

    async fn close_app_for_window(
        &mut self,
        window_identifier: usize,
        guard: &mut graphics::GraphicsGuard<'_>,
    ) {
        if let Err(error) = guard
            .with_lvgl(|lvgl| {
                self.graphics_manager
                    .send_window_close_request(lvgl, window_identifier)
            })
            .flatten()
        {
            warning!(
                "Failed to send close request to window {}: {:?}",
                window_identifier,
                error
            );
        }
    }

    unsafe fn clear_main_button_pressed_state(&self) {
        unsafe {
            const STATE: lvgl::lv_state_t = lvgl::LV_STATE_PRESSED;

            lvgl::lv_obj_remove_state(self.main_button, STATE);
        }
    }

    unsafe fn is_object_inside(parent: *mut lvgl::lv_obj_t, object: *mut lvgl::lv_obj_t) -> bool {
        unsafe {
            let mut current = object;

            while !current.is_null() {
                if current == parent {
                    return true;
                }

                current = lvgl::lv_obj_get_parent(current);
            }

            false
        }
    }

    fn find_event_target(&mut self, event: &Event) -> Option<core::ptr::NonNull<lvgl::lv_obj_t>> {
        type Search<'a> = (&'a Event, *mut lvgl::lv_obj_t);
        unsafe extern "C" fn visit(
            object: *mut lvgl::lv_obj_t,
            data: *mut core::ffi::c_void,
        ) -> lvgl::lv_obj_tree_walk_res_t {
            let (event, found) = unsafe { &mut *data.cast::<Search<'_>>() };
            if event.is_target(object) {
                *found = object;
                lvgl::lv_obj_tree_walk_res_t_LV_OBJ_TREE_WALK_END
            } else {
                lvgl::lv_obj_tree_walk_res_t_LV_OBJ_TREE_WALK_NEXT
            }
        }

        let mut search: Search<'_> = (event, core::ptr::null_mut());
        // SAFETY: the graphics lock keeps the window tree live during the walk.
        unsafe {
            lvgl::lv_obj_tree_walk(
                self.window.as_object_mutable(),
                Some(visit),
                (&raw mut search).cast(),
            );
        }
        core::ptr::NonNull::new(search.1)
    }

    async fn get_visible_windows(
        &self,
        guard: &mut graphics::GraphicsGuard<'_>,
    ) -> Result<Vec<(usize, String, Color)>> {
        let graphics_manager = self.graphics_manager;
        // Enumerate the windows in one session.
        guard.with_lvgl(|lvgl| {
            let window_count = graphics_manager.get_window_count(lvgl)?;

            let mut windows = Vec::new();

            for index in 0..window_count {
                let Ok(window_identifier) = graphics_manager.get_window_identifier(lvgl, index)
                else {
                    continue;
                };

                if window_identifier == self.window.get_identifier() {
                    continue;
                }

                match graphics_manager.get_window_icon(lvgl, index) {
                    Ok((icon_string, icon_color)) => {
                        windows.push((window_identifier, icon_string, icon_color));
                    }
                    Err(error) => {
                        warning!(
                            "Failed to fetch icon for window {} during dock rebuild: {:?}",
                            window_identifier,
                            error
                        );
                    }
                }
            }

            Ok(windows)
        })?
    }

    unsafe fn clear_dock_icons(&self) {
        unsafe {
            let mut child_index = lvgl::lv_obj_get_child_count(self.dock) as i32;

            while child_index > 0 {
                child_index -= 1;

                let child = lvgl::lv_obj_get_child(self.dock, child_index);

                if child.is_null() || child == self.main_button {
                    continue;
                }

                lvgl::lv_obj_delete(child);
            }
        }
    }

    unsafe fn rebuild_dock_icons(&self, windows: &[(usize, String, Color)]) -> Result<()> {
        unsafe {
            for (window_identifier, icon_string, icon_color) in windows {
                let icon = create_icon(self.dock, *icon_color, icon_string, Self::DOCK_ICON_SIZE)?;

                lvgl::lv_obj_set_user_data(icon, *window_identifier as *mut c_void);
            }
        }

        Ok(())
    }

    // This function is intentionally private and is only used within this module.
    async fn refresh_dock(&mut self, guard: &mut graphics::GraphicsGuard<'_>) -> Result<()> {
        let visible_windows = self.get_visible_windows(guard).await?;

        unsafe {
            self.clear_dock_icons();
            self.rebuild_dock_icons(&visible_windows)?;
        }

        if self
            .dock_menu_target_window
            .is_some_and(|window_identifier| {
                !visible_windows
                    .iter()
                    .any(|(visible_identifier, _, _)| *visible_identifier == window_identifier)
            })
        {
            unsafe {
                self.close_dock_menu();
            }
        }

        Ok(())
    }

    pub async fn handle_events(&mut self) -> bool {
        let mut guard = self.graphics_manager.lock().await;
        // Take each event in a short session; the guard stays held across the
        // awaited handlers below.
        while let Some(event) = guard
            .with_lvgl(|lvgl| self.window.pop_event(lvgl))
            .ok()
            .flatten()
        {
            if let Err(error) = self.handle_event(event, &mut guard).await {
                log::error!("Failed to handle desk event: {error:?}");
            }
        }

        true
    }

    pub async fn handle_event(
        &mut self,
        event: Event,
        guard: &mut graphics::GraphicsGuard<'_>,
    ) -> Result<()> {
        // Resolve pointer events against the live window tree before
        // inspecting their widgets. The graphics lock is held by the caller.
        let target = match event.code {
            EventKind::Clicked
            | EventKind::LongPressed
            | EventKind::Pressed
            | EventKind::Released => {
                let Some(target) = self.find_event_target(&event) else {
                    return Ok(());
                };
                target.as_ptr()
            }
            _ => core::ptr::null_mut(),
        };

        match event.code {
            Self::HOME_EVENT => unsafe {
                lvgl::lv_tileview_set_tile_by_index(self.tile_view, 0, 0, true);
            },
            EventKind::ValueChanged if event.is_target(self.tile_view) => unsafe {
                if lvgl::lv_tileview_get_tile_active(self.tile_view) == self.desk_tile {
                    lvgl::lv_obj_clean(self.drawer_tile);
                } else if lvgl::lv_obj_get_child_count(self.drawer_tile) == 0 {
                    // Drawer scan failures, including shortcut rendering
                    // failures, are routed through the desk event error
                    // handler in `handle_events`.
                    self.create_drawer_interface(self.drawer_tile).await?;
                }
            },
            EventKind::Clicked => {
                if self
                    .dock_menu_maximize_button
                    .is_some_and(|maximize_button| {
                        target == maximize_button
                            || unsafe { lvgl::lv_obj_get_parent(target) == maximize_button }
                    })
                {
                    if let Some(window_identifier) = self.dock_menu_target_window
                        && let Err(error) = guard
                            .with_lvgl(|lvgl| {
                                self.graphics_manager
                                    .maximize_window(lvgl, window_identifier)
                            })
                            .flatten()
                    {
                        warning!(
                            "Failed to maximize window {} from dock menu: {:?}",
                            window_identifier,
                            error
                        );
                    }

                    unsafe {
                        self.close_dock_menu();
                    }

                    return Ok(());
                }

                if self.dock_menu_close_button.is_some_and(|close_button| {
                    target == close_button
                        || unsafe { lvgl::lv_obj_get_parent(target) == close_button }
                }) {
                    if let Some(window_identifier) = self.dock_menu_target_window {
                        self.close_app_for_window(window_identifier, guard).await;
                    }

                    unsafe {
                        self.close_dock_menu();
                    }

                    return Ok(());
                }

                if self.dock_menu.is_some()
                    && unsafe { lvgl::lv_obj_get_parent(target) == self.dock }
                    && target != self.main_button
                {
                    let clicked_window_identifier =
                        unsafe { lvgl::lv_obj_get_user_data(target) as usize };

                    if self
                        .dock_menu_target_window
                        .is_some_and(|window_identifier| {
                            window_identifier == clicked_window_identifier
                        })
                    {
                        return Ok(());
                    }
                }

                if let Some(menu) = self.dock_menu
                    && !unsafe { Self::is_object_inside(menu, target) }
                {
                    unsafe {
                        self.close_dock_menu();
                    }
                }

                // If the target is a shortcut, execute the shortcut
                if let Some(shortcut_name) = self.shortcuts.get(&target).cloned() {
                    self.execute_shortcut(&shortcut_name).await?;
                }
                // If the target is a dock icon, move the window to the foreground
                else if unsafe { lvgl::lv_obj_get_parent(target) == self.dock } {
                    // Ignore the main button
                    if target == self.main_button {
                        return Ok(());
                    }

                    unsafe {
                        self.close_dock_menu();
                    }

                    let window_identifier = unsafe { lvgl::lv_obj_get_user_data(target) as usize };

                    if let Err(error) = guard
                        .with_lvgl(|lvgl| {
                            self.graphics_manager
                                .maximize_window(lvgl, window_identifier)
                        })
                        .flatten()
                    {
                        warning!(
                            "Failed to maximize window {} from dock click: {:?}",
                            window_identifier,
                            error
                        );
                    }
                }
            }
            EventKind::LongPressed
                if unsafe { lvgl::lv_obj_get_parent(target) == self.dock }
                    && target != self.main_button =>
            {
                let window_identifier = unsafe { lvgl::lv_obj_get_user_data(target) as usize };

                unsafe {
                    self.open_dock_menu(target, window_identifier)?;
                }
            }
            EventKind::Pressed
                if (target == self.main_button
                    || unsafe { lvgl::lv_obj_get_parent(target) == self.main_button }) =>
            unsafe {
                lvgl::lv_obj_add_state(self.main_button, lvgl::LV_STATE_PRESSED);
            },
            EventKind::Released
                if (target == self.main_button
                    || unsafe { lvgl::lv_obj_get_parent(target) == self.main_button }) =>
            {
                unsafe {
                    self.clear_main_button_pressed_state();
                }

                unsafe {
                    lvgl::lv_tileview_set_tile_by_index(self.tile_view, 0, 1, true);
                }
            }
            EventKind::PressLost => unsafe {
                self.clear_main_button_pressed_state();
            },
            WINDOWS_PARENT_CHILD_CHANGED => {
                // Ignore consecutive windows parent child changed events
                let pending = guard
                    .with_lvgl(|lvgl| self.window.peek_event(lvgl))
                    .unwrap_or(None);
                if let Some(Event {
                    code: WINDOWS_PARENT_CHILD_CHANGED,
                    ..
                }) = pending
                {
                    return Ok(());
                }

                if let Err(error) = self.refresh_dock(guard).await {
                    warning!(
                        "Failed to refresh dock after window tree update: {:?}",
                        error
                    );
                }
            }
            _ => {}
        }

        Ok(())
    }
}

/// Classify one shortcut directory scan from its creation and opening
/// results. Only `AlreadyExists` on creation is expected; any other
/// creation error is kept so that, when opening the directory fails too,
/// both causes are reported together. A creation failure followed by a
/// successful opening is not a failure at all: the drawer can still load.
fn classify_shortcut_directory_scan<T>(
    creation: core::result::Result<(), IoErrorKind>,
    opening: core::result::Result<T, IoErrorKind>,
) -> core::result::Result<T, ShortcutDirectoryFailure> {
    let creation = ignore_already_exists_error(creation).err();
    match opening {
        Ok(directory) => Ok(directory),
        Err(opening) => Err(ShortcutDirectoryFailure { creation, opening }),
    }
}

/// Whether a shortcut directory failure still needs to be reported given the
/// last reported one: the drawer scan reruns on every drawer opening, so the
/// same failure is not repeated, but a change of cause is, and so is a
/// failure after the directory opened again (`None`).
fn is_new_shortcut_directory_failure(
    last_reported: Option<ShortcutDirectoryFailure>,
    failure: ShortcutDirectoryFailure,
) -> bool {
    last_reported != Some(failure)
}

#[cfg(test)]
mod tests {
    use alloc::{string::String, vec, vec::Vec};

    use xila::graphics::Color;
    use xila::virtual_file_system::IoErrorKind;

    fn visible_window_identifiers(windows: &[(usize, String, Color)]) -> Vec<usize> {
        windows
            .iter()
            .map(|(window_identifier, _, _)| *window_identifier)
            .collect()
    }

    #[test]
    fn background_logo_fits_above_the_dock() {
        use super::Desk;

        for (width, height, dock_reserved) in [
            (320, 216, 70),
            (320, 216, 90),
            (240, 296, 70),
            (800, 456, 70),
        ] {
            let (factor, top) = Desk::background_logo_placement(width, height, dock_reserved)
                .expect("the logo fits");
            let size = 32 * factor as i32;
            assert!(top >= Desk::LOGO_MARGIN, "{width}x{height}: top margin");
            assert!(
                top + size <= height - dock_reserved - Desk::LOGO_MARGIN,
                "{width}x{height}: the logo overlaps the dock"
            );
            assert!(
                size <= width - 2 * Desk::LOGO_MARGIN,
                "{width}x{height}: width"
            );
        }
        // The 128 px logo is the largest, and it shrinks on compact displays.
        assert_eq!(Desk::background_logo_placement(800, 456, 70).unwrap().0, 4);
        assert_eq!(Desk::background_logo_placement(320, 216, 90).unwrap().0, 3);
        assert_eq!(Desk::background_logo_placement(160, 216, 90).unwrap().0, 3);
        assert_eq!(Desk::background_logo_placement(90, 216, 90).unwrap().0, 2);
        // No room at all: no logo rather than an overlap.
        assert_eq!(Desk::background_logo_placement(320, 100, 70), None);
        assert_eq!(Desk::background_logo_placement(10, 216, 70), None);
    }

    #[test]
    fn drawer_columns_are_centered() {
        // 800 px: eight 88 px tiles with 8 px gaps use 760 px, 20 px a side.
        assert_eq!(super::Desk::drawer_grid(800), (20, 8));
        // 320 px: three tiles.
        assert_eq!(super::Desk::drawer_grid(320), (20, 8));
        // Narrower than one tile: a single column without negative padding.
        assert_eq!(super::Desk::drawer_grid(60), (0, 8));
    }

    #[test]
    fn preserves_original_order_of_non_desk_windows() {
        let windows = vec![
            (42, String::from("A"), Color::BLACK),
            (77, String::from("B"), Color::WHITE),
            (3, String::from("C"), Color::new(12, 34, 56)),
        ];

        assert_eq!(visible_window_identifiers(&windows), vec![42, 77, 3]);
    }

    #[test]
    fn shortcut_directory_scan_reports_at_most_one_failure_with_both_causes() {
        use super::{ShortcutDirectoryFailure, classify_shortcut_directory_scan};

        // The directory opens: nothing to report, even when the creation
        // reported `AlreadyExists` or failed while the directory stayed
        // readable, because the drawer can still load.
        assert!(classify_shortcut_directory_scan(Ok(()), Ok(())).is_ok());
        assert!(classify_shortcut_directory_scan(Err(IoErrorKind::AlreadyExists), Ok(())).is_ok());
        assert!(
            classify_shortcut_directory_scan(Err(IoErrorKind::PermissionDenied), Ok(())).is_ok()
        );

        // The directory cannot be opened: one failure carrying both causes.
        assert_eq!(
            classify_shortcut_directory_scan::<()>(
                Err(IoErrorKind::PermissionDenied),
                Err(IoErrorKind::NotFound)
            )
            .unwrap_err(),
            ShortcutDirectoryFailure {
                creation: Some(IoErrorKind::PermissionDenied),
                opening: IoErrorKind::NotFound,
            }
        );
        assert_eq!(
            classify_shortcut_directory_scan::<()>(Ok(()), Err(IoErrorKind::PermissionDenied))
                .unwrap_err(),
            ShortcutDirectoryFailure {
                creation: None,
                opening: IoErrorKind::PermissionDenied,
            }
        );
    }

    #[test]
    fn a_given_shortcut_directory_failure_is_reported_once() {
        use super::{ShortcutDirectoryFailure, is_new_shortcut_directory_failure};

        let failure = ShortcutDirectoryFailure {
            creation: Some(IoErrorKind::PermissionDenied),
            opening: IoErrorKind::PermissionDenied,
        };
        // The first failure, and a failure after a recovery, are reported.
        assert!(is_new_shortcut_directory_failure(None, failure));
        // The same failure is not repeated on the next drawer opening.
        assert!(!is_new_shortcut_directory_failure(Some(failure), failure));
        // A change of cause is reported again.
        let other = ShortcutDirectoryFailure {
            creation: None,
            opening: IoErrorKind::NotFound,
        };
        assert!(is_new_shortcut_directory_failure(Some(failure), other));
    }

    #[test]
    fn allows_detecting_disappeared_menu_target() {
        let windows = vec![
            (2, String::from("A"), Color::BLACK),
            (3, String::from("B"), Color::WHITE),
        ];

        let visible = visible_window_identifiers(&windows);

        assert!(!visible.contains(&99));
    }
}

use crate::error::{Error, Result};
use alloc::{collections::vec_deque::VecDeque, string::String};
use core::ffi::CStr;
use xila::graphics::OwnedWindow;
use xila::graphics::fonts::get_font_monospace_medium;
use xila::graphics::{
    self, Color, EventKind, Key,
    lvgl::{self, lv_obj_t},
};
use xila::internationalization::t;
use xila::synchronization::{blocking_mutex::raw::CriticalSectionRawMutex, rwlock::RwLock};

pub(crate) struct Inner {
    window: OwnedWindow,
    buffer: String,
    display: *mut lvgl::lv_obj_t,
    input: *mut lvgl::lv_obj_t,
    send: *mut lvgl::lv_obj_t,
    clear: *mut lvgl::lv_obj_t,
    validated_input: VecDeque<u8>,
}

pub struct Terminal {
    pub(crate) input_waiters: xila::file_system::WaitQueue<16>,
    pub(crate) input_closed: core::sync::atomic::AtomicBool,
    inner: RwLock<CriticalSectionRawMutex, Inner>,
    graphics_manager: &'static graphics::Manager,
}

unsafe impl Send for Terminal {}

unsafe impl Sync for Terminal {}

impl Terminal {
    const CLEAR: &'static str = "\x1B[2J";
    const HOME: &'static str = "\x1B[H";

    pub async fn new(graphics_manager: &'static graphics::Manager) -> Result<Self> {
        let inner = graphics::lock!(graphics_manager, {
            let mut window = graphics_manager.create_window().await?;
            unsafe {
                window.set_icon(">_", Color::BLACK);

                lvgl::lv_obj_set_flex_flow(
                    window.as_object_mutable(),
                    lvgl::lv_flex_flow_t_LV_FLEX_FLOW_COLUMN,
                );
                // Edge-to-edge window chrome: the shared window class already
                // flattens border and radius, only the container padding remains.
                lvgl::lv_obj_set_style_pad_all(
                    window.as_object_mutable(),
                    0,
                    lvgl::LV_STATE_DEFAULT,
                );
            }

            let compact = unsafe {
                lvgl::lv_obj_update_layout(window.as_object_mutable());
                lvgl::lv_obj_set_style_pad_row(
                    window.as_object_mutable(),
                    0,
                    lvgl::LV_STATE_DEFAULT,
                );
                lvgl::lv_obj_get_width(window.as_object_mutable()) < 560
            };
            let spacing = if compact { 6 } else { 10 };

            let container = unsafe {
                let container = lvgl::lv_obj_create(window.as_object_mutable());

                lvgl::lv_obj_set_width(container, lvgl::lv_pct(100));
                lvgl::lv_obj_set_flex_grow(container, 1);
                lvgl::lv_obj_set_flex_flow(container, lvgl::lv_flex_flow_t_LV_FLEX_FLOW_COLUMN);
                // The output sits on the darker background surface, inset from
                // the window edges, without the card border and radius.
                graphics::theme::set_surface(container, graphics::theme::Surface::Background);
                lvgl::lv_obj_set_style_pad_all(container, spacing, lvgl::LV_STATE_DEFAULT);
                lvgl::lv_obj_set_style_border_width(container, 0, lvgl::LV_STATE_DEFAULT);
                lvgl::lv_obj_set_style_radius(container, 0, lvgl::LV_STATE_DEFAULT);
                lvgl::lv_obj_set_scroll_snap_y(
                    container,
                    lvgl::lv_scroll_snap_t_LV_SCROLL_SNAP_END,
                );

                container
            };

            let mut buffer = String::with_capacity(80 * 24);
            buffer.push('\0'); // Initialize with null terminator for LVGL

            let display = unsafe {
                let label = lvgl::lv_label_create(container);

                if label.is_null() {
                    return Err(crate::error::Error::FailedToCreateObject);
                }

                lvgl::lv_obj_set_width(label, lvgl::lv_pct(100));
                lvgl::lv_label_set_text_static(label, buffer.as_ptr() as *const core::ffi::c_char);
                lvgl::lv_obj_set_style_text_font(
                    label,
                    get_font_monospace_medium(),
                    lvgl::LV_STATE_DEFAULT,
                );

                label
            };

            // Command bar: the field, then Clear and the primary Send action.
            let (input, send, clear) = unsafe {
                let bar = lvgl::lv_obj_create(window.as_object_mutable());
                if bar.is_null() {
                    return Err(crate::error::Error::FailedToCreateObject);
                }
                lvgl::lv_obj_set_size(bar, lvgl::lv_pct(100), lvgl::LV_SIZE_CONTENT);
                lvgl::lv_obj_set_flex_flow(bar, lvgl::lv_flex_flow_t_LV_FLEX_FLOW_ROW);
                lvgl::lv_obj_set_flex_align(
                    bar,
                    lvgl::lv_flex_align_t_LV_FLEX_ALIGN_START,
                    lvgl::lv_flex_align_t_LV_FLEX_ALIGN_CENTER,
                    lvgl::lv_flex_align_t_LV_FLEX_ALIGN_CENTER,
                );
                lvgl::lv_obj_set_style_pad_all(bar, spacing, lvgl::LV_STATE_DEFAULT);
                lvgl::lv_obj_set_style_pad_column(bar, spacing, lvgl::LV_STATE_DEFAULT);
                lvgl::lv_obj_set_style_border_width(bar, 0, lvgl::LV_STATE_DEFAULT);
                lvgl::lv_obj_set_style_radius(bar, 0, lvgl::LV_STATE_DEFAULT);
                lvgl::lv_obj_remove_flag(bar, lvgl::lv_obj_flag_t_LV_OBJ_FLAG_SCROLLABLE);

                let input = lvgl::lv_textarea_create(bar);

                if input.is_null() {
                    return Err(crate::error::Error::FailedToCreateObject);
                }

                lvgl::lv_textarea_set_placeholder_text(
                    input,
                    t!(c"input.command_placeholder").as_ptr(),
                );
                lvgl::lv_obj_set_flex_grow(input, 1);
                lvgl::lv_textarea_set_one_line(input, true);
                lvgl::lv_obj_set_style_text_font(
                    input,
                    get_font_monospace_medium(),
                    lvgl::LV_STATE_DEFAULT,
                );

                let clear = Self::icon_button(bar, lvgl::LV_SYMBOL_TRASH)?;
                lvgl::lv_theme_shadcn_set_variant(
                    clear,
                    lvgl::lv_theme_shadcn_variant_t_LV_THEME_SHADCN_VARIANT_OUTLINE,
                );
                let send = Self::icon_button(bar, lvgl::LV_SYMBOL_NEW_LINE)?;

                (input, send, clear)
            };

            Inner {
                window,
                buffer,
                display,
                input,
                send,
                clear,
                validated_input: VecDeque::with_capacity(128),
            }
        });

        Ok(Self {
            input_waiters: xila::file_system::WaitQueue::new(),
            input_closed: core::sync::atomic::AtomicBool::new(false),
            inner: RwLock::new(inner),
            graphics_manager,
        })
    }

    /// A toolbar button showing one symbol.
    ///
    /// # Safety
    /// `parent` must be live and the caller must hold the LVGL lock.
    unsafe fn icon_button(parent: *mut lv_obj_t, symbol: &[u8]) -> Result<*mut lv_obj_t> {
        unsafe {
            let button = lvgl::lv_button_create(parent);
            if button.is_null() {
                return Err(Error::FailedToCreateObject);
            }
            let label = lvgl::lv_label_create(button);
            if label.is_null() {
                return Err(Error::FailedToCreateObject);
            }
            lvgl::lv_label_set_text(label, symbol.as_ptr() as *const core::ffi::c_char);
            lvgl::lv_obj_center(label);
            Ok(button)
        }
    }

    pub fn print(&self, text: &str) -> Result<()> {
        let mut inner = self.inner.try_write().map_err(|_| Error::RessourceBusy)?;
        let _lock = self
            .graphics_manager
            .try_lock()
            .ok_or(Error::RessourceBusy)?;

        Self::print_internal(&mut inner, text)?;

        Ok(())
    }

    fn get_start_index(buffer: &mut String, text: &str) {
        let start_index = if let Some(last_clear) = text.rfind(Self::CLEAR) {
            buffer.clear();
            last_clear + Self::CLEAR.len()
        } else {
            0
        };

        let start_index = if let Some(last_home) = text.rfind(Self::HOME) {
            start_index.max(last_home + Self::HOME.len())
        } else {
            start_index
        };

        let text = &text[start_index..];

        buffer.push_str(text);
    }

    fn print_internal(inner: &mut Inner, text: &str) -> Result<()> {
        inner.buffer.pop(); // Remove the trailing null character

        Self::get_start_index(&mut inner.buffer, text);
        inner.buffer.push('\0');

        unsafe {
            lvgl::lv_label_set_text_static(
                inner.display,
                inner.buffer.as_ptr() as *const core::ffi::c_char,
            );
            lvgl::lv_obj_scroll_to_view(inner.display, true);
        }

        Ok(())
    }

    async fn print_line_internal(inner: &mut Inner, text: &str) -> Result<()> {
        inner.buffer.pop(); // Remove the trailing null character

        Self::get_start_index(&mut inner.buffer, text);
        inner.buffer.push_str("\n\0");

        unsafe {
            lvgl::lv_label_set_text_static(
                inner.display,
                inner.buffer.as_ptr() as *const core::ffi::c_char,
            );
            lvgl::lv_obj_scroll_to_view(inner.display, true);
        }

        Ok(())
    }

    pub fn read_input(&self, buffer: &mut [u8]) -> Result<usize> {
        let mut inner = self.inner.try_write().map_err(|_| Error::RessourceBusy)?;

        let mut read = 0;

        buffer
            .iter_mut()
            .zip(inner.validated_input.iter())
            .for_each(|(byte, &char)| {
                *byte = char;
                read += 1;
            });

        inner.validated_input.drain(0..read);

        Ok(read)
    }

    fn get_input(text_area: *mut lv_obj_t) -> &'static str {
        unsafe {
            let text = lvgl::lv_textarea_get_text(text_area);

            CStr::from_ptr(text).to_str().unwrap_or("")
        }
    }

    /// Echoes the typed line and queues it, with its newline, for the reader.
    async fn submit(inner: &mut Inner) -> Result<()> {
        let text = Self::get_input(inner.input);

        Self::print_line_internal(inner, text).await?;

        let text = Self::get_input(inner.input);

        inner.validated_input.extend(text.as_bytes());
        inner.validated_input.push_back(b'\n');

        unsafe {
            lvgl::lv_textarea_set_text(inner.input, c"".as_ptr());
            // A click on Send moves the focus to the button; typing continues
            // in the field.
            lvgl::lv_group_focus_obj(inner.input);
        }
        Ok(())
    }

    /// Byte offset of the line being written, after the last newline.
    fn last_line_start(buffer: &str) -> usize {
        buffer.rfind('\n').map_or(0, |index| index + 1)
    }

    pub async fn handle_events(&self) -> Result<bool> {
        let mut running = true;
        let mut notify_input = false;

        graphics::lock!(self.graphics_manager, {
            let mut inner = self.inner.write().await;

            while let Some(event) = inner.window.pop_event() {
                match event.code {
                    EventKind::Delete | EventKind::CloseRequested
                        if event.target == inner.window.as_object_mutable() =>
                    {
                        running = false;
                    }
                    EventKind::Key => {
                        if let Some(Key::Enter) = event.key {
                            Self::submit(&mut inner).await?;
                            notify_input = true;
                        }
                    }
                    EventKind::Clicked if event.target == inner.send => {
                        Self::submit(&mut inner).await?;
                        notify_input = true;
                    }
                    EventKind::Clicked if event.target == inner.clear => {
                        // Only the scrollback is dropped; the shell keeps
                        // running and its current line, the prompt, stays.
                        let last_line = Self::last_line_start(&inner.buffer);
                        inner.buffer.drain(..last_line);
                        Self::print_internal(&mut inner, "")?;
                        unsafe { lvgl::lv_group_focus_obj(inner.input) };
                    }
                    _ => {}
                }
            }
        });

        if !running {
            self.input_closed
                .store(true, core::sync::atomic::Ordering::Release);
        }
        if notify_input || !running {
            self.input_waiters.wake_all();
        }
        Ok(running)
    }
}

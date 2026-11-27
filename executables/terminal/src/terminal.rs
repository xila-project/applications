use crate::{
    command_input::{CommandInput, CommandOutput},
    error::{Error, Result},
};
use alloc::{boxed::Box, collections::vec_deque::VecDeque, string::String};
use core::ffi::CStr;
use xila::file_system::character_device::TerminalAttributes;
use xila::graphics::OwnedWindow;
use xila::graphics::fonts::get_font_monospace_medium;
use xila::graphics::{
    self, Color, EventKind,
    lvgl::{self, lv_obj_t},
};
use xila::internationalization::t;
use xila::synchronization::{blocking_mutex::raw::CriticalSectionRawMutex, rwlock::RwLock};

struct TerminalWidgets {
    buffer: String,
    display: *mut lvgl::lv_obj_t,
    input: *mut lvgl::lv_obj_t,
    send: *mut lvgl::lv_obj_t,
    clear: *mut lvgl::lv_obj_t,
}

pub(crate) struct Inner {
    window: OwnedWindow,
    buffer: String,
    display: *mut lvgl::lv_obj_t,
    input: *mut lvgl::lv_obj_t,
    send: *mut lvgl::lv_obj_t,
    clear: *mut lvgl::lv_obj_t,
    validated_input: VecDeque<u8>,
    command_input: Box<CommandInput>,
    command_output: CommandOutput,
}

pub struct Terminal {
    pub(crate) input_waiters: xila::file_system::WaitQueue<16>,
    pub(crate) graphics_waiters: xila::file_system::WaitQueue<16>,
    pub(crate) input_closed: portable_atomic::AtomicBool,
    inner: RwLock<CriticalSectionRawMutex, Inner>,
    graphics_manager: &'static graphics::Manager,
}

unsafe impl Send for Terminal {}

unsafe impl Sync for Terminal {}

impl Terminal {
    pub async fn new(graphics_manager: &'static graphics::Manager) -> Result<Self> {
        Self::new_with(graphics_manager, Self::build_widgets).await
    }

    async fn new_with(
        graphics_manager: &'static graphics::Manager,
        build: impl FnOnce(&mut OwnedWindow) -> Result<TerminalWidgets>,
    ) -> Result<Self> {
        let inner = graphics::lock!(graphics_manager, {
            let mut window = graphics_manager.create_window().await?;
            let widgets = match build(&mut window) {
                Ok(widgets) => widgets,
                Err(error) => {
                    // SAFETY: the graphics lock is held, and this function owns
                    // `window`. Delete it directly so `OwnedWindow::drop` does
                    // not deadlock on the non-re-entrant lock already held here.
                    unsafe {
                        lvgl::lv_obj_delete(window.into_raw().as_ptr().cast());
                    }
                    return Err(error);
                }
            };

            let command_input = Box::new(CommandInput::new());
            unsafe { command_input.attach(widgets.input) };
            Inner {
                window,
                buffer: widgets.buffer,
                display: widgets.display,
                input: widgets.input,
                send: widgets.send,
                clear: widgets.clear,
                validated_input: VecDeque::with_capacity(128),
                command_input,
                command_output: CommandOutput::default(),
            }
        });

        Ok(Self {
            input_waiters: xila::file_system::WaitQueue::new(),
            graphics_waiters: xila::file_system::WaitQueue::new(),
            input_closed: portable_atomic::AtomicBool::new(false),
            inner: RwLock::new(inner),
            graphics_manager,
        })
    }

    fn build_widgets(window: &mut OwnedWindow) -> Result<TerminalWidgets> {
        unsafe {
            window.set_icon(">_", Color::BLACK);

            lvgl::lv_obj_set_flex_flow(
                window.as_object_mutable(),
                lvgl::lv_flex_flow_t_LV_FLEX_FLOW_COLUMN,
            );
            // Edge-to-edge window chrome: the shared window class already
            // flattens border and radius, only the container padding remains.
            lvgl::lv_obj_set_style_pad_all(window.as_object_mutable(), 0, lvgl::LV_STATE_DEFAULT);
        }

        let compact = unsafe {
            lvgl::lv_obj_update_layout(window.as_object_mutable());
            lvgl::lv_obj_set_style_pad_row(window.as_object_mutable(), 0, lvgl::LV_STATE_DEFAULT);
            lvgl::lv_obj_get_width(window.as_object_mutable()) < 560
        };
        let spacing = if compact { 6 } else { 10 };

        let container = unsafe {
            let container = lvgl::lv_obj_create(window.as_object_mutable());
            if container.is_null() {
                return Err(crate::error::Error::FailedToCreateObject);
            }

            lvgl::lv_obj_set_width(container, lvgl::lv_pct(100));
            lvgl::lv_obj_set_flex_grow(container, 1);
            lvgl::lv_obj_set_flex_flow(container, lvgl::lv_flex_flow_t_LV_FLEX_FLOW_COLUMN);
            // The output sits on the darker background surface, inset from
            // the window edges, without the card border and radius.
            graphics::theme::set_surface(container, graphics::theme::Surface::Background);
            lvgl::lv_obj_set_style_pad_all(container, spacing, lvgl::LV_STATE_DEFAULT);
            lvgl::lv_obj_set_style_border_width(container, 0, lvgl::LV_STATE_DEFAULT);
            lvgl::lv_obj_set_style_radius(container, 0, lvgl::LV_STATE_DEFAULT);
            lvgl::lv_obj_set_scroll_snap_y(container, lvgl::lv_scroll_snap_t_LV_SCROLL_SNAP_END);

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
            lvgl::lv_textarea_set_password_show_time(input, 0);
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

        Ok(TerminalWidgets {
            buffer,
            display,
            input,
            send,
            clear,
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

    pub(crate) fn print(&self, bytes: &[u8]) -> Result<()> {
        let mut inner = self.inner.try_write().map_err(|_| Error::WouldBlock)?;
        let _lock = self.graphics_manager.try_lock().ok_or(Error::WouldBlock)?;

        for text in xila::shared::utf8_chunks(bytes) {
            Self::print_internal(&mut inner, text)?;
        }
        Ok(())
    }

    fn print_internal(inner: &mut Inner, text: &str) -> Result<()> {
        inner.buffer.pop(); // Remove the trailing null character.
        let raw = !inner.command_input.attributes().is_canonical();
        let was_editing = inner.command_output.edit().is_some();
        inner.command_output.write(text, &mut inner.buffer, raw);
        inner.buffer.push('\0');

        unsafe {
            lvgl::lv_label_set_text_static(
                inner.display,
                inner.buffer.as_ptr() as *const core::ffi::c_char,
            );
            Self::show_last_line(inner.display);

            let edit = inner.command_output.edit();
            if edit.is_some() || was_editing {
                let (line, cursor) = edit.unwrap_or(("", 0));
                let mut terminated = String::from(line);
                terminated.push('\0');
                inner.command_input.updating_widget(|| {
                    lvgl::lv_textarea_set_text(inner.input, terminated.as_ptr().cast());
                    lvgl::lv_textarea_set_cursor_pos(inner.input, cursor as i32);
                });
            }
        }

        Ok(())
    }

    pub(crate) fn terminal_attributes(&self) -> Result<TerminalAttributes> {
        let inner = self.inner.try_read().map_err(|_| Error::WouldBlock)?;
        // LVGL callbacks access the same input state under the graphics lock.
        let _lock = self.graphics_manager.try_lock().ok_or(Error::WouldBlock)?;
        Ok(inner.command_input.attributes())
    }

    pub(crate) fn set_terminal_attributes(&self, attributes: TerminalAttributes) -> Result<()> {
        let mut inner = self.inner.try_write().map_err(|_| Error::WouldBlock)?;
        let _lock = self.graphics_manager.try_lock().ok_or(Error::WouldBlock)?;
        let saved = inner.command_input.attributes();
        inner.command_input.set_attributes(attributes);
        if saved.is_canonical() != attributes.is_canonical() {
            inner.command_output.reset();
            inner.command_input.updating_widget(|| unsafe {
                lvgl::lv_textarea_set_text(inner.input, c"".as_ptr());
            });
        }
        unsafe {
            lvgl::lv_textarea_set_password_mode(
                inner.input,
                attributes.is_canonical() && !attributes.is_echo_enabled(),
            );
        }
        Ok(())
    }

    /// Scrolls the output to its last line, without an animation.
    ///
    /// An animated scroll redraws the whole output on every frame for up to
    /// 400 ms, and each print started a new one.
    ///
    /// # Safety
    /// `display` must be live and the caller must hold the LVGL lock.
    unsafe fn show_last_line(display: *mut lvgl::lv_obj_t) {
        unsafe { lvgl::lv_obj_scroll_to_view(display, false) };
    }

    pub fn read_input(&self, buffer: &mut [u8]) -> Result<usize> {
        let mut inner = self.inner.try_write().map_err(|_| Error::WouldBlock)?;

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

    fn submit(inner: &mut Inner) -> Result<()> {
        let attributes = inner.command_input.attributes();
        if attributes.is_canonical() {
            let text = Self::get_input(inner.input);
            inner.validated_input.extend(text.as_bytes());
            inner.validated_input.push_back(b'\n');
            if attributes.is_echo_enabled() {
                Self::print_internal(inner, text)?;
            }
            Self::print_internal(inner, "\n")?;
            unsafe { lvgl::lv_textarea_set_text(inner.input, c"".as_ptr()) };
        } else {
            inner.command_input.queue(b"\n");
        }
        unsafe { lvgl::lv_group_focus_obj(inner.input) };
        Ok(())
    }

    /// Byte offset of the line being written, after the last newline.
    fn last_line_start(buffer: &str) -> usize {
        buffer.rfind('\n').map_or(0, |index| index + 1)
    }

    pub async fn handle_events(&self) -> Result<bool> {
        let mut running = true;

        let notify_input = graphics::lock!(self.graphics_manager, {
            let mut inner = self.inner.write().await;

            while let Some(event) = inner.window.pop_event() {
                match event.code {
                    EventKind::Delete | EventKind::CloseRequested
                        if event.is_target(inner.window.as_object()) =>
                    {
                        running = false;
                    }
                    EventKind::Ready if event.is_target(inner.input) => {
                        Self::submit(&mut inner)?;
                    }
                    EventKind::Clicked if event.is_target(inner.send) => {
                        Self::submit(&mut inner)?;
                    }
                    EventKind::Clicked if event.is_target(inner.clear) => {
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
            let Inner {
                command_input,
                validated_input,
                ..
            } = &mut *inner;
            command_input.drain_into(validated_input);
            !validated_input.is_empty()
        });

        if !running {
            self.input_closed
                .store(true, portable_atomic::Ordering::Release);
        }
        if notify_input || !running {
            self.input_waiters.wake_all();
        }
        // Writers and control calls can retry now that the graphics lock is free.
        self.graphics_waiters.wake_all();
        Ok(running)
    }
}

#[cfg(all(
    test,
    any(target_os = "linux", target_os = "macos", target_os = "windows")
))]
mod setup_failure_tests {
    use alloc::boxed::Box;
    use core::{
        task::{Context, Poll},
        time::Duration,
    };
    use std::sync::OnceLock;
    use xila::{
        file_system::{
            self, ControlCommand, ControlCommandIdentifier, DirectBaseOperations,
            DirectCharacterDevice, MountOperations, Size,
        },
        graphics::{self, GET_RESOLUTION, Point, SET_DRAWING_AREA, WAS_RESIZED, lvgl},
        shared::AnyByLayout,
    };

    use super::{Error, Terminal};

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
                    WAS_RESIZED::cast_output(output).map(|resized| *resized = 0)
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
            graphics::initialize_with_input_devices(time, &HeadlessScreen, &[], 1024, false).await,
        ));
        graphics::set_ffi_manager(manager);
        MANAGER.get_or_init(|| manager)
    }

    #[xila::task::test(task_path = xila::task)]
    async fn terminal_setup_failure_deletes_window_without_deadlocking() {
        let manager = headless_graphics_manager().await;
        let baseline_windows = manager.get_window_count().await.unwrap();

        let (sender, receiver) = std::sync::mpsc::channel();
        let handle = std::thread::spawn(move || {
            let result = xila::task::block_on(Terminal::new_with(manager, |window| {
                let child = unsafe { lvgl::lv_obj_create(window.as_object_mutable()) };
                assert!(!child.is_null());
                Err(Error::FailedToCreateObject)
            }));
            let _ = sender.send(result.err());
        });

        let error = receiver
            .recv_timeout(Duration::from_secs(5))
            .unwrap_or_else(|_| {
                // A deadlocked worker keeps the graphics lock forever, so later
                // tests in this binary would block on it: exit instead of panicking.
                std::eprintln!("Terminal::new_with hung (likely deadlocked on the graphics lock)");
                std::process::exit(1)
            });
        handle.join().unwrap();

        assert!(matches!(error, Some(Error::FailedToCreateObject)));
        assert!(
            manager.try_lock().is_some(),
            "graphics lock must be released after Terminal setup error"
        );
        assert_eq!(manager.get_window_count().await.unwrap(), baseline_windows);
    }

    #[xila::task::test(task_path = xila::task)]
    async fn raw_terminal_forwards_keys_and_displays_the_shell_cursor() {
        use xila::{file_system::character_device::TerminalAttributes, graphics::Key};

        let manager = headless_graphics_manager().await;
        let terminal = Terminal::new(manager).await.unwrap();
        terminal
            .set_terminal_attributes(TerminalAttributes::RAW)
            .unwrap();

        manager
            .lock_function(|| unsafe {
                let input = terminal.inner.try_read().unwrap().input;
                let mut key = u32::from(Key::Left);
                lvgl::lv_obj_send_event(
                    input,
                    lvgl::lv_event_code_t_LV_EVENT_KEY,
                    (&raw mut key).cast(),
                );
                lvgl::lv_textarea_add_text(input, c"é".as_ptr());
                Ok(())
            })
            .await
            .unwrap();
        terminal.handle_events().await.unwrap();
        let mut keys = [0; 16];
        let size = terminal.read_input(&mut keys).unwrap();
        assert_eq!(&keys[..size], "\x1b[Dé".as_bytes());

        terminal
            .print("prompt$ \x1b7\x1b8\x1b[Jrésumé\x1b8ré".as_bytes())
            .unwrap();
        manager
            .lock_function(|| unsafe {
                let input = terminal.inner.try_read().unwrap().input;
                assert_eq!(Terminal::get_input(input), "résumé");
                assert_eq!(lvgl::lv_textarea_get_cursor_pos(input), 2);
                Ok(())
            })
            .await
            .unwrap();
    }

    #[xila::task::test(task_path = xila::task)]
    async fn printing_shows_the_last_line_without_animating_the_scroll() {
        let manager = headless_graphics_manager().await;
        let terminal = Terminal::new(manager).await.unwrap();
        for line in 0..40 {
            terminal
                .print(std::format!("line {line}\n").as_bytes())
                .unwrap();
        }
        manager
            .lock_function(|| unsafe {
                let display = terminal.inner.try_read().unwrap().display;
                let container = lvgl::lv_obj_get_parent(display);
                lvgl::lv_obj_update_layout(container);
                // The output is longer than its container, and already at its end.
                assert!(lvgl::lv_obj_get_scroll_y(container) > 0);
                assert_eq!(lvgl::lv_obj_get_scroll_bottom(container), 0);
                assert!(lvgl::lv_anim_get(container.cast(), None).is_null());
                Ok(())
            })
            .await
            .unwrap();
    }
}

#![no_std]

xila::internationalization::i18n!("locales");

mod desk;
mod error;
mod home;
mod icon;
mod layout;
mod login;
mod shortcut;

extern crate alloc;
#[cfg(test)]
extern crate std;

#[cfg(all(
    test,
    any(target_os = "linux", target_os = "macos", target_os = "windows")
))]
extern crate abi_definitions;

#[cfg(all(
    test,
    any(target_os = "linux", target_os = "macos", target_os = "windows")
))]
testing::instantiate_global_allocator!();

use crate::{desk::Desk, error::Error};
use alloc::{boxed::Box, string::String, vec::Vec};
use core::fmt::Write;
use core::num::NonZeroUsize;
use core::time::Duration;
use getargs_derive::GetArgs;
use home::Home;
use layout::Layout;
use login::Login;
use xila::executable::{self, ExecutableTrait, Standard};
use xila::graphics;
use xila::task;

#[derive(GetArgs)]
#[command(name = "graphical_shell", about = "Run the graphical shell.", version)]
struct GraphicalShellArguments {
    /// Show the on-screen keyboard.
    #[arg(short = 'k', long = "show-keyboard")]
    show_keyboard: bool,
}

pub async fn main(
    mut standard: Standard<'static>,
    arguments: Vec<String>,
) -> Result<(), NonZeroUsize> {
    let GraphicalShellArguments { show_keyboard } =
        match GraphicalShellArguments::from_args(arguments.iter().map(String::as_str)) {
            Ok(arguments) => arguments,
            Err(error) if error.is_help() || error.is_version() => {
                writeln!(standard.out(), "{error}").ok();
                return Ok(());
            }
            Err(error) => {
                writeln!(standard.error(), "{error}").ok();
                return Err(NonZeroUsize::new(1).unwrap());
            }
        };

    Shell::new(standard, show_keyboard).await.main().await
}

// Fields drop in order: `_home` first (its button lives on `lv_layer_top()`
// and its `RELEASED` handler passes the desk window as user data), then the
// shell's own windows (`desk`, `login`), then `layout` (which keeps the screen
// callback parent alive while `desk` detaches its callback and reparents any
// remaining application windows back to the screen before deleting `body`).
pub struct Shell {
    context: &'static xila::executable::ExecutableContext,
    _standard: Standard<'static>,
    running: bool,
    _home: Option<Box<Home>>,
    desk: Option<Box<Desk>>,
    login: Option<Box<Login>>,
    layout: Layout,
}

pub struct ShellExecutable;

impl ExecutableTrait for ShellExecutable {
    fn main(standard: Standard<'static>, arguments: Vec<String>) -> executable::MainFuture {
        Box::pin(async move { main(standard, arguments).await })
    }
}

impl Shell {
    pub async fn new(standard: Standard<'static>, show_keyboard: bool) -> Self {
        let context = standard.context;
        let layout = Layout::new(context, show_keyboard).await.unwrap();

        let login = Box::new(Login::new(context).await.unwrap());

        Self {
            context,
            _standard: standard,
            layout,
            desk: None,
            running: true,
            _home: None,
            login: Some(login),
        }
    }

    pub async fn main(&mut self) -> Result<(), NonZeroUsize> {
        let task_manager = self.context.task_manager;
        let shell_task = task_manager.get_current_task_identifier().await;
        while self.running {
            // Leave the session cooperatively when the supervisor asks.
            if xila::lifecycle::tasks::termination_requested(task_manager, shell_task).await {
                break;
            }
            self.layout.run().await;

            if let Some(login) = &mut self.login {
                login.event_handler().await;

                if let Some(user) = login.get_logged_user() {
                    let user_name = self
                        .context
                        .users_manager
                        .get_user_name(user)
                        .await
                        .unwrap();

                    let task_manager = &self.context.task_manager;
                    let task = task_manager.get_current_task_identifier().await;

                    graphics::appearance::restore_appearance(
                        graphics::ffi_manager(),
                        self.context.virtual_file_system,
                        task,
                    )
                    .await;

                    task_manager
                        .set_environment_variable(task, "User", user_name.as_str())
                        .await
                        .map_err(Error::FailedToSetEnvironmentVariable)?;

                    self.desk = Some(Box::new(
                        Desk::new(self.context, self.layout.get_windows_parent()).await?,
                    ));

                    if let Some(desk) = &mut self.desk {
                        self._home = Some(Box::new(
                            Home::new(graphics::ffi_manager(), desk.get_window_object()).await?,
                        ));
                    }

                    self.login = None;
                }
            }

            if let Some(desk) = &mut self.desk
                && !desk.is_hidden()
            {
                desk.handle_events().await;
            }

            task::sleep(Duration::from_millis(50)).await;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use getargs::Options;

    use super::GraphicalShellArguments;

    #[test]
    fn parses_show_keyboard_flag_without_panic() {
        let args = ["--show-keyboard"];
        let mut options = Options::new(args.into_iter());

        let parsed = GraphicalShellArguments::parse(&mut options);

        assert!(parsed.is_ok());
        assert!(parsed.unwrap().show_keyboard);
    }

    #[test]
    fn keyboard_is_hidden_by_default() {
        let parsed = GraphicalShellArguments::from_args([].into_iter()).unwrap();
        assert!(!parsed.show_keyboard);
    }

    #[test]
    fn help_and_version_are_information_requests() {
        for argument in ["--help", "-h", "--version", "-V"] {
            let error = GraphicalShellArguments::from_args([argument].into_iter())
                .err()
                .unwrap();
            assert!(error.is_help() || error.is_version());
        }
        assert!(GraphicalShellArguments::HELP.contains("Usage: graphical_shell"));
        assert!(GraphicalShellArguments::HELP.contains("--show-keyboard"));
    }

    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    mod teardown {
        use alloc::{boxed::Box, vec::Vec};
        use core::{
            ffi::c_void,
            ptr::null_mut,
            task::{Context, Poll},
        };
        use std::sync::{Mutex, OnceLock};
        use xila::{
            file_system::{
                self, ControlCommand, ControlCommandIdentifier, DirectBaseOperations,
                DirectCharacterDevice, MountOperations, Size,
            },
            graphics::{
                self, EventKind, GET_RESOLUTION, Point, SET_DRAWING_AREA, WAS_RESIZED, lvgl,
            },
            shared::AnyByLayout,
        };

        use crate::{
            Shell,
            desk::{Desk, WINDOWS_PARENT_CHILD_CHANGED},
            home::Home,
        };

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
                Poll::Ready(Err(file_system::Error::UnsupportedOperation))
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
                    _ => Err(file_system::Error::UnsupportedOperation),
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

        static EVENT_LOG: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());

        unsafe extern "C" fn log_event(event: *mut lvgl::lv_event_t) {
            let tag = unsafe { *(lvgl::lv_event_get_user_data(event) as *const &'static str) };
            EVENT_LOG.lock().unwrap().push(tag);
        }

        unsafe fn attach_probe(
            object: *mut lvgl::lv_obj_t,
            code: lvgl::lv_event_code_t,
            tag: &'static &'static str,
        ) {
            unsafe {
                lvgl::lv_obj_add_event_cb(
                    object,
                    Some(log_event),
                    code,
                    (tag as *const &'static str).cast_mut().cast::<c_void>(),
                );
            }
        }

        #[xila::task::test(task_path = xila::task)]
        async fn shell_teardown_keeps_screen_and_open_application_window_alive() {
            static SCREEN_DELETE: &str = "screen";
            static HOME_DELETE: &str = "home";
            static DESK_DELETE: &str = "desk";
            static DESK_CHILD_CHANGED: &str = "desk_child_changed";
            static LOGIN_DELETE: &str = "login";
            static HEADER_DELETE: &str = "header";
            static BODY_DELETE: &str = "body";
            static KEYBOARD_DELETE: &str = "keyboard";
            static APP_DELETE: &str = "app";

            let standard = testing::initialize(false, false).await;
            let manager = headless_graphics_manager().await;
            let screen = manager.get_current_screen().unwrap();

            let baseline_screen_events = manager
                .lock_function_sync(|| unsafe {
                    attach_probe(
                        screen,
                        lvgl::lv_event_code_t_LV_EVENT_DELETE,
                        &SCREEN_DELETE,
                    );
                    Ok(lvgl::lv_obj_get_event_count(screen))
                })
                .unwrap();

            let context = standard.context;
            let mut shell = Shell::new(standard, true).await;
            let mut desk = Box::new(
                Desk::new(context, shell.layout.get_windows_parent())
                    .await
                    .unwrap(),
            );
            let desk_window = desk.get_window_object();
            let home = Box::new(Home::new(manager, desk_window).await.unwrap());
            let home_button = home.button_object();
            let login_window = shell.login.as_mut().unwrap().window_object();
            let header = shell.layout.header_object();
            let body = shell.layout.body_object();
            let keyboard = shell.layout.keyboard_object();
            shell.desk = Some(desk);
            shell._home = Some(home);

            // Open an application window while the shell is active.
            let mut app_window =
                graphics::lock!(manager, { manager.create_window().await.unwrap() });
            let app_window_object = app_window.as_object_mutable();
            let app_textarea = manager
                .lock_function_sync(|| unsafe {
                    assert_eq!(lvgl::lv_obj_get_parent(app_window_object), body);
                    // Layout registers 3 screen callbacks (Focused, Defocused, Cancel)
                    // and Desk registers 1 (All).
                    assert_eq!(
                        lvgl::lv_obj_get_event_count(screen),
                        baseline_screen_events + 4
                    );

                    let textarea = lvgl::lv_textarea_create(app_window_object);
                    assert!(!textarea.is_null());

                    attach_probe(
                        home_button,
                        lvgl::lv_event_code_t_LV_EVENT_DELETE,
                        &HOME_DELETE,
                    );
                    attach_probe(
                        desk_window,
                        lvgl::lv_event_code_t_LV_EVENT_DELETE,
                        &DESK_DELETE,
                    );
                    attach_probe(
                        desk_window,
                        WINDOWS_PARENT_CHILD_CHANGED.into_lvgl_code(),
                        &DESK_CHILD_CHANGED,
                    );
                    attach_probe(
                        login_window,
                        lvgl::lv_event_code_t_LV_EVENT_DELETE,
                        &LOGIN_DELETE,
                    );
                    attach_probe(
                        header,
                        lvgl::lv_event_code_t_LV_EVENT_DELETE,
                        &HEADER_DELETE,
                    );
                    attach_probe(body, lvgl::lv_event_code_t_LV_EVENT_DELETE, &BODY_DELETE);
                    attach_probe(
                        keyboard,
                        lvgl::lv_event_code_t_LV_EVENT_DELETE,
                        &KEYBOARD_DELETE,
                    );
                    attach_probe(
                        app_window_object,
                        lvgl::lv_event_code_t_LV_EVENT_DELETE,
                        &APP_DELETE,
                    );
                    Ok(textarea)
                })
                .unwrap();

            EVENT_LOG.lock().unwrap().clear();

            // Tear down the shell while the application window stays owned.
            drop(shell);

            assert_eq!(
                EVENT_LOG.lock().unwrap().as_slice(),
                &["home", "desk", "login", "header", "body", "keyboard"],
                "Shell must drop home, its own windows, and layout children in order without freeing the screen or open app windows or dispatching Desk's screen callback"
            );

            manager
                .lock_function_sync(|| unsafe {
                    assert_eq!(lvgl::lv_screen_active(), screen);
                    assert_eq!(lvgl::lv_obj_get_parent(app_window_object), screen);
                    assert_eq!(lvgl::lv_obj_get_event_count(screen), baseline_screen_events);

                    // Events bubbling to the screen must not reach freed
                    // Layout keyboard or Desk handlers.
                    for code in [
                        EventKind::Focused.into_lvgl_code(),
                        EventKind::Defocused.into_lvgl_code(),
                        EventKind::Cancel.into_lvgl_code(),
                    ] {
                        lvgl::lv_obj_send_event(app_textarea, code, null_mut());
                    }
                    let probe_child = lvgl::lv_obj_create(screen);
                    assert!(!probe_child.is_null());
                    lvgl::lv_obj_delete(probe_child);
                    Ok(())
                })
                .unwrap();

            // New windows created after shell teardown go onto the live screen.
            let mut later_window =
                graphics::lock!(manager, { manager.create_window().await.unwrap() });
            let later_window_object = later_window.as_object_mutable();
            manager
                .lock_function_sync(|| unsafe {
                    assert_eq!(lvgl::lv_obj_get_parent(later_window_object), screen);
                    Ok(())
                })
                .unwrap();
            drop(later_window);

            // Dropping the application's OwnedWindow deletes it once.
            drop(app_window);

            let log = EVENT_LOG.lock().unwrap();
            assert_eq!(log.iter().filter(|&&entry| entry == "app").count(), 1);
            assert_eq!(log.iter().filter(|&&entry| entry == "screen").count(), 0);
            assert_eq!(
                log.iter()
                    .filter(|&&entry| entry == "desk_child_changed")
                    .count(),
                0
            );
            drop(log);

            // Detach the test's own screen probe.
            manager
                .lock_function_sync(|| unsafe {
                    lvgl::lv_obj_remove_event_cb_with_user_data(
                        screen,
                        Some(log_event),
                        (&SCREEN_DELETE as *const &'static str)
                            .cast_mut()
                            .cast::<c_void>(),
                    );
                    Ok(())
                })
                .unwrap();
        }
    }
}

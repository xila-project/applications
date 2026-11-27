use alloc::string::String;
use xila::file_system::Kind;
use xila::graphics::OwnedWindow;
use xila::graphics::{
    self, EventKind, lvgl,
    palette::{self, Hue},
};
use xila::task;

use crate::error::Result;
use crate::tabs::{AboutTab, AppearanceTab, GeneralTab, NetworkTab, PasswordTab, Tab};

pub struct Settings {
    graphics_manager: &'static graphics::Manager,
    window: OwnedWindow,
    tabs: [Tab; 5],
}

#[derive(Clone)]
pub struct FileItem {
    pub name: String,
    pub r#type: Kind,
    pub size: u64,
}

impl Settings {
    pub async fn new(context: &'static xila::executable::ExecutableContext) -> Result<Self> {
        let graphics_manager = graphics::ffi_manager();
        // The guard queues the window deletion when the lock is busy, so a
        // construction error can drop OwnedWindow while the guard is held.
        let mut guard = graphics_manager.lock().await;
        let mut window = guard.with_lvgl(|lvgl| graphics_manager.create_window(lvgl))??;

        guard.with_lvgl(|lvgl| window.set_icon(lvgl, "Se", WINDOW_ICON_COLOR))?;

        // Flatten window chrome so the tabview fills the window edge-to-edge
        unsafe {
            let window = window.as_object();
            lvgl::lv_obj_set_style_pad_all(window, 0, lvgl::LV_STATE_DEFAULT);
            lvgl::lv_obj_set_layout(window, lvgl::lv_layout_t_LV_LAYOUT_FLEX);
            lvgl::lv_obj_set_flex_flow(window, lvgl::lv_flex_flow_t_LV_FLEX_FLOW_COLUMN);
        }

        // Create tabview
        let tabview = unsafe {
            let tabview = lvgl::lv_tabview_create(window.as_object());

            if tabview.is_null() {
                return Err(crate::error::Error::FailedToCreateUiElement);
            }

            lvgl::lv_obj_set_size(tabview, lvgl::lv_pct(100), lvgl::lv_pct(100));
            lvgl::lv_obj_set_flex_grow(tabview, 1);

            tabview
        };

        // Create tabs
        let mut tabs = [
            Tab::General(GeneralTab::new(context)),
            Tab::Appearance(AppearanceTab::new(context).await),
            Tab::Password(PasswordTab::new(context)),
            Tab::Network(NetworkTab::new(context)),
            Tab::About(AboutTab::new(context)),
        ];

        for tab in &mut tabs {
            tab.create_ui(tabview).await?;
        }

        let manager = Self {
            graphics_manager,
            window,
            tabs,
        };

        Ok(manager)
    }

    pub async fn handle_events(&mut self) -> bool {
        let mut needs_refresh = false;
        let mut needs_preview = false;
        let mut guard = self.graphics_manager.lock().await;
        // Take each event in a short session; the guard is passed to the tabs
        // and stays held across the awaited handlers below.
        while let Some(event) = guard
            .with_lvgl(|lvgl| self.window.pop_event(lvgl))
            .ok()
            .flatten()
        {
            // Logique de filtrage spécifique à Settings
            if (event.code == EventKind::Delete || event.code == EventKind::CloseRequested)
                && event.is_target(self.window.as_object())
            {
                return false;
            }

            for tab in &mut self.tabs {
                if tab.handle_event(&event, &mut guard).await {
                    break;
                }
            }
        }
        for tab in &mut self.tabs {
            match tab {
                Tab::Network(network) => {
                    network.refresh().await;
                    needs_refresh |= network.needs_refresh();
                }
                // The local time preview changes every second.
                Tab::General(general) if general.is_active() => {
                    general.refresh();
                    needs_preview = true;
                }
                _ => {}
            }
        }
        drop(guard);

        // A time zone change is written after the graphics lock is released.
        for tab in &mut self.tabs {
            if let Tab::General(general) = tab {
                general.save_pending(self.graphics_manager).await;
            }
        }

        // The atomic waker does not remember a wake that arrived before
        // registration: register it, then check the queue again before
        // returning pending.
        let events = task::suspend(|ctx| {
            self.window.register_waker(ctx.waker());
            if let Some(mut guard) = self.graphics_manager.try_lock() {
                let pending = guard
                    .with_lvgl(|lvgl| self.window.peek_event(lvgl))
                    .unwrap_or(None)
                    .is_some();
                drop(guard);
                if pending {
                    ctx.waker().wake_by_ref();
                }
            }
        });
        if needs_refresh || needs_preview {
            let delay = if needs_refresh { 250 } else { 1000 };
            embassy_futures::select::select(
                events,
                task::sleep(core::time::Duration::from_millis(delay)),
            )
            .await;
        } else {
            events.await;
        }

        true
    }
}

// Interface panels own widgets and ephemeral password inputs. Release them
// under the graphics lock before OwnedWindow deletes their parent tree.
impl Drop for Settings {
    fn drop(&mut self) {
        let _lock = task::block_on(self.graphics_manager.lock());
        for tab in &mut self.tabs {
            if let Tab::Network(network) = tab {
                network.close_panel();
            }
        }
    }
}

const WINDOW_ICON_COLOR: graphics::Color = palette::get(Hue::Grey, palette::Tone::MAIN);

#[cfg(test)]
#[test]
fn window_icon_matches_platform_shortcut() {
    // Keep this in sync with Platforms' tools/fs_image/shortcuts/settings.json.
    assert_eq!(WINDOW_ICON_COLOR, graphics::Color::new(158, 158, 158));
}

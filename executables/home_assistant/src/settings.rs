//! Settings screen: the server connection, the long-lived access token (typed
//! in a field hidden behind a mask until Show is pressed, or imported from a
//! file such as one on an SD card) and the entity list with a picker on the
//! server. It edits a draft of the configuration and writes the same files as
//! the terminal route when saved.

use embedded_i18n::t;
use home_assistant::{
    api,
    configuration::{
        Configuration, DEFAULT_REFRESH_SECONDS, Invalid, MAXIMUM_ENTITIES, is_valid_entity_id,
    },
    error::Error,
    net::{Endpoint, device::Devices},
    picker::{self, Entry},
    storage::{MAXIMUM_TOKEN_LENGTH, Storage, validate_token},
    text::{error_message, invalid_message},
};
use std::{string::String, thread::sleep, time::Duration, vec::Vec};
use wasm::*;

use crate::widgets::*;

const ERROR_COLOR: u32 = 0xEF4444;
const SUCCESS_COLOR: u32 = 0x22C55E;
const MUTED_COLOR: u32 = 0x71717A;

/// Default path of the import file: the SD card mount of the boards that have
/// one, such as the WT32-SC01 Plus.
const DEFAULT_IMPORT_PATH: &str = "/media/sd_card/home_assistant_token.txt";

pub struct Context<'a> {
    pub storage: Option<&'a Storage>,
    /// Set when a close request is drained after a blocking call: the host
    /// sends it only once, so the run loop must see it.
    pub close_requested: &'a mut bool,
}

pub enum Outcome {
    Stay,
    Close,
    Saved(Box<Configuration>),
}

#[derive(Clone)]
enum Action {
    Save,
    Cancel,
    ToggleToken,
    Import,
    AddManual,
    OpenPicker,
    PickerBack,
    /// Shows the entities of the area at this index of the last areas answer.
    PickerArea(usize),
    PickerWithoutArea,
    RemoveEntity(usize),
    MoveUp(usize),
    MoveDown(usize),
}

/// The picker page over the settings.
struct Picker {
    root: *mut Object,
    /// The page body the list is built in.
    parent: *mut Object,
    content: *mut Object,
    /// The message line of the picker page: while the picker is open, the
    /// settings page is hidden, so messages go here.
    message: *mut Object,
    /// The last areas answer, when the area list shows.
    areas: Vec<Entry>,
    /// Switches of the entity rows showing, with the entity identifier.
    switches: Vec<(*mut Object, String)>,
}

pub struct Settings {
    window: *mut Object,
    root: *mut Object,
    draft: Configuration,
    message: *mut Object,
    url: *mut Object,
    refresh: *mut Object,
    token_field: *mut Object,
    token_toggle_label: *mut Object,
    /// The token of the token field. The field itself shows [`SECRET_MASK`]
    /// while it is hidden, so the secret is kept here, never read back from
    /// the masked text.
    token: String,
    token_visible: bool,
    /// Hint under the token field, shown while the field is hidden.
    token_hint: *mut Object,
    import_field: *mut Object,
    entities_section: *mut Object,
    entities_content: *mut Object,
    manual_field: *mut Object,
    picker: Option<Picker>,
    actions: Vec<(*mut Object, Action)>,
}

/// Caption above a field.
unsafe fn caption(parent: *mut Object, text: &str) -> wasm::Result<*mut Object> {
    unsafe {
        let caption = paragraph(parent, text, FONT_SMALL)?;
        object_set_text_color(caption, MUTED_COLOR, 0)?;
        Ok(caption)
    }
}

unsafe fn section(parent: *mut Object, text: &str) -> wasm::Result<*mut Object> {
    unsafe { paragraph(parent, text, FONT_MEDIUM) }
}

/// A page filling the window: a bar, a message line, then a scrolling body.
struct Page {
    root: *mut Object,
    bar: *mut Object,
    message: *mut Object,
    body: *mut Object,
}

unsafe fn page(window: *mut Object, title: &str) -> wasm::Result<Page> {
    unsafe {
        let root = container(window)?;
        object_set_width(root, percentage(100)?)?;
        object_set_flex_grow(root, 1)?;
        object_set_flex_flow(root, FlexFlow::Column)?;
        let bar = flex(root, FlexFlow::Row)?;
        let heading = label(bar, title, FONT_LARGE)?;
        object_set_width(heading, 0)?;
        object_set_flex_grow(heading, 1)?;
        let message = paragraph(root, "", FONT_SMALL)?;
        set_hidden(message, true)?;
        let body = container(root)?;
        object_set_width(body, percentage(100)?)?;
        object_set_flex_grow(body, 1)?;
        object_set_flex_flow(body, FlexFlow::Column)?;
        object_set_flex_align(body, FlexAlign::Start, FlexAlign::Start, FlexAlign::Start)?;
        object_set_padding(body, 4)?;
        object_add_flag(body, ObjectFlag::Scrollable)?;
        Ok(Page {
            root,
            bar,
            message,
            body,
        })
    }
}

impl Settings {
    /// Opens the settings over the main screen, from `configuration` or empty
    /// fields when there is none, and the saved `token` in the token field.
    pub unsafe fn open(
        window: *mut Object,
        configuration: Option<&Configuration>,
        token: Option<&str>,
    ) -> wasm::Result<Self> {
        let draft = configuration.cloned().unwrap_or_else(|| Configuration {
            url: String::from("http://"),
            refresh_seconds: DEFAULT_REFRESH_SECONDS,
            entities: Vec::new(),
        });
        unsafe {
            let Page {
                root,
                bar,
                message,
                body,
            } = page(window, t!("settings.title"))?;
            let actions = vec![
                (button(bar, t!("settings.cancel"), OUTLINE)?, Action::Cancel),
                (button(bar, t!("settings.save"), PRIMARY)?, Action::Save),
            ];
            let root_page = root;
            let root = body;

            section(root, t!("settings.connection"))?;
            caption(root, t!("settings.url"))?;
            let url = field(root, &draft.url, 256)?;
            caption(root, t!("settings.refresh"))?;
            let refresh = field(root, &draft.refresh_seconds.to_string(), 6)?;

            section(root, t!("settings.advanced"))?;
            caption(root, t!("settings.token"))?;
            let token_row = flex(root, FlexFlow::Row)?;
            let token_field = field(token_row, "", MAXIMUM_TOKEN_LENGTH as u32)?;
            object_set_width(token_field, 0)?;
            object_set_flex_grow(token_field, 1)?;
            let (token_toggle, token_toggle_label) =
                labelled_button(token_row, t!("settings.show"), OUTLINE)?;
            let token_hint = caption(root, t!("settings.token_hint"))?;
            caption(root, t!("settings.import_path"))?;
            let import_field = field(root, DEFAULT_IMPORT_PATH, 256)?;
            let import_row = flex(root, FlexFlow::Row)?;
            paragraph(root, t!("settings.token_help_create"), FONT_SMALL)?;
            paragraph(root, t!("settings.token_help_terminal"), FONT_SMALL)?;

            section(root, t!("settings.entities"))?;
            let entities_section = flex(root, FlexFlow::Column)?;
            let entities_content = container(entities_section)?;
            caption(root, t!("settings.entity_id"))?;
            let manual_field = field(root, "", 64)?;

            let token = token.unwrap_or("");
            let mut settings = Self {
                window,
                root: root_page,
                draft,
                message,
                url,
                refresh,
                token_field,
                token_toggle_label,
                token: String::from(token),
                // An empty token starts visible, so it can be typed directly.
                token_visible: token.is_empty(),
                token_hint,
                import_field,
                entities_section,
                entities_content,
                manual_field,
                picker: None,
                actions,
            };
            settings.actions.push((token_toggle, Action::ToggleToken));
            settings.actions.push((
                button(import_row, t!("settings.import"), OUTLINE)?,
                Action::Import,
            ));
            settings.update_token_field()?;
            let caption = if settings.token_visible {
                t!("settings.hide")
            } else {
                t!("settings.show")
            };
            set_text(settings.token_toggle_label, caption)?;
            set_hidden(settings.token_hint, settings.token_visible)?;
            settings.build_entities()?;
            Ok(settings)
        }
    }

    pub unsafe fn close(self) -> wasm::Result<()> {
        unsafe {
            if let Some(picker) = &self.picker {
                delete(picker.root)?;
            }
            delete(self.root)
        }
    }

    /// Entity list: one row per entity with reorder and remove actions, then
    /// the add buttons.
    unsafe fn build_entities(&mut self) -> wasm::Result<()> {
        unsafe {
            delete(self.entities_content)?;
            self.actions.retain(|(_, action)| {
                !matches!(
                    action,
                    Action::AddManual
                        | Action::OpenPicker
                        | Action::RemoveEntity(_)
                        | Action::MoveUp(_)
                        | Action::MoveDown(_)
                )
            });
            self.entities_content = flex(self.entities_section, FlexFlow::Column)?;
            let section = self.entities_content;
            for index in 0..self.draft.entities.len() {
                let row = flex(section, FlexFlow::Row)?;
                let name = label(row, &self.draft.entities[index], FONT_MEDIUM)?;
                object_set_width(name, 0)?;
                object_set_flex_grow(name, 1)?;
                if index > 0 {
                    self.actions.push((
                        button(row, t!("settings.up"), OUTLINE)?,
                        Action::MoveUp(index),
                    ));
                }
                if index + 1 < self.draft.entities.len() {
                    self.actions.push((
                        button(row, t!("settings.down"), OUTLINE)?,
                        Action::MoveDown(index),
                    ));
                }
                self.actions.push((
                    button(row, t!("settings.remove"), DESTRUCTIVE)?,
                    Action::RemoveEntity(index),
                ));
            }
            if self.draft.entities.len() < MAXIMUM_ENTITIES {
                let buttons = flex(section, FlexFlow::Row)?;
                self.actions.push((
                    button(buttons, t!("settings.add_manual"), OUTLINE)?,
                    Action::AddManual,
                ));
                self.actions.push((
                    button(buttons, t!("settings.add_from_server"), OUTLINE)?,
                    Action::OpenPicker,
                ));
            }
        }
        Ok(())
    }

    unsafe fn show_message(&self, text: &str, color: u32) -> wasm::Result<()> {
        // While the picker is open, its page covers the settings: the message
        // goes to the picker page, not to the hidden settings one.
        let message = match &self.picker {
            Some(picker) => picker.message,
            None => self.message,
        };
        unsafe {
            set_text(message, text)?;
            object_set_text_color(message, color, 0)?;
            set_hidden(message, text.is_empty())?;
        }
        Ok(())
    }

    /// Reads the connection fields into the draft.
    unsafe fn read_fields(&mut self) -> wasm::Result<core::result::Result<(), Invalid>> {
        unsafe {
            self.draft.url = field_text(self.url)?.trim().into();
            match field_text(self.refresh)?.trim().parse::<u32>() {
                Ok(seconds) => self.draft.refresh_seconds = seconds,
                Err(_) => return Ok(Err(Invalid::Refresh)),
            }
        }
        Ok(Ok(()))
    }

    /// The endpoint of the draft URL, or the validation message.
    unsafe fn endpoint(&mut self) -> wasm::Result<core::result::Result<Endpoint, String>> {
        if let Err(invalid) = unsafe { self.read_fields()? } {
            return Ok(Err(String::from(invalid_message(invalid))));
        }
        match Endpoint::parse(&self.draft.url) {
            Ok(endpoint) => Ok(Ok(endpoint)),
            Err(error) => Ok(Err(error_message(&error))),
        }
    }

    /// Shows the token field as the secret while it is visible, as the mask
    /// while it is hidden.
    unsafe fn update_token_field(&self) -> wasm::Result<()> {
        let shown = if self.token_visible {
            self.token.clone()
        } else if self.token.is_empty() {
            String::new()
        } else {
            String::from(SECRET_MASK)
        };
        unsafe { set_field_bounded(self.token_field, &shown, MAXIMUM_TOKEN_LENGTH as u32) }
    }

    /// The token of the token field, checked: the picker and the save both
    /// need a token the header can carry. While the field is visible, its
    /// text is read back, so a change event drained after a blocking call
    /// does not lose an edit.
    unsafe fn token(&mut self) -> wasm::Result<core::result::Result<String, Error>> {
        if self.token_visible {
            self.token = unsafe { field_text(self.token_field)? };
        }
        let token = self.token.trim();
        if token.is_empty() {
            return Ok(Err(Error::MissingToken));
        }
        Ok(validate_token(token).map(|()| String::from(token)))
    }

    /// Shows a message and lets the display catch up before a blocking call.
    unsafe fn busy(&self, text: &str) -> wasm::Result<()> {
        unsafe { self.show_message(text, MUTED_COLOR)? };
        sleep(Duration::from_millis(30));
        Ok(())
    }

    /// Discards the window events queued while a blocking call ran, so a tap
    /// made during it does not start another one right away. The close
    /// request is the exception: the host sends it only once, so it is kept
    /// for the run loop.
    unsafe fn drain_events(&mut self, context: &mut Context<'_>) -> wasm::Result<()> {
        loop {
            let mut code = EventCode::All as u32;
            let mut target: *mut Object = core::ptr::null_mut();
            unsafe {
                window_pop_event(
                    self.window,
                    &mut code,
                    (&mut target as *mut *mut Object).cast(),
                )?;
            }
            if code == EVENT_CLOSE_REQUESTED {
                *context.close_requested = true;
                return Ok(());
            }
            if code == EventCode::All as u32 {
                return Ok(());
            }
        }
    }

    /// Imports the token from the file of the import field into the token
    /// field. The source file is only read.
    unsafe fn import_token(&mut self) -> wasm::Result<()> {
        let path = unsafe { field_text(self.import_field)? };
        let path = path.trim();
        let result = if path.is_empty() {
            Err(Error::ImportMissing)
        } else {
            Storage::read_token_file(path)
        };
        match result {
            Ok(token) => {
                self.token = token;
                // An imported token is masked like a saved one.
                self.token_visible = false;
                unsafe {
                    self.update_token_field()?;
                    set_text(self.token_toggle_label, t!("settings.show"))?;
                    set_hidden(self.token_hint, false)?;
                    self.show_message(t!("settings.imported"), SUCCESS_COLOR)
                }
            }
            Err(error) => unsafe { self.show_message(&error_message(&error), ERROR_COLOR) },
        }
    }

    /// Opens the picker page and loads the areas of the server.
    unsafe fn open_picker(&mut self, context: &mut Context<'_>) -> wasm::Result<()> {
        let endpoint = match unsafe { self.endpoint()? } {
            Ok(endpoint) => endpoint,
            Err(message) => return unsafe { self.show_message(&message, ERROR_COLOR) },
        };
        let token = match unsafe { self.token()? } {
            Ok(token) => token,
            Err(error) => return unsafe { self.show_message(&error_message(&error), ERROR_COLOR) },
        };
        unsafe {
            set_hidden(self.root, true)?;
            sleep(Duration::from_millis(30));
            let Page {
                root: picker_root,
                bar,
                message: picker_message,
                body: parent,
            } = page(self.window, t!("settings.picker_title"))?;
            self.actions.push((
                button(bar, t!("settings.back"), OUTLINE)?,
                Action::PickerBack,
            ));
            let content = flex(parent, FlexFlow::Column)?;
            paragraph(content, t!("settings.loading"), FONT_SMALL)?;
            self.picker = Some(Picker {
                root: picker_root,
                parent,
                content,
                message: picker_message,
                areas: Vec::new(),
                switches: Vec::new(),
            });
            self.busy(t!("settings.loading"))?;
            let answer =
                api::render_template(&mut Devices, &endpoint, &token, &picker::areas_template());
            self.drain_events(context)?;
            match answer {
                Ok(text) => {
                    let answer = picker::parse_entries(&text, false);
                    if let Some(picker) = &mut self.picker {
                        picker.areas = answer.entries;
                    }
                    self.build_picker_areas(answer.truncated)?;
                    // The list replaced the loading line.
                    self.show_message("", MUTED_COLOR)
                }
                Err(error) => {
                    let text = error_message(&error);
                    self.close_picker_page()?;
                    self.show_message(&text, ERROR_COLOR)
                }
            }
        }
    }

    /// The area list of the picker.
    unsafe fn build_picker_areas(&mut self, truncated: bool) -> wasm::Result<()> {
        unsafe {
            let areas = match &self.picker {
                Some(picker) => picker.areas.clone(),
                None => return Ok(()),
            };
            self.actions.retain(|(_, action)| {
                !matches!(action, Action::PickerArea(_) | Action::PickerWithoutArea)
            });
            if let Some(picker) = &mut self.picker {
                delete(picker.content)?;
                picker.switches.clear();
                picker.content = flex(picker.parent, FlexFlow::Column)?;
                let content = picker.content;
                if areas.is_empty() {
                    paragraph(content, t!("settings.no_areas"), FONT_SMALL)?;
                }
                if truncated {
                    paragraph(content, t!("settings.list_truncated"), FONT_SMALL)?;
                }
                for (index, area) in areas.iter().enumerate() {
                    let caption = if area.name.is_empty() {
                        area.identifier.clone()
                    } else {
                        std::format!("{} ({})", area.name, area.identifier)
                    };
                    self.actions.push((
                        button(content, &caption, OUTLINE)?,
                        Action::PickerArea(index),
                    ));
                }
                self.actions.push((
                    button(content, t!("settings.without_area"), OUTLINE)?,
                    Action::PickerWithoutArea,
                ));
            }
        }
        Ok(())
    }

    /// The entities of one area, or of no area, as switch rows.
    unsafe fn build_picker_entities(
        &mut self,
        template: &str,
        context: &mut Context<'_>,
    ) -> wasm::Result<()> {
        let endpoint = match unsafe { self.endpoint()? } {
            Ok(endpoint) => endpoint,
            Err(message) => return unsafe { self.show_message(&message, ERROR_COLOR) },
        };
        let token = match unsafe { self.token()? } {
            Ok(token) => token,
            Err(error) => return unsafe { self.show_message(&error_message(&error), ERROR_COLOR) },
        };
        unsafe { self.busy(t!("settings.loading"))? };
        let answer = api::render_template(&mut Devices, &endpoint, &token, template);
        unsafe { self.drain_events(context)? };
        let answer = match answer {
            Ok(text) => picker::parse_entries(&text, true),
            Err(error) => return unsafe { self.show_message(&error_message(&error), ERROR_COLOR) },
        };
        unsafe {
            self.actions.retain(|(_, action)| {
                !matches!(action, Action::PickerArea(_) | Action::PickerWithoutArea)
            });
            let Some(picker) = &mut self.picker else {
                return Ok(());
            };
            delete(picker.content)?;
            picker.switches.clear();
            picker.content = flex(picker.parent, FlexFlow::Column)?;
            let content = picker.content;
            if answer.entries.is_empty() {
                paragraph(content, t!("settings.no_entities_in_area"), FONT_SMALL)?;
            }
            if answer.truncated {
                paragraph(content, t!("settings.list_truncated"), FONT_SMALL)?;
            }
            for entry in &answer.entries {
                let row = flex(content, FlexFlow::Row)?;
                let caption = if entry.name.is_empty() {
                    entry.identifier.clone()
                } else {
                    std::format!("{} ({})", entry.name, entry.identifier)
                };
                let name = label(row, &caption, FONT_MEDIUM)?;
                object_set_width(name, 0)?;
                object_set_flex_grow(name, 1)?;
                let switch = switch_create(row)?;
                let checked = self.draft.entities.contains(&entry.identifier);
                set_checked(switch, checked)?;
                picker.switches.push((switch, entry.identifier.clone()));
            }
            // The list replaced the loading line.
            self.show_message("", MUTED_COLOR)?;
        }
        Ok(())
    }

    unsafe fn close_picker_page(&mut self) -> wasm::Result<()> {
        if let Some(picker) = self.picker.take() {
            self.actions.retain(|(_, action)| {
                !matches!(
                    action,
                    Action::PickerBack | Action::PickerArea(_) | Action::PickerWithoutArea
                )
            });
            unsafe {
                delete(picker.root)?;
                set_hidden(self.root, false)?;
            }
        }
        Ok(())
    }

    /// Adds or removes an entity of the picker in the draft; returns whether
    /// the draft changed. Adding past the entity limit changes nothing.
    fn toggle_picker_entity(&mut self, identifier: &str, checked: bool) -> bool {
        if checked {
            if self.draft.entities.len() >= MAXIMUM_ENTITIES {
                return false;
            }
            if !self
                .draft
                .entities
                .iter()
                .any(|entity| entity == identifier)
            {
                self.draft.entities.push(String::from(identifier));
                return true;
            }
        } else {
            self.draft.entities.retain(|entity| entity != identifier);
        }
        true
    }

    /// Saves the draft: the configuration file and the token file, each
    /// written atomically.
    unsafe fn save(&mut self, context: &mut Context<'_>) -> wasm::Result<Outcome> {
        unsafe {
            if let Err(invalid) = self.read_fields()?.and_then(|()| self.draft.validate()) {
                self.show_message(invalid_message(invalid), ERROR_COLOR)?;
                return Ok(Outcome::Stay);
            }
            let Some(storage) = context.storage else {
                self.show_message(&error_message(&Error::Storage), ERROR_COLOR)?;
                return Ok(Outcome::Stay);
            };
            let token = match self.token()? {
                Ok(token) => token,
                Err(error) => {
                    self.show_message(&error_message(&error), ERROR_COLOR)?;
                    return Ok(Outcome::Stay);
                }
            };
            // The configuration is written first, the token second: a failed
            // token write leaves the previous token file in place.
            if let Err(error) = storage.save(&self.draft) {
                self.show_message(&error_message(&error), ERROR_COLOR)?;
                return Ok(Outcome::Stay);
            }
            if let Err(error) = storage.save_token(&token) {
                self.show_message(&error_message(&error), ERROR_COLOR)?;
                return Ok(Outcome::Stay);
            }
        }
        Ok(Outcome::Saved(Box::new(self.draft.clone())))
    }

    /// Handles one event; returns what the settings do next.
    pub unsafe fn handle(
        &mut self,
        code: u32,
        target: *mut Object,
        context: &mut Context<'_>,
    ) -> wasm::Result<Outcome> {
        unsafe {
            if code == EventCode::ValueChanged as u32 {
                if target == self.token_field {
                    if self.token_visible {
                        self.token = field_text(self.token_field)?;
                    } else {
                        // Hidden, the field shows the mask: an edit cannot be
                        // told apart from the mask, so it is discarded and the
                        // mask shown again. Setting the same text again would
                        // send another change event, so only reset when the
                        // text differs.
                        let shown = if self.token.is_empty() {
                            String::new()
                        } else {
                            String::from(SECRET_MASK)
                        };
                        if field_text(self.token_field)? != shown {
                            self.update_token_field()?;
                        }
                    }
                    return Ok(Outcome::Stay);
                }
                if let Some(picker) = &self.picker
                    && let Some((_, identifier)) =
                        picker.switches.iter().find(|(switch, _)| *switch == target)
                {
                    let identifier = identifier.clone();
                    let checked = is_checked(target)?;
                    if checked && !self.toggle_picker_entity(&identifier, checked) {
                        // The draft did not change: the switch goes back off.
                        set_checked(target, false)?;
                        let text = t!(
                            "settings.too_many_entities",
                            count = MAXIMUM_ENTITIES as u64
                        )
                        .to_string();
                        self.show_message(&text, ERROR_COLOR)?;
                    }
                }
                return Ok(Outcome::Stay);
            }
            if code != EventCode::Clicked as u32 {
                return Ok(Outcome::Stay);
            }
            let Some(action) = self
                .actions
                .iter()
                .find(|(object, _)| *object == target)
                .map(|(_, action)| action.clone())
            else {
                return Ok(Outcome::Stay);
            };
            match action {
                Action::Save => return self.save(context),
                Action::Cancel => return Ok(Outcome::Close),
                Action::ToggleToken => {
                    self.token_visible = !self.token_visible;
                    self.update_token_field()?;
                    let caption = if self.token_visible {
                        t!("settings.hide")
                    } else {
                        t!("settings.show")
                    };
                    set_text(self.token_toggle_label, caption)?;
                    set_hidden(self.token_hint, self.token_visible)?;
                }
                Action::Import => self.import_token()?,
                Action::AddManual => {
                    let text = field_text(self.manual_field)?.trim().to_string();
                    if is_valid_entity_id(&text)
                        && !self.draft.entities.iter().any(|entity| entity == &text)
                        && self.draft.entities.len() < MAXIMUM_ENTITIES
                    {
                        self.draft.entities.push(text);
                        set_field_bounded(self.manual_field, "", 64)?;
                        self.build_entities()?;
                    } else {
                        self.show_message(invalid_message(Invalid::Entity), ERROR_COLOR)?;
                    }
                }
                Action::OpenPicker => self.open_picker(context)?,
                Action::PickerBack => {
                    self.close_picker_page()?;
                    self.build_entities()?;
                }
                Action::PickerArea(index) => {
                    let area = self
                        .picker
                        .as_ref()
                        .and_then(|picker| picker.areas.get(index))
                        .map(|area| area.identifier.clone());
                    if let Some(area) = area {
                        let template = picker::area_entities_template(&area).map_err(|_| -1)?;
                        self.build_picker_entities(&template, context)?;
                    }
                }
                Action::PickerWithoutArea => {
                    let template = picker::no_area_entities_template();
                    self.build_picker_entities(&template, context)?;
                }
                Action::RemoveEntity(index) => {
                    if index < self.draft.entities.len() {
                        self.draft.entities.remove(index);
                    }
                    self.build_entities()?;
                }
                Action::MoveUp(index) => {
                    if index > 0 && index < self.draft.entities.len() {
                        self.draft.entities.swap(index - 1, index);
                    }
                    self.build_entities()?;
                }
                Action::MoveDown(index) => {
                    if index + 1 < self.draft.entities.len() {
                        self.draft.entities.swap(index, index + 1);
                    }
                    self.build_entities()?;
                }
            }
            // A blocking call may have queued taps: discard them, but keep a
            // close request for the run loop.
            self.drain_events(context)?;
        }
        Ok(Outcome::Stay)
    }
}

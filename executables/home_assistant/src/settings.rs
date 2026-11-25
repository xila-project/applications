//! Settings screen: the server connection, the account (a sign-in with
//! username and password, or a long-lived access token) and the entity list
//! with a picker on the server. It edits a draft of the configuration and
//! writes the same files as the serial-shell route when saved.

use embedded_i18n::t;
use home_assistant::{
    api,
    auth::{self, Credentials, FlowResult, Session},
    configuration::{
        Configuration, DEFAULT_REFRESH_SECONDS, Invalid, MAXIMUM_ENTITIES, is_valid_entity_id,
    },
    error::Error,
    net::{Endpoint, device::Devices},
    picker::{self, Entry},
    storage::{MAXIMUM_TOKEN_LENGTH, Storage},
    text::{apply_masked_edit, error_message, invalid_message, login_error, mask},
};
use std::{
    string::String,
    thread::sleep,
    time::{Duration, Instant},
    vec::Vec,
};
use wasm::*;

use crate::widgets::*;

const ERROR_COLOR: u32 = 0xEF4444;
const SUCCESS_COLOR: u32 = 0x22C55E;
const MUTED_COLOR: u32 = 0x71717A;

pub struct Context<'a> {
    pub storage: Option<&'a Storage>,
    pub session: &'a mut Session,
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
    SignIn,
    SignOut,
    SubmitSignIn,
    CancelSignIn,
    ReplaceToken,
    ToggleToken,
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

/// The account being edited: a signed-in session, or a long-lived token typed
/// in the advanced field (empty when the saved one is kept).
struct Account {
    signed_in: Option<Credentials>,
    token: String,
    token_saved: bool,
    token_visible: bool,
}

/// The sign-in form in progress. The field objects belong to the account row
/// and are rebuilt with it; their text is kept across rebuilds.
struct SignIn {
    flow_id: String,
    two_factor: bool,
    username: *mut Object,
    password: *mut Object,
    /// The password itself, hidden by the masked field. It is cleared after
    /// each request.
    password_text: String,
    code: *mut Object,
}

/// The picker page over the settings.
struct Picker {
    root: *mut Object,
    /// The page body the list is built in.
    parent: *mut Object,
    content: *mut Object,
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
    account_section: *mut Object,
    account_content: *mut Object,
    account: Account,
    sign_in: Option<SignIn>,
    token_section: *mut Object,
    token_content: *mut Object,
    token_field: *mut Object,
    token_toggle: *mut Object,
    token_toggle_label: *mut Object,
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
    /// Opens the settings over the main screen, from `configuration` or
    /// empty fields when there is none, and the saved account.
    pub unsafe fn open(
        window: *mut Object,
        configuration: Option<&Configuration>,
        session: &Session,
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

            section(root, t!("settings.account"))?;
            let account_section = flex(root, FlexFlow::Column)?;
            let account_content = container(account_section)?;

            section(root, t!("settings.advanced"))?;
            caption(root, t!("settings.token"))?;
            let token_section = flex(root, FlexFlow::Row)?;
            let token_content = container(token_section)?;

            section(root, t!("settings.entities"))?;
            let entities_section = flex(root, FlexFlow::Column)?;
            let entities_content = container(entities_section)?;
            caption(root, t!("settings.entity_id"))?;
            let manual_field = field(root, "", 64)?;

            let mut settings = Self {
                window,
                root: root_page,
                draft,
                message,
                url,
                refresh,
                account_section,
                account_content,
                account: Account {
                    signed_in: session.credentials.clone(),
                    token: String::new(),
                    token_saved: session.long_lived.is_some(),
                    token_visible: false,
                },
                sign_in: None,
                token_section,
                token_content,
                token_field: core::ptr::null_mut(),
                token_toggle: core::ptr::null_mut(),
                token_toggle_label: core::ptr::null_mut(),
                entities_section,
                entities_content,
                manual_field,
                picker: None,
                actions,
            };
            settings.build_account()?;
            settings.build_token(!settings.account.token_saved)?;
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

    /// The line describing the account: signed in as a user, a token set, or
    /// not signed in.
    fn account_label(&self) -> String {
        if let Some(credentials) = &self.account.signed_in {
            match &credentials.name {
                Some(name) => t!("settings.signed_in", name = name.as_str()).to_string(),
                None => String::from(t!("settings.signed_in_no_name")),
            }
        } else if !self.account.token.is_empty() || self.account.token_saved {
            String::from(t!("settings.token_set_account"))
        } else {
            String::from(t!("settings.not_signed_in"))
        }
    }

    /// Account row: the state, a sign-in or sign-out button, and the sign-in
    /// form when one is in progress. What the form holds is kept across the
    /// rebuild.
    unsafe fn build_account(&mut self) -> wasm::Result<()> {
        // Read the sign-in form before it is deleted.
        let kept = match &self.sign_in {
            Some(sign_in) => unsafe {
                (
                    if sign_in.username.is_null() {
                        String::new()
                    } else {
                        field_text(sign_in.username).unwrap_or_default()
                    },
                    sign_in.password_text.clone(),
                    if sign_in.code.is_null() {
                        String::new()
                    } else {
                        field_text(sign_in.code).unwrap_or_default()
                    },
                )
            },
            None => (String::new(), String::new(), String::new()),
        };
        let (two_factor, flow_id) = match &self.sign_in {
            Some(sign_in) => (sign_in.two_factor, sign_in.flow_id.clone()),
            None => (false, String::new()),
        };
        unsafe {
            delete(self.account_content)?;
            self.actions.retain(|(_, action)| {
                !matches!(
                    action,
                    Action::SignIn | Action::SignOut | Action::SubmitSignIn | Action::CancelSignIn
                )
            });
            let content = flex(self.account_section, FlexFlow::Column)?;
            self.account_content = content;
            let row = flex(content, FlexFlow::Row)?;
            let text = label(row, &self.account_label(), FONT_MEDIUM)?;
            object_set_width(text, 0)?;
            object_set_flex_grow(text, 1)?;
            if self.account.signed_in.is_some()
                || self.account.token_saved
                || !self.account.token.is_empty()
            {
                self.actions.push((
                    button(row, t!("settings.sign_out"), OUTLINE)?,
                    Action::SignOut,
                ));
            } else if self.sign_in.is_none() {
                // While the sign-in form shows, its own buttons act.
                self.actions.push((
                    button(row, t!("settings.sign_in"), OUTLINE)?,
                    Action::SignIn,
                ));
            }
            if self.sign_in.is_some() {
                let form = flex(content, FlexFlow::Column)?;
                let (username, password, code) = if two_factor {
                    paragraph(form, t!("settings.two_factor_hint"), FONT_SMALL)?;
                    caption(form, t!("settings.code"))?;
                    (
                        core::ptr::null_mut(),
                        core::ptr::null_mut(),
                        field(form, &kept.2, 16)?,
                    )
                } else {
                    caption(form, t!("settings.username"))?;
                    let username = field(form, &kept.0, 64)?;
                    caption(form, t!("settings.password"))?;
                    let password = field(form, &mask(kept.1.chars().count()), 128)?;
                    (username, password, core::ptr::null_mut())
                };
                let buttons = flex(form, FlexFlow::Row)?;
                self.actions.push((
                    button(buttons, t!("settings.cancel"), OUTLINE)?,
                    Action::CancelSignIn,
                ));
                self.actions.push((
                    button(buttons, t!("settings.sign_in"), PRIMARY)?,
                    Action::SubmitSignIn,
                ));
                if let Some(sign_in) = &mut self.sign_in {
                    sign_in.flow_id = flow_id;
                    sign_in.two_factor = two_factor;
                    sign_in.username = username;
                    sign_in.password = password;
                    sign_in.password_text = kept.1;
                    sign_in.code = code;
                }
            }
        }
        Ok(())
    }

    /// Token row: "Token set" with a replace action, or the masked field.
    unsafe fn build_token(&mut self, editing: bool) -> wasm::Result<()> {
        unsafe {
            delete(self.token_content)?;
            self.actions.retain(|(_, action)| {
                !matches!(action, Action::ReplaceToken | Action::ToggleToken)
            });
            let content = flex(self.token_section, FlexFlow::Row)?;
            self.token_content = content;
            if editing {
                caption(self.token_section, t!("settings.set_token"))?;
                let field = field(
                    content,
                    &mask(self.account.token.chars().count()),
                    MAXIMUM_TOKEN_LENGTH as u32,
                )?;
                object_set_width(field, 0)?;
                object_set_flex_grow(field, 1)?;
                let (toggle, toggle_label) =
                    labelled_button(content, t!("settings.show"), OUTLINE)?;
                self.token_field = field;
                self.token_toggle = toggle;
                self.token_toggle_label = toggle_label;
                self.account.token_visible = false;
                self.actions.push((toggle, Action::ToggleToken));
            } else {
                let text = label(content, t!("settings.token_set"), FONT_MEDIUM)?;
                object_set_width(text, 0)?;
                object_set_flex_grow(text, 1)?;
                let replace = button(content, t!("settings.replace"), OUTLINE)?;
                self.token_field = core::ptr::null_mut();
                self.token_toggle = core::ptr::null_mut();
                self.token_toggle_label = core::ptr::null_mut();
                self.actions.push((replace, Action::ReplaceToken));
            }
        }
        Ok(())
    }

    unsafe fn toggle_token(&mut self) -> wasm::Result<()> {
        self.account.token_visible = !self.account.token_visible;
        unsafe {
            if self.account.token_visible {
                set_field(self.token_field, &self.account.token)?;
            } else {
                set_field(self.token_field, &mask(self.account.token.chars().count()))?;
            }
            let caption = if self.account.token_visible {
                t!("settings.hide")
            } else {
                t!("settings.show")
            };
            set_text(self.token_toggle_label, caption)?;
        }
        Ok(())
    }

    unsafe fn token_changed(&mut self) -> wasm::Result<()> {
        let shown = unsafe { field_text(self.token_field)? };
        if self.account.token_visible {
            self.account.token = shown;
            return Ok(());
        }
        let before = self.account.token.clone();
        apply_masked_edit(&mut self.account.token, &shown);
        let masked = mask(self.account.token.chars().count());
        if shown != masked || before != self.account.token {
            unsafe { set_field(self.token_field, &masked)? };
        }
        Ok(())
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
        unsafe {
            set_text(self.message, text)?;
            object_set_text_color(self.message, color, 0)?;
            set_hidden(self.message, text.is_empty())?;
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

    /// Shows a message and lets the display catch up before a blocking call.
    unsafe fn busy(&self, text: &str) -> wasm::Result<()> {
        unsafe { self.show_message(text, MUTED_COLOR)? };
        sleep(Duration::from_millis(30));
        Ok(())
    }

    /// Signs out: revokes the refresh token when there is one, and deletes
    /// the stored credentials or token.
    unsafe fn sign_out(&mut self, context: &mut Context<'_>) -> wasm::Result<()> {
        let endpoint = unsafe { self.endpoint()? }.ok();
        if let (Some(credentials), Some(endpoint)) = (&self.account.signed_in, endpoint) {
            let _ = auth::revoke(&mut Devices, &endpoint, &credentials.refresh_token);
        }
        if let Some(storage) = context.storage {
            if self.account.signed_in.is_some() {
                let _ = storage.remove_credentials();
            }
            if self.account.token_saved || !self.account.token.is_empty() {
                let _ = storage.remove_token();
            }
        }
        self.account.signed_in = None;
        self.account.token.clear();
        self.account.token_saved = false;
        self.sign_in = None;
        unsafe {
            self.build_account()?;
            self.build_token(true)
        }
    }

    /// Starts the login flow when needed, then submits the step the form
    /// shows: username and password, or a two-factor code.
    unsafe fn submit_sign_in(&mut self, context: &mut Context<'_>) -> wasm::Result<()> {
        let endpoint = match unsafe { self.endpoint()? } {
            Ok(endpoint) => endpoint,
            Err(message) => return unsafe { self.show_message(&message, ERROR_COLOR) },
        };
        unsafe { self.busy(t!("settings.signing_in"))? };
        // Start the flow once; its identifier then lives in the form.
        if self
            .sign_in
            .as_ref()
            .is_none_or(|sign_in| sign_in.flow_id.is_empty())
        {
            match auth::start_login_flow(&mut Devices, &endpoint) {
                Ok(FlowResult::Form { flow_id, .. }) => {
                    // Keep the form and what was typed in it; only the flow
                    // identifier is new.
                    match &mut self.sign_in {
                        Some(sign_in) => sign_in.flow_id = flow_id,
                        None => {
                            self.sign_in = Some(SignIn {
                                flow_id,
                                two_factor: false,
                                username: core::ptr::null_mut(),
                                password: core::ptr::null_mut(),
                                password_text: String::new(),
                                code: core::ptr::null_mut(),
                            });
                        }
                    }
                    unsafe { self.build_account()? };
                }
                Ok(FlowResult::Abort { reason }) => {
                    let text = match reason {
                        Some(reason) => login_error(&reason),
                        None => t!("settings.login_failed", reason = "abort").to_string(),
                    };
                    return unsafe { self.show_message(&text, ERROR_COLOR) };
                }
                Ok(_) => {
                    return unsafe {
                        self.show_message(&error_message(&Error::InvalidResponse), ERROR_COLOR)
                    };
                }
                Err(error) => {
                    return unsafe { self.show_message(&error_message(&error), ERROR_COLOR) };
                }
            }
        }
        // The step to submit, from the form fields.
        let (flow_id, body) = match &self.sign_in {
            Some(sign_in) if sign_in.two_factor => {
                let code = unsafe { field_text(sign_in.code)? };
                (sign_in.flow_id.clone(), auth::mfa_body(code.trim()))
            }
            Some(sign_in) => {
                let username = unsafe { field_text(sign_in.username)? };
                (
                    sign_in.flow_id.clone(),
                    auth::credentials_body(username.trim(), &sign_in.password_text),
                )
            }
            None => return Ok(()),
        };
        let result = auth::submit_login_flow(&mut Devices, &endpoint, &flow_id, &body);
        // The password does not stay in memory after its request.
        if let Some(sign_in) = &mut self.sign_in {
            sign_in.password_text.clear();
            if !sign_in.password.is_null() {
                unsafe { set_field(sign_in.password, "")? };
            }
        }
        match result {
            Ok(FlowResult::CreateEntry { code, title }) => {
                match auth::exchange_code(&mut Devices, &endpoint, &code) {
                    Ok(set) => match set.refresh_token {
                        Some(refresh_token) => match Credentials::new(&refresh_token, title) {
                            Ok(credentials) => {
                                // The session gets the credentials and the access token right
                                // away, so the picker can query the server before saving.
                                context.session.credentials = Some(credentials.clone());
                                context.session.set_access_token(
                                    &set.access_token,
                                    set.expires_in,
                                    Instant::now(),
                                );
                                self.account.signed_in = Some(credentials);
                                self.account.token.clear();
                                self.account.token_saved = false;
                                self.sign_in = None;
                                unsafe {
                                    self.build_account()?;
                                    self.build_token(false)?;
                                    self.show_message(t!("settings.saved"), SUCCESS_COLOR)
                                }
                            }
                            Err(error) => unsafe {
                                self.show_message(&error_message(&error), ERROR_COLOR)
                            },
                        },
                        None => unsafe {
                            self.show_message(&error_message(&Error::InvalidResponse), ERROR_COLOR)
                        },
                    },
                    Err(error) => unsafe { self.show_message(&error_message(&error), ERROR_COLOR) },
                }
            }
            Ok(FlowResult::Form {
                flow_id,
                step_id,
                errors,
            }) => {
                let two_factor = step_id == "mfa";
                if let Some(sign_in) = &mut self.sign_in {
                    sign_in.flow_id = flow_id;
                    sign_in.two_factor = two_factor;
                }
                unsafe { self.build_account()? };
                let text = if two_factor {
                    String::from(t!("settings.two_factor_hint"))
                } else {
                    let reason = errors
                        .iter()
                        .find(|(key, _)| key == "base")
                        .map(|(_, message)| message.as_str())
                        .unwrap_or("invalid_auth");
                    login_error(reason)
                };
                unsafe {
                    let color = if two_factor { MUTED_COLOR } else { ERROR_COLOR };
                    self.show_message(&text, color)
                }
            }
            Ok(FlowResult::Abort { reason }) => {
                let text = match reason {
                    Some(reason) => login_error(&reason),
                    None => t!("settings.login_failed", reason = "abort").to_string(),
                };
                unsafe { self.show_message(&text, ERROR_COLOR) }
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
        let token = match context
            .session
            .access_token(&mut Devices, &endpoint, Instant::now())
        {
            Ok(token) => token,
            Err(error) => return unsafe { self.show_message(&error_message(&error), ERROR_COLOR) },
        };
        unsafe {
            set_hidden(self.root, true)?;
            sleep(Duration::from_millis(30));
            let Page {
                root: picker_root,
                bar,
                message: _,
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
                areas: Vec::new(),
                switches: Vec::new(),
            });
            match api::render_template(&mut Devices, &endpoint, &token, &picker::areas_template()) {
                Ok(text) => {
                    let areas = picker::parse_entries(&text, false);
                    if let Some(picker) = &mut self.picker {
                        picker.areas = areas;
                    }
                    self.build_picker_areas()
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
    unsafe fn build_picker_areas(&mut self) -> wasm::Result<()> {
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
        let token = match context
            .session
            .access_token(&mut Devices, &endpoint, Instant::now())
        {
            Ok(token) => token,
            Err(error) => return unsafe { self.show_message(&error_message(&error), ERROR_COLOR) },
        };
        unsafe { self.busy(t!("settings.loading"))? };
        let answer = api::render_template(&mut Devices, &endpoint, &token, template);
        let entries = match answer {
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
            if entries.is_empty() {
                paragraph(content, t!("settings.no_entities_in_area"), FONT_SMALL)?;
            }
            for entry in &entries {
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

    /// Adds or removes an entity of the picker in the draft.
    fn toggle_picker_entity(&mut self, identifier: &str, checked: bool) {
        if checked {
            if self.draft.entities.len() >= MAXIMUM_ENTITIES {
                return;
            }
            if !self
                .draft
                .entities
                .iter()
                .any(|entity| entity == identifier)
            {
                self.draft.entities.push(String::from(identifier));
            }
        } else {
            self.draft.entities.retain(|entity| entity != identifier);
        }
    }

    /// Saves the draft: the configuration file, and the credentials or token
    /// file of the account, each written atomically.
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
            if self.account.signed_in.is_none()
                && self.account.token.is_empty()
                && !self.account.token_saved
            {
                self.show_message(t!("errors.missing_token"), ERROR_COLOR)?;
                return Ok(Outcome::Stay);
            }
            if let Some(credentials) = &self.account.signed_in {
                if let Err(error) = storage.save_credentials(credentials) {
                    self.show_message(&error_message(&error), ERROR_COLOR)?;
                    return Ok(Outcome::Stay);
                }
                let _ = storage.remove_token();
            } else if !self.account.token.is_empty() {
                if let Err(error) = storage.save_token(&self.account.token) {
                    let text = match error {
                        Error::MissingToken => String::from(t!("errors.missing_token")),
                        error => error_message(&error),
                    };
                    self.show_message(&text, ERROR_COLOR)?;
                    return Ok(Outcome::Stay);
                }
                let _ = storage.remove_credentials();
            } else {
                // The saved token is kept: only the credentials file goes.
                let _ = storage.remove_credentials();
            }
            if let Err(error) = storage.save(&self.draft) {
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
                if !self.token_field.is_null() && target == self.token_field {
                    self.token_changed()?;
                } else if let Some(sign_in) = &self.sign_in
                    && !sign_in.password.is_null()
                    && target == sign_in.password
                {
                    let shown = field_text(target)?;
                    let sign_in = self.sign_in.as_mut().unwrap();
                    apply_masked_edit(&mut sign_in.password_text, &shown);
                    set_field(target, &mask(sign_in.password_text.chars().count()))?;
                } else if let Some(picker) = &self.picker
                    && let Some((_, identifier)) =
                        picker.switches.iter().find(|(switch, _)| *switch == target)
                {
                    let identifier = identifier.clone();
                    let checked = is_checked(target)?;
                    self.toggle_picker_entity(&identifier, checked);
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
                Action::SignIn => {
                    self.sign_in = Some(SignIn {
                        flow_id: String::new(),
                        two_factor: false,
                        username: core::ptr::null_mut(),
                        password: core::ptr::null_mut(),
                        password_text: String::new(),
                        code: core::ptr::null_mut(),
                    });
                    self.build_account()?;
                }
                Action::SubmitSignIn => self.submit_sign_in(context)?,
                Action::CancelSignIn => {
                    self.sign_in = None;
                    self.build_account()?;
                }
                Action::SignOut => self.sign_out(context)?,
                Action::ReplaceToken => self.build_token(true)?,
                Action::ToggleToken => self.toggle_token()?,
                Action::AddManual => {
                    let text = field_text(self.manual_field)?.trim().to_string();
                    if is_valid_entity_id(&text)
                        && !self.draft.entities.iter().any(|entity| entity == &text)
                        && self.draft.entities.len() < MAXIMUM_ENTITIES
                    {
                        self.draft.entities.push(text);
                        set_field(self.manual_field, "")?;
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
        }
        Ok(Outcome::Stay)
    }
}

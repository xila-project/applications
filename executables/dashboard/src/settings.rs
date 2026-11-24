//! Settings screen: connection, token and panels, with a panel editor.
//!
//! It edits a draft of the configuration and writes the same files as the
//! serial-shell route when saved.

use dashboard::{
    configuration::{
        Configuration, DEFAULT_REFRESH_SECONDS, Invalid, Kind, MAXIMUM_EXPRESSION_LENGTH,
        MAXIMUM_PANELS, MAXIMUM_TITLE_LENGTH, Panel, RANGES, is_valid_uid, number_text,
        parse_optional_number, validate_url,
    },
    error::Error,
    format::{MAXIMUM_DECIMALS, Unit, format_value},
    grafana,
    model::Reload,
    net::device::Devices,
    storage::{MAXIMUM_TOKEN_LENGTH, Storage},
    text::{
        apply_masked_edit, error_message, invalid_message, kind_name, mask, range_name, unit_name,
    },
};
use embedded_i18n::t;
use std::{thread::sleep, time::Duration};
use wasm::*;

use crate::widgets::*;

const ERROR_COLOR: u32 = 0xEF4444;
const SUCCESS_COLOR: u32 = 0x22C55E;
const MUTED_COLOR: u32 = 0x71717A;

pub struct Context<'a> {
    pub storage: Option<&'a Storage>,
    /// The saved token, if any.
    pub token: Option<&'a str>,
}

pub enum Outcome {
    Stay,
    Close,
    Saved(Box<Configuration>),
}

#[derive(Clone, Copy, PartialEq)]
enum Action {
    Save,
    Cancel,
    Test,
    ReplaceToken,
    ToggleToken,
    Reload,
    Convert,
    Add,
    Edit(usize),
    Delete(usize),
    EditorDone,
    EditorCancel,
}

struct Editor {
    root: *mut Object,
    index: Option<usize>,
    title: *mut Object,
    expression: *mut Object,
    unit: *mut Object,
    decimals: *mut Object,
    warning: *mut Object,
    critical: *mut Object,
    kind: *mut Object,
    range_row: *mut Object,
    range: *mut Object,
    message: *mut Object,
}

pub struct Settings {
    window: *mut Object,
    root: *mut Object,
    draft: Configuration,
    message: *mut Object,
    url: *mut Object,
    data_source: *mut Object,
    refresh: *mut Object,
    dashboard: *mut Object,
    /// Holds `token_content`, which is rebuilt.
    token_section: *mut Object,
    token_content: *mut Object,
    token_field: *mut Object,
    token_toggle: *mut Object,
    token_toggle_label: *mut Object,
    /// Token typed in this session; it is never read back from the file.
    token: String,
    token_visible: bool,
    token_saved: bool,
    /// Holds `panels_content`, which is rebuilt.
    panels_section: *mut Object,
    panels_content: *mut Object,
    /// Switches showing imported panels, with their hiding key.
    switches: Vec<(*mut Object, String)>,
    actions: Vec<(*mut Object, Action)>,
    editor: Option<Editor>,
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
    /// fields when there is none.
    pub unsafe fn open(
        window: *mut Object,
        configuration: Option<&Configuration>,
        token_saved: bool,
    ) -> wasm::Result<Self> {
        let draft = configuration.cloned().unwrap_or_else(|| Configuration {
            url: String::from("https://"),
            refresh_seconds: DEFAULT_REFRESH_SECONDS,
            ..Configuration::default()
        });
        unsafe {
            let Page {
                root,
                bar,
                message,
                body,
            } = page(window, t!("settings.title"))?;
            let mut actions = Vec::new();
            actions.push((button(bar, t!("settings.cancel"), OUTLINE)?, Action::Cancel));
            actions.push((button(bar, t!("settings.save"), PRIMARY)?, Action::Save));
            let root_page = root;
            let root = body;

            section(root, t!("settings.connection"))?;
            caption(root, t!("settings.url"))?;
            let url = field(root, &draft.url, 256)?;
            caption(root, t!("settings.data_source"))?;
            let data_source = field(root, &draft.data_source, 64)?;
            caption(root, t!("settings.refresh"))?;
            let refresh = field(root, &draft.refresh_seconds.to_string(), 6)?;
            caption(root, t!("settings.dashboard"))?;
            let dashboard = field(root, draft.dashboard.as_deref().unwrap_or(""), 64)?;
            actions.push((button(root, t!("settings.test"), OUTLINE)?, Action::Test));

            section(root, t!("settings.token"))?;
            let token_section = flex(root, FlexFlow::Row)?;
            let token_content = container(token_section)?;
            section(root, t!("settings.panels"))?;
            let panels_section = flex(root, FlexFlow::Column)?;
            let panels_content = container(panels_section)?;

            let mut settings = Self {
                window,
                root: root_page,
                draft,
                message,
                url,
                data_source,
                refresh,
                dashboard,
                token_section,
                token_content,
                token_field: core::ptr::null_mut(),
                token_toggle: core::ptr::null_mut(),
                token_toggle_label: core::ptr::null_mut(),
                token: String::new(),
                token_visible: false,
                token_saved,
                panels_section,
                panels_content,
                switches: Vec::new(),
                actions,
                editor: None,
            };
            settings.build_token(!token_saved)?;
            settings.build_panels()?;
            Ok(settings)
        }
    }

    pub unsafe fn close(self) -> wasm::Result<()> {
        unsafe {
            if let Some(editor) = &self.editor {
                delete(editor.root)?;
            }
            delete(self.root)
        }
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
                let field = field(
                    content,
                    &mask(self.token.chars().count()),
                    MAXIMUM_TOKEN_LENGTH as u32,
                )?;
                object_set_width(field, 0)?;
                object_set_flex_grow(field, 1)?;
                let (toggle, toggle_label) =
                    labelled_button(content, t!("settings.show"), OUTLINE)?;
                self.token_field = field;
                self.token_toggle = toggle;
                self.token_toggle_label = toggle_label;
                self.token_visible = false;
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
        self.token_visible = !self.token_visible;
        unsafe {
            if self.token_visible {
                set_field(self.token_field, &self.token)?;
            } else {
                set_field(self.token_field, &mask(self.token.chars().count()))?;
            }
            let caption = if self.token_visible {
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
        if self.token_visible {
            self.token = shown;
            return Ok(());
        }
        let before = self.token.clone();
        apply_masked_edit(&mut self.token, &shown);
        let masked = mask(self.token.chars().count());
        if shown != masked || before != self.token {
            unsafe { set_field(self.token_field, &masked)? };
        }
        Ok(())
    }

    /// Panel list: the dashboard's panels with a visibility switch, or the
    /// manual panels with edit and delete actions.
    unsafe fn build_panels(&mut self) -> wasm::Result<()> {
        unsafe {
            delete(self.panels_content)?;
            self.actions.retain(|(_, action)| {
                !matches!(
                    action,
                    Action::Reload
                        | Action::Convert
                        | Action::Add
                        | Action::Edit(_)
                        | Action::Delete(_)
                )
            });
            self.switches.clear();
            self.panels_content = flex(self.panels_section, FlexFlow::Column)?;
            let section = self.panels_content;
            if self.draft.dashboard.is_some() {
                let summary = match self.draft.current_import() {
                    Some(imported) => t!(
                        "settings.imported",
                        count = imported.panels.len(),
                        title = imported.title,
                        skipped = imported.skipped
                    )
                    .to_string(),
                    None => String::from(t!("settings.not_imported")),
                };
                paragraph(section, &summary, FONT_SMALL)?;
                let buttons = flex(section, FlexFlow::RowWrap)?;
                self.actions.push((
                    button(buttons, t!("settings.reload"), OUTLINE)?,
                    Action::Reload,
                ));
                self.actions.push((
                    button(buttons, t!("settings.convert"), OUTLINE)?,
                    Action::Convert,
                ));
                let panels: Vec<(String, String, bool)> = self
                    .draft
                    .current_import()
                    .map(|imported| {
                        imported
                            .panels
                            .iter()
                            .map(|panel| {
                                (
                                    panel.title.clone(),
                                    String::from(panel.hiding_key()),
                                    !self.draft.is_hidden(panel),
                                )
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                for (title, key, visible) in panels {
                    let row = flex(section, FlexFlow::Row)?;
                    let name = label(row, &title, FONT_MEDIUM)?;
                    object_set_width(name, 0)?;
                    object_set_flex_grow(name, 1)?;
                    let switch = switch_create(row)?;
                    set_checked(switch, visible)?;
                    self.switches.push((switch, key));
                }
            } else {
                for index in 0..self.draft.panels.len() {
                    let row = flex(section, FlexFlow::Row)?;
                    let name = label(row, &self.draft.panels[index].title, FONT_MEDIUM)?;
                    object_set_width(name, 0)?;
                    object_set_flex_grow(name, 1)?;
                    self.actions.push((
                        button(row, t!("settings.edit"), OUTLINE)?,
                        Action::Edit(index),
                    ));
                    self.actions.push((
                        button(row, t!("settings.delete"), DESTRUCTIVE)?,
                        Action::Delete(index),
                    ));
                }
                if self.draft.panels.len() < MAXIMUM_PANELS {
                    self.actions
                        .push((button(section, t!("settings.add"), OUTLINE)?, Action::Add));
                }
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
            self.draft.data_source = field_text(self.data_source)?.trim().into();
            let dashboard: String = field_text(self.dashboard)?.trim().into();
            self.draft.dashboard = (!dashboard.is_empty()).then_some(dashboard);
            match field_text(self.refresh)?.trim().parse::<u32>() {
                Ok(seconds) => self.draft.refresh_seconds = seconds,
                Err(_) => return Ok(Err(Invalid::Refresh)),
            }
        }
        Ok(Ok(()))
    }

    /// Checks what a request needs: the URL, the data source and a token.
    fn connection<'a>(&'a self, context: &'a Context<'_>) -> core::result::Result<&'a str, String> {
        validate_url(&self.draft.url).map_err(|invalid| String::from(invalid_message(invalid)))?;
        if self.draft.data_source.is_empty() {
            return Err(String::from(invalid_message(Invalid::DataSource)));
        }
        if !self.token.is_empty() {
            Ok(&self.token)
        } else {
            context
                .token
                .ok_or_else(|| error_message(&Error::MissingToken))
        }
    }

    unsafe fn busy(&self, text: &str) -> wasm::Result<()> {
        unsafe { self.show_message(text, MUTED_COLOR)? };
        sleep(Duration::from_millis(30));
        Ok(())
    }

    unsafe fn test(&mut self, context: &Context<'_>) -> wasm::Result<()> {
        unsafe {
            if let Err(invalid) = self.read_fields()? {
                return self.show_message(invalid_message(invalid), ERROR_COLOR);
            }
            let token = match self.connection(context) {
                Ok(token) => String::from(token),
                Err(message) => return self.show_message(&message, ERROR_COLOR),
            };
            self.busy(t!("settings.testing"))?;
            let panel = self
                .draft
                .visible_panels()
                .first()
                .map(|panel| (*panel).clone());
            match grafana::test_connection(&mut Devices, &self.draft, &token, panel.as_ref()) {
                Ok((panel, data)) => {
                    let value = data
                        .series
                        .first()
                        .and_then(|series| series.last)
                        .map_or_else(
                            || String::from("-"),
                            |value| format_value(value, panel.unit(), panel.decimals),
                        );
                    let text =
                        t!("settings.connected", title = panel.title, value = value).to_string();
                    self.show_message(&text, SUCCESS_COLOR)
                }
                Err(error) => self.show_message(&error_message(&error), ERROR_COLOR),
            }
        }
    }

    unsafe fn reload(&mut self, context: &Context<'_>) -> wasm::Result<()> {
        unsafe {
            if let Err(invalid) = self.read_fields()? {
                return self.show_message(invalid_message(invalid), ERROR_COLOR);
            }
            if !self.draft.dashboard.as_deref().is_some_and(is_valid_uid) {
                return self.show_message(invalid_message(Invalid::Dashboard), ERROR_COLOR);
            }
            let token = match self.connection(context) {
                Ok(token) => String::from(token),
                Err(message) => return self.show_message(&message, ERROR_COLOR),
            };
            self.busy(t!("settings.importing"))?;
            let result = grafana::import(&mut Devices, &self.draft, &token);
            match self.draft.apply_import(result) {
                Reload::Imported => self.show_message("", MUTED_COLOR)?,
                Reload::Failed(error) => self.show_message(&error_message(&error), ERROR_COLOR)?,
            }
            self.build_panels()
        }
    }

    unsafe fn save(&mut self, context: &Context<'_>) -> wasm::Result<Outcome> {
        unsafe {
            if let Err(invalid) = self.read_fields()?.and_then(|()| self.draft.validate()) {
                self.show_message(invalid_message(invalid), ERROR_COLOR)?;
                return Ok(Outcome::Stay);
            }
            let Some(storage) = context.storage else {
                self.show_message(&error_message(&Error::Storage), ERROR_COLOR)?;
                return Ok(Outcome::Stay);
            };
            if self.token.is_empty() && !self.token_saved {
                self.show_message(t!("invalid.token"), ERROR_COLOR)?;
                return Ok(Outcome::Stay);
            }
            if !self.token.is_empty()
                && let Err(error) = storage.save_token(&self.token)
            {
                let text = match error {
                    Error::MissingToken => String::from(t!("invalid.token")),
                    error => error_message(&error),
                };
                self.show_message(&text, ERROR_COLOR)?;
                return Ok(Outcome::Stay);
            }
            if let Err(error) = storage.save(&self.draft) {
                self.show_message(&error_message(&error), ERROR_COLOR)?;
                return Ok(Outcome::Stay);
            }
        }
        Ok(Outcome::Saved(Box::new(self.draft.clone())))
    }

    unsafe fn open_editor(&mut self, index: Option<usize>) -> wasm::Result<()> {
        let panel = index
            .and_then(|index| self.draft.panels.get(index).cloned())
            .unwrap_or_else(|| Panel {
                kind: Some(Kind::Value),
                ..Panel::new()
            });
        unsafe {
            set_hidden(self.root, true)?;
            let heading = if index.is_some() {
                t!("editor.edit")
            } else {
                t!("editor.new")
            };
            let Page {
                root: page_root,
                bar,
                message,
                body: root,
            } = page(self.window, heading)?;
            self.actions.push((
                button(bar, t!("settings.cancel"), OUTLINE)?,
                Action::EditorCancel,
            ));
            self.actions
                .push((button(bar, t!("editor.done"), PRIMARY)?, Action::EditorDone));
            object_set_text_color(message, ERROR_COLOR, 0)?;
            caption(root, t!("editor.title"))?;
            let title = field(root, &panel.title, MAXIMUM_TITLE_LENGTH as u32)?;
            caption(root, t!("editor.expression"))?;
            let expression = field(root, &panel.expression, MAXIMUM_EXPRESSION_LENGTH as u32)?;
            caption(root, t!("editor.unit"))?;
            let units: Vec<&str> = Unit::ALL.iter().map(|unit| unit_name(*unit)).collect();
            let unit = dropdown(root, &units, panel.unit().index())?;
            caption(root, t!("editor.decimals"))?;
            let decimals = field(
                root,
                &panel
                    .decimals
                    .map(|value| value.to_string())
                    .unwrap_or_default(),
                1,
            )?;
            caption(root, t!("editor.warning"))?;
            let warning = field(root, &number_text(panel.warning), 24)?;
            caption(root, t!("editor.critical"))?;
            let critical = field(root, &number_text(panel.critical), 24)?;
            caption(root, t!("editor.kind"))?;
            let kinds = [kind_name(Kind::Value), kind_name(Kind::Chart)];
            let kind = dropdown(root, &kinds, usize::from(panel.kind() == Kind::Chart))?;
            let range_row = flex(root, FlexFlow::Column)?;
            caption(range_row, t!("editor.range"))?;
            let names: Vec<String> = RANGES.iter().map(|seconds| range_name(*seconds)).collect();
            let names: Vec<&str> = names.iter().map(String::as_str).collect();
            let selected = RANGES
                .iter()
                .position(|seconds| *seconds == panel.range_seconds())
                .unwrap_or(1);
            let range = dropdown(range_row, &names, selected)?;
            set_hidden(range_row, panel.kind() != Kind::Chart)?;
            self.editor = Some(Editor {
                root: page_root,
                index,
                title,
                expression,
                unit,
                decimals,
                warning,
                critical,
                kind,
                range_row,
                range,
                message,
            });
        }
        Ok(())
    }

    unsafe fn close_editor(&mut self) -> wasm::Result<()> {
        if let Some(editor) = self.editor.take() {
            self.actions
                .retain(|(_, action)| !matches!(action, Action::EditorDone | Action::EditorCancel));
            unsafe {
                delete(editor.root)?;
                set_hidden(self.root, false)?;
            }
        }
        Ok(())
    }

    /// Reads the editor into a panel, or the reason it is not valid.
    unsafe fn editor_panel(
        editor: &Editor,
        previous: Option<&Panel>,
    ) -> wasm::Result<core::result::Result<Panel, Invalid>> {
        unsafe {
            let decimals = field_text(editor.decimals)?;
            let decimals = match decimals.trim() {
                "" => None,
                text => match text.parse::<u8>() {
                    Ok(value) if value <= MAXIMUM_DECIMALS => Some(value),
                    _ => return Ok(Err(Invalid::Decimals)),
                },
            };
            let (Ok(warning), Ok(critical)) = (
                parse_optional_number(&field_text(editor.warning)?),
                parse_optional_number(&field_text(editor.critical)?),
            ) else {
                return Ok(Err(Invalid::Threshold));
            };
            let kind = if dropdown_get_selected(editor.kind)? == 1 {
                Kind::Chart
            } else {
                Kind::Value
            };
            let range = RANGES
                .get(dropdown_get_selected(editor.range)? as usize)
                .copied()
                .unwrap_or(RANGES[1]);
            let panel = Panel {
                key: previous.and_then(|panel| panel.key.clone()),
                title: field_text(editor.title)?.trim().into(),
                expression: field_text(editor.expression)?.trim().into(),
                unit: Some(
                    Unit::ALL
                        .get(dropdown_get_selected(editor.unit)? as usize)
                        .copied()
                        .unwrap_or_default(),
                ),
                decimals,
                warning,
                critical,
                kind: Some(kind),
                range_seconds: (kind == Kind::Chart).then_some(range),
            };
            Ok(panel.validate().map(|()| panel))
        }
    }

    unsafe fn editor_done(&mut self) -> wasm::Result<()> {
        let Some(editor) = &self.editor else {
            return Ok(());
        };
        let previous = editor.index.and_then(|index| self.draft.panels.get(index));
        match unsafe { Self::editor_panel(editor, previous)? } {
            Err(invalid) => unsafe {
                set_text(editor.message, invalid_message(invalid))?;
                set_hidden(editor.message, false)
            },
            Ok(panel) => {
                match editor.index {
                    Some(index) if index < self.draft.panels.len() => {
                        self.draft.panels[index] = panel
                    }
                    _ => self.draft.panels.push(panel),
                }
                unsafe {
                    self.close_editor()?;
                    self.build_panels()
                }
            }
        }
    }

    /// Handles one event; returns whether the settings stay open.
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
                } else if let Some((_, key)) =
                    self.switches.iter().find(|(switch, _)| *switch == target)
                {
                    let key = key.clone();
                    let visible = is_checked(target)?;
                    self.draft.set_hidden(&key, !visible);
                } else if let Some(editor) = &self.editor
                    && target == editor.kind
                {
                    set_hidden(editor.range_row, dropdown_get_selected(editor.kind)? != 1)?;
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
                .map(|(_, action)| *action)
            else {
                return Ok(Outcome::Stay);
            };
            match action {
                Action::Save => return self.save(context),
                Action::Cancel => return Ok(Outcome::Close),
                Action::Test => self.test(context)?,
                Action::ReplaceToken => self.build_token(true)?,
                Action::ToggleToken => self.toggle_token()?,
                Action::Reload => self.reload(context)?,
                Action::Convert => {
                    self.draft.convert_to_manual();
                    set_field(self.dashboard, "")?;
                    self.build_panels()?;
                }
                Action::Add => self.open_editor(None)?,
                Action::Edit(index) => self.open_editor(Some(index))?,
                Action::Delete(index) => {
                    if index < self.draft.panels.len() {
                        self.draft.panels.remove(index);
                    }
                    self.build_panels()?;
                }
                Action::EditorDone => self.editor_done()?,
                Action::EditorCancel => self.close_editor()?,
            }
        }
        Ok(Outcome::Stay)
    }
}

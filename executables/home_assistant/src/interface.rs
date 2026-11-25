//! Main screen: a header with the title, the time since the last refresh and
//! the errors, then a grid of entity cards. Each card shows one configured
//! entity and the controls its domain has.

use core::ptr::null_mut;
use std::{
    thread::sleep,
    time::{Duration, Instant},
};

use embedded_i18n::t;
use home_assistant::{
    api,
    configuration::{self, Configuration, Invalid, domain_of},
    error::Error,
    net::{Endpoint, device::Devices},
    service,
    state::{Control, Domain, EntityState, controls},
    storage::Storage,
    text::{age, error_message, invalid_message},
};
use wasm::*;

use crate::widgets::*;

include!(concat!(env!("OUT_DIR"), "/shortcut_icon.rs"));

/// Windows narrower than this show two cards per row instead of three.
const WIDE_MINIMUM_WIDTH: i32 = 400;
const ERROR_COLOR: u32 = 0xEF4444;
const MUTED_COLOR: u32 = 0x71717A;

/// A card button and the control it calls.
struct CardButton {
    object: *mut Object,
    caption: *mut Object,
    control: Control,
}

/// One entity card.
struct Card {
    name: *mut Object,
    media: *mut Object,
    value: *mut Object,
    error: *mut Object,
    buttons: Vec<CardButton>,
}

pub struct Interface {
    window: *mut Object,
    columns: i32,
    storage: Option<Storage>,
    configuration: Option<Configuration>,
    token: Option<String>,
    title: *mut Object,
    status: *mut Object,
    refresh_button: *mut Object,
    notice: *mut Object,
    grid: *mut Object,
    cards: Vec<Card>,
    entities: Vec<String>,
    states: Vec<Option<EntityState>>,
    errors: Vec<Option<Error>>,
    last_refresh: Option<Instant>,
    next_refresh: Option<Instant>,
    /// Error of the last refresh.
    error: Option<Error>,
    /// Error of the last service call. It stays visible across the refresh
    /// that follows the call, and clears on the next successful refresh.
    action_error: Option<Error>,
    /// Set when a close request arrived, including one drained after a
    /// refresh: the host sends it only once, so it must not be lost.
    close_requested: bool,
    /// Problem found while reading the files at start.
    startup_error: Option<Error>,
    /// Which configuration field failed validation at start.
    startup_invalid: Option<Invalid>,
    shown_status: String,
}

impl Interface {
    pub fn new() -> wasm::Result<Self> {
        unsafe {
            let window = window_create()?;
            window_set_icon(window, ICON_TEXT.as_ptr(), ICON_COLOR)?;
            object_set_flex_flow(window, FlexFlow::Column)?;
            object_remove_flag(window, ObjectFlag::Scrollable)?;
            object_update_layout(window)?;
            let columns = if object_get_width(window)? < WIDE_MINIMUM_WIDTH {
                2
            } else {
                3
            };
            object_set_padding(window, 4)?;

            let header = flex(window, FlexFlow::Row)?;
            object_set_padding(header, 2)?;
            let title = label(header, t!("header.title"), FONT_LARGE)?;
            object_set_width(title, 0)?;
            object_set_flex_grow(title, 1)?;
            let status = label(header, "", FONT_SMALL)?;
            object_set_text_color(status, MUTED_COLOR, 0)?;
            let refresh_button = button(header, SYMBOL_REFRESH, GHOST)?;

            let notice = paragraph(window, "", FONT_SMALL)?;
            object_set_text_color(notice, ERROR_COLOR, 0)?;
            set_hidden(notice, true)?;

            let grid = Self::create_grid(window)?;

            Ok(Self {
                window,
                columns,
                storage: Storage::from_environment(),
                configuration: None,
                token: None,
                title,
                status,
                refresh_button,
                notice,
                grid,
                cards: Vec::new(),
                entities: Vec::new(),
                states: Vec::new(),
                errors: Vec::new(),
                last_refresh: None,
                next_refresh: None,
                error: None,
                action_error: None,
                close_requested: false,
                startup_error: None,
                startup_invalid: None,
                shown_status: String::new(),
            })
        }
    }

    unsafe fn create_grid(window: *mut Object) -> wasm::Result<*mut Object> {
        unsafe {
            let grid = container(window)?;
            object_set_width(grid, percentage(100)?)?;
            object_set_flex_grow(grid, 1)?;
            object_set_flex_flow(grid, FlexFlow::RowWrap)?;
            object_set_flex_align(grid, FlexAlign::Start, FlexAlign::Start, FlexAlign::Start)?;
            object_add_flag(grid, ObjectFlag::Scrollable)?;
            Ok(grid)
        }
    }

    /// Reads the files, builds the cards and shows the first states.
    fn start(&mut self) -> wasm::Result<()> {
        match &self.storage {
            None => self.startup_error = Some(Error::Storage),
            Some(storage) => {
                match storage.load() {
                    Ok(configuration) => self.configuration = configuration,
                    Err(error) => self.startup_error = Some(error),
                }
                match storage.token() {
                    Ok(token) => self.token = token,
                    Err(error) => {
                        // A configuration error shows first when both fail.
                        if self.startup_error.is_none() {
                            self.startup_error = Some(error);
                        }
                    }
                }
            }
        }
        let invalid = self
            .configuration
            .as_ref()
            .and_then(|configuration| configuration.validate().err());
        if let Some(invalid) = invalid {
            self.startup_invalid = Some(invalid);
            self.configuration = None;
        }
        if self.configuration.is_some() && self.token.is_none() && self.startup_error.is_none() {
            self.startup_error = Some(Error::MissingToken);
        }
        if self.configuration.is_none() {
            return self.show_notice();
        }
        self.rebuild()?;
        self.refresh()
    }

    /// Shows `text` in the status line right away, before a blocking call.
    fn show_busy(&mut self, text: &str) -> wasm::Result<()> {
        unsafe {
            set_text(self.status, text)?;
            object_set_text_color(self.status, MUTED_COLOR, 0)?;
        }
        self.shown_status = String::from(text);
        // Let the display catch up before the guest blocks on the network.
        sleep(Duration::from_millis(30));
        Ok(())
    }

    /// Recreates the cards from the configuration.
    fn rebuild(&mut self) -> wasm::Result<()> {
        unsafe {
            delete(self.grid)?;
            self.grid = Self::create_grid(self.window)?;
        }
        self.cards.clear();
        self.entities = self
            .configuration
            .as_ref()
            .map(|configuration| configuration.entities.clone())
            .unwrap_or_default();
        self.states = self.entities.iter().map(|_| None).collect();
        self.errors = self.entities.iter().map(|_| None).collect();
        for index in 0..self.entities.len() {
            let card = unsafe { self.create_card(index)? };
            self.cards.push(card);
        }
        unsafe {
            set_text(self.title, t!("header.title"))?;
        }
        self.show_notice()
    }

    unsafe fn create_card(&self, index: usize) -> wasm::Result<Card> {
        let entity_id = &self.entities[index];
        let domain = Domain::parse(entity_id);
        unsafe {
            let card = card_create(self.grid)?;
            // A media player card spans the row: it holds five controls.
            let width = match (self.columns, domain) {
                (_, Domain::MediaPlayer) => 97,
                (3, _) => 31,
                (_, _) => 48,
            };
            object_set_size(card, percentage(width)?, size_content())?;
            object_set_flex_flow(card, FlexFlow::Column)?;
            object_set_padding(card, 6)?;
            object_remove_flag(card, ObjectFlag::Scrollable)?;
            object_set_min_max_size(card, 0, 16_384, 84, 16_384, 0)?;
            let name = paragraph(card, entity_id, FONT_SMALL)?;
            let media = paragraph(card, "", FONT_SMALL)?;
            object_set_text_color(media, MUTED_COLOR, 0)?;
            set_hidden(media, true)?;
            let value = paragraph(card, "-", FONT_LARGE)?;
            let buttons_row = flex(card, FlexFlow::Row)?;
            let mut buttons = Vec::new();
            for &control in controls(domain) {
                let caption = match control {
                    Control::Toggle => String::from(t!("controls.toggle")),
                    Control::TurnOn => String::from(t!("controls.turn_on")),
                    Control::PlayPause => String::from(SYMBOL_PLAY),
                    Control::NextTrack => String::from(SYMBOL_NEXT),
                    Control::PreviousTrack => String::from(SYMBOL_PREVIOUS),
                    Control::VolumeUp => String::from("+"),
                    Control::VolumeDown => String::from("-"),
                };
                let variant = if matches!(control, Control::Toggle | Control::TurnOn) {
                    PRIMARY
                } else {
                    GHOST
                };
                let (object, caption_object) = labelled_button(buttons_row, &caption, variant)?;
                buttons.push(CardButton {
                    object,
                    caption: caption_object,
                    control,
                });
            }
            set_hidden(buttons_row, buttons.is_empty())?;
            let error = paragraph(card, "", FONT_SMALL)?;
            object_set_text_color(error, ERROR_COLOR, 0)?;
            set_hidden(error, true)?;
            Ok(Card {
                name,
                media,
                value,
                error,
                buttons,
            })
        }
    }

    /// Reads every entity state and updates the cards, then discards the
    /// events queued while the guest waited on the network, so a tap made
    /// during the refresh does not start another one right away.
    fn refresh(&mut self) -> wasm::Result<()> {
        self.refresh_inner(false)
    }

    /// The refresh that directly follows an action: the action error stays
    /// visible through it.
    fn refresh_after_action(&mut self) -> wasm::Result<()> {
        self.refresh_inner(true)
    }

    fn refresh_inner(&mut self, keep_action_error: bool) -> wasm::Result<()> {
        let result = self.fetch_and_update();
        self.drain_events()?;
        if !keep_action_error && self.error.is_none() {
            // A successful periodic or manual refresh brings the
            // "updated" line back.
            self.action_error = None;
        }
        result
    }

    /// Discards the window events queued while the guest was blocked. The
    /// close request is the exception: the host sends it only once, so it is
    /// kept as a flag for the run loop.
    fn drain_events(&mut self) -> wasm::Result<()> {
        loop {
            let mut code = EventCode::All as u32;
            let mut target: *mut Object = null_mut();
            unsafe {
                window_pop_event(
                    self.window,
                    &mut code,
                    (&mut target as *mut *mut Object).cast(),
                )?;
            }
            if code == EVENT_CLOSE_REQUESTED {
                self.close_requested = true;
                return Ok(());
            }
            if code == EventCode::All as u32 {
                return Ok(());
            }
        }
    }

    fn fetch_and_update(&mut self) -> wasm::Result<()> {
        let period = self
            .configuration
            .as_ref()
            .map_or(configuration::DEFAULT_REFRESH_SECONDS, |configuration| {
                configuration.refresh_seconds()
            });
        if self.configuration.is_none() || self.entities.is_empty() {
            return Ok(());
        }
        let Some(token) = self.token.clone() else {
            // The start-up notice already shows the missing or unreadable
            // token; do not repeat it in the status line.
            self.finish_refresh(period);
            return Ok(());
        };
        self.show_busy(t!("header.loading"))?;
        let results = match &self.configuration {
            Some(configuration) => {
                let entities = self.entities.clone();
                api::refresh_states(&mut Devices, configuration, &token, &entities)
            }
            None => return Ok(()),
        };
        let results = match results {
            Ok(results) => results,
            Err(error) => {
                self.error = Some(error);
                self.finish_refresh(period);
                return Ok(());
            }
        };
        self.error = None;
        for index in 0..self.entities.len() {
            match results.get(index).cloned().unwrap_or(Err(Error::Network)) {
                Ok(entity_state) => {
                    self.states[index] = Some(entity_state);
                    self.errors[index] = None;
                }
                Err(error) => {
                    self.errors[index] = Some(error.clone());
                    if self.error.is_none() {
                        self.error = Some(error);
                    }
                }
            }
            unsafe { self.update_card(index)? };
        }
        self.last_refresh = Some(Instant::now());
        self.finish_refresh(period);
        Ok(())
    }

    /// The period counts from the end of the refresh.
    fn finish_refresh(&mut self, period: u32) {
        self.next_refresh = Some(Instant::now() + Duration::from_secs(u64::from(period)));
        self.shown_status.clear();
    }

    unsafe fn update_card(&mut self, index: usize) -> wasm::Result<()> {
        let entity_state = self.states.get(index).cloned().flatten();
        let error = self.errors.get(index).cloned().flatten();
        let domain = Domain::parse(&self.entities[index]);
        let card = &mut self.cards[index];
        unsafe {
            set_hidden(card.error, error.is_none())?;
            if let Some(error) = &error {
                set_text(card.error, &error_message(error))?;
            }
            match &entity_state {
                None => set_text(card.value, "-")?,
                Some(entity_state) => {
                    set_text(card.name, entity_state.display_name())?;
                    let media = entity_state.media_text();
                    set_hidden(card.media, media.is_none())?;
                    if let Some(media) = &media {
                        set_text(card.media, media)?;
                    }
                    let mut value = entity_state.value_text();
                    if domain == Domain::MediaPlayer
                        && let Some(volume) = entity_state.volume_text()
                    {
                        value = std::format!("{value}, {volume}");
                    }
                    set_text(card.value, &value)?;
                    for button in &card.buttons {
                        if button.control == Control::PlayPause {
                            let symbol = if entity_state.state == "playing" {
                                SYMBOL_PAUSE
                            } else {
                                SYMBOL_PLAY
                            };
                            set_text(button.caption, symbol)?;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Calls the service of a card control, then refreshes the states.
    fn act(&mut self, card_index: usize, control: Control) -> wasm::Result<()> {
        let Some(entity_id) = self.entities.get(card_index).cloned() else {
            return Ok(());
        };
        let Some(configuration) = &self.configuration else {
            return Ok(());
        };
        let Some(token) = self.token.clone() else {
            self.error = Some(Error::MissingToken);
            return Ok(());
        };
        let endpoint = match Endpoint::parse(&configuration.url) {
            Ok(endpoint) => endpoint,
            Err(error) => {
                self.error = Some(error);
                return Ok(());
            }
        };
        self.show_busy(t!("header.loading"))?;
        self.action_error = None;
        let body = service::service_body(&entity_id);
        if let Err(error) = api::call_service(
            &mut Devices,
            &endpoint,
            &token,
            domain_of(&entity_id),
            control.service(),
            &body,
        ) {
            // Kept in its own slot: the refresh below reports its own errors.
            self.action_error = Some(error);
        }
        self.refresh_after_action()
    }

    fn show_notice(&mut self) -> wasm::Result<()> {
        let text = if let Some(invalid) = &self.startup_invalid {
            Some(String::from(invalid_message(*invalid)))
        } else if let Some(error) = &self.startup_error {
            Some(error_message(error))
        } else if self.configuration.is_none() {
            Some(String::from(t!("main.not_configured")))
        } else if self.entities.is_empty() {
            Some(String::from(t!("main.no_entities")))
        } else {
            None
        };
        unsafe {
            set_hidden(self.notice, text.is_none())?;
            set_text(self.notice, text.as_deref().unwrap_or(""))?;
        }
        Ok(())
    }

    /// Status line: the error of the last refresh, else the time since it.
    fn update_status(&mut self) -> wasm::Result<()> {
        let (text, color) = match (&self.action_error, &self.error, self.last_refresh) {
            (Some(error), _, _) => (error_message(error), ERROR_COLOR),
            (None, Some(error), _) => (error_message(error), ERROR_COLOR),
            (None, None, Some(last)) => (
                t!("header.updated", age = age(last.elapsed().as_secs())).to_string(),
                MUTED_COLOR,
            ),
            (None, None, None) => (String::new(), MUTED_COLOR),
        };
        if text != self.shown_status {
            unsafe {
                set_text(self.status, &text)?;
                object_set_text_color(self.status, color, 0)?;
            }
            self.shown_status = text;
        }
        Ok(())
    }

    fn handle(&mut self, code: u32, target: *mut Object) -> wasm::Result<()> {
        if code != EventCode::Clicked as u32 {
            return Ok(());
        }
        if target == self.refresh_button {
            return self.refresh();
        }
        for (card_index, card) in self.cards.iter().enumerate() {
            for button in &card.buttons {
                if button.object == target {
                    let control = button.control;
                    return self.act(card_index, control);
                }
            }
        }
        Ok(())
    }

    pub fn run(&mut self) -> wasm::Result<()> {
        self.start()?;
        loop {
            if self.close_requested {
                unsafe { delete(self.window)? };
                return Ok(());
            }
            loop {
                let mut code = EventCode::All as u32;
                let mut target: *mut Object = null_mut();
                unsafe {
                    window_pop_event(
                        self.window,
                        &mut code,
                        (&mut target as *mut *mut Object).cast(),
                    )?;
                }
                if code == EVENT_CLOSE_REQUESTED {
                    self.close_requested = true;
                    break;
                }
                if code == EventCode::All as u32 {
                    break;
                }
                self.handle(code, target)?;
            }
            if self.next_refresh.is_some_and(|next| Instant::now() >= next) {
                self.refresh()?;
            }
            self.update_status()?;
            sleep(Duration::from_millis(50));
        }
    }
}

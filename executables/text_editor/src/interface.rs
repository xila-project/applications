use crate::storage::{self, PAGE_BYTES, Page, WorkingCopy};
use embedded_i18n::t;
use std::{
    ffi::{CStr, CString},
    path::Path,
    ptr::null_mut,
    thread::sleep,
    time::Duration,
};
use wasm::*;

struct Interface {
    window: *mut Object,
    actions: [*mut Object; 7],
    more: *mut Object,
    details: *mut Object,
    details_open: bool,
    title: *mut Object,
    rendered_title: String,
    secondary: *mut Object,
    navigation: *mut Object,
    compact: bool,
    path: *mut Object,
    text: *mut Object,
    keyboard: *mut Object,
    status: *mut Object,
    page_label: *mut Object,
    working: Option<WorkingCopy>,
    original_page: String,
    page_start: u64,
    page_length: usize,
    dirty: bool,
    revision: u64,
    confirm_open: Option<u64>,
    confirm_close: Option<u64>,
    working_unusable: bool,
    picker: Option<(*mut Object, FilePickerMode)>,
    retained_picker: Option<*mut Object>,
}

pub fn run() -> wasm::Result<()> {
    let mut ui = unsafe { Interface::new()? };
    loop {
        ui.poll_picker()?;
        for _ in 0..32 {
            let mut code = 0u32;
            let mut target: *mut Object = null_mut();
            let (mut x, mut y, mut width, mut height) = (0, 0, 0, 0);
            unsafe {
                window_pop_pointer_event(
                    ui.window,
                    &mut code,
                    (&mut target as *mut *mut Object).cast(),
                    &mut x,
                    &mut y,
                    &mut width,
                    &mut height,
                )?;
            }
            if code == EventCode::All as u32 {
                break;
            }
            if code == EVENT_CLOSE_REQUESTED {
                if ui.working_unusable {
                    if ui.confirm_close != Some(ui.revision) {
                        ui.status(t!("errors.edit_failed_close_again"))?;
                        ui.confirm_close = Some(ui.revision);
                        continue;
                    }
                } else if !ui.commit_page()? {
                    continue;
                }
                if !ui.dirty || ui.confirm_close == Some(ui.revision) {
                    unsafe {
                        object_delete(u32::try_from(ui.window as usize).map_err(|_| -1)?)?;
                    }
                    return Ok(());
                }
                ui.status(t!("warnings.unsaved_close_again"))?;
                ui.confirm_close = Some(ui.revision);
            } else if ui.picker.is_some() {
                continue;
            } else if code == EventCode::Clicked as u32 && target == ui.more {
                ui.details_open = !ui.details_open;
                unsafe {
                    object_add_flag(ui.keyboard, ObjectFlag::Hidden)?;
                    set_hidden(ui.details, !ui.details_open)?;
                    set_hidden(ui.text, false)?;
                    if ui.compact {
                        set_hidden(ui.secondary, false)?;
                    }
                    set_hidden(ui.navigation, false)?;
                    set_hidden(ui.status, false)?;
                }
            } else if code == EventCode::Clicked as u32
                && let Some(action) = ui.actions.iter().position(|button| *button == target)
            {
                unsafe {
                    object_add_flag(ui.keyboard, ObjectFlag::Hidden)?;
                    object_add_flag(ui.details, ObjectFlag::Hidden)?;
                    set_hidden(ui.text, false)?;
                    if ui.compact {
                        set_hidden(ui.secondary, false)?;
                    }
                    set_hidden(ui.navigation, false)?;
                    set_hidden(ui.status, false)?;
                }
                ui.details_open = false;
                match action {
                    0 => ui.navigate(false)?,
                    1 => ui.navigate(true)?,
                    2 => ui.new_document()?,
                    3 => ui.choose_path(FilePickerMode::OpenFile)?,
                    4 => ui.choose_path(FilePickerMode::SaveFile)?,
                    5 => ui.choose_path(FilePickerMode::Directory)?,
                    6 => ui.save_document()?,
                    _ => (),
                }
            } else if code == EventCode::Focused as u32 && (target == ui.path || target == ui.text)
            {
                unsafe {
                    keyboard_set_textarea(ui.keyboard, target)?;
                    if ui.compact {
                        object_add_flag(ui.navigation, ObjectFlag::Hidden)?;
                        object_add_flag(ui.status, ObjectFlag::Hidden)?;
                        set_hidden(ui.text, target == ui.path)?;
                        set_hidden(ui.secondary, target == ui.path)?;
                        if target == ui.text {
                            object_add_flag(ui.details, ObjectFlag::Hidden)?;
                            ui.details_open = false;
                        }
                    }
                    object_remove_flag(ui.keyboard, ObjectFlag::Hidden)?;
                }
            } else if code == EventCode::ValueChanged as u32 && target == ui.text {
                if ui.working_unusable {
                    ui.revision = ui.revision.wrapping_add(1);
                    ui.confirm_open = None;
                    ui.confirm_close = None;
                }
                ui.refresh_title()?;
            } else if (code == EventCode::Ready as u32 || code == EventCode::Cancel as u32)
                && target == ui.keyboard
            {
                unsafe {
                    object_add_flag(ui.keyboard, ObjectFlag::Hidden)?;
                    set_hidden(ui.text, false)?;
                    if ui.compact {
                        set_hidden(ui.secondary, false)?;
                    }
                    set_hidden(ui.navigation, false)?;
                    set_hidden(ui.status, false)?;
                }
            }
            ui.refresh_title()?;
        }
        sleep(Duration::from_millis(16));
    }
}

unsafe fn set_hidden(object: *mut Object, hidden: bool) -> wasm::Result<()> {
    unsafe {
        if hidden {
            object_add_flag(object, ObjectFlag::Hidden)
        } else {
            object_remove_flag(object, ObjectFlag::Hidden)
        }
    }
}

unsafe fn row(parent: *mut Object, height: i32) -> wasm::Result<*mut Object> {
    unsafe {
        let row = object_create(parent)?;
        object_set_background(row, 0, 0, 0)?;
        object_set_border(row, 0, 0, 0, 0)?;
        object_set_padding(row, 0)?;
        object_set_size(row, percentage(100)?, height)?;
        object_set_flex_flow(row, FlexFlow::Row)?;
        object_set_flex_align(row, FlexAlign::Start, FlexAlign::Center, FlexAlign::Center)?;
        object_remove_flag(row, ObjectFlag::Scrollable)?;
        Ok(row)
    }
}

unsafe fn action_button(
    parent: *mut Object,
    title: &CStr,
    variant: u32,
) -> wasm::Result<*mut Object> {
    unsafe {
        let button = button_create(parent)?;
        object_set_theme_variant(button, variant)?;
        object_set_min_max_size(button, 0, 16384, 0, 16384, 0)?;
        object_set_size(button, size_content(), 28)?;
        object_set_padding(button, 6)?;
        let label = label_create(button)?;
        label_set_text(label, title.as_ptr())?;
        object_align(label, Align::Center, 0, 0)?;
        Ok(button)
    }
}

impl Interface {
    unsafe fn new() -> wasm::Result<Self> {
        unsafe {
            let window = window_create()?;
            window_set_icon(
                window,
                c"Te".as_ptr(),
                Color {
                    red: 76,
                    green: 110,
                    blue: 245,
                },
            )?;
            object_set_flex_flow(window, FlexFlow::Column)?;
            object_set_padding(window, 4)?;
            object_remove_flag(window, ObjectFlag::Scrollable)?;
            object_set_font(window, 1, 0)?;
            object_update_layout(window)?;
            let compact = object_get_width(window)? < 560 || object_get_height(window)? < 360;
            let toolbar = row(window, 32)?;
            let title = label_create(toolbar)?;
            label_set_text(title, c"".as_ptr())?;
            object_set_width(title, 0)?;
            object_set_height(title, 18)?;
            object_set_flex_grow(title, 1)?;
            let details = row(window, size_content())?;
            object_set_flex_flow(details, FlexFlow::Column)?;
            object_add_flag(details, ObjectFlag::Hidden)?;
            let secondary = if compact { row(details, 30)? } else { toolbar };
            let new = action_button(secondary, t!(c"controls.new_file"), 3)?;
            let open = action_button(secondary, t!(c"controls.open_file"), 3)?;
            let save = action_button(secondary, t!(c"controls.save_file"), 2)?;
            let save_as = action_button(toolbar, t!(c"controls.save_as"), 0)?;
            let more = action_button(toolbar, c"...", 3)?;
            object_set_width(more, 32)?;
            let folder = action_button(
                if compact { secondary } else { details },
                t!(c"controls.choose_folder"),
                3,
            )?;
            let path = textarea_create(details)?;
            textarea_set_one_line(path, true)?;
            textarea_set_max_length(path, 1023)?;
            let initial = std::env::args()
                .nth(1)
                .or_else(storage::default_path)
                .unwrap_or_default();
            let initial = CString::new(initial).map_err(|_| -1)?;
            textarea_set_text(path, initial.as_ptr())?;
            object_set_size(path, percentage(100)?, 30)?;
            let text = textarea_create(window)?;
            textarea_set_max_length(text, PAGE_BYTES as u32)?;
            object_set_width(text, percentage(100)?)?;
            object_set_height(text, 0)?;
            object_set_flex_grow(text, 1)?;
            object_set_font(text, 5, 0)?;
            object_set_min_max_size(text, 0, 16384, 36, 16384, 0)?;
            let navigation = row(window, 28)?;
            let previous = action_button(navigation, t!(c"navigation.previous_page"), 3)?;
            let page_label = label_create(navigation)?;
            label_set_text(page_label, c"".as_ptr())?;
            object_set_width(page_label, 0)?;
            object_set_flex_grow(page_label, 1)?;
            object_set_text_align(page_label, TextAlign::Center as u32, 0)?;
            let next = action_button(navigation, t!(c"navigation.next_page"), 3)?;
            let actions = [previous, next, new, open, save_as, folder, save];
            let status = label_create(window)?;
            label_set_text(status, c"".as_ptr())?;
            object_set_font(status, 0, 0)?;
            object_set_width(status, percentage(100)?)?;
            let keyboard = keyboard_create(window)?;
            object_set_size(keyboard, percentage(100)?, if compact { 100 } else { 180 })?;
            object_add_flag(keyboard, ObjectFlag::Hidden)?;
            // Leave room for the shell's home handle below the status/keyboard.
            row(window, 8)?;
            for object in actions.into_iter().chain([more, path, text, keyboard]) {
                object_add_flag(object, ObjectFlag::EventBubble)?;
            }
            let mut ui = Self {
                window,
                actions,
                more,
                details,
                details_open: false,
                title,
                rendered_title: String::new(),
                secondary,
                navigation,
                compact,
                path,
                text,
                keyboard,
                status,
                page_label,
                working: None,
                original_page: String::new(),
                page_start: 0,
                page_length: 0,
                dirty: false,
                revision: 0,
                confirm_open: None,
                confirm_close: None,
                working_unusable: false,
                picker: None,
                retained_picker: None,
            };
            ui.status(t!("file_path.picker_instructions"))?;
            ui.show_page()?;
            ui.refresh_title()?;
            Ok(ui)
        }
    }

    fn refresh_title(&mut self) -> wasm::Result<()> {
        let path = self.path().unwrap_or_default();
        let name = Path::new(&path)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        let modified = self.dirty
            || (self.working.is_some()
                && self.edited_text()?.as_deref() != Some(&self.original_page));
        // Keep long filenames on one line; the complete path remains editable
        // in file details. Truncate at a character boundary, never inside UTF-8.
        let limit = if self.compact { 24 } else { 48 };
        let mut name_text: String = name.chars().take(limit).collect();
        if name.chars().count() > limit {
            name_text.push('…');
        }
        let title = format!("{}{}", if modified { "* " } else { "" }, name_text);
        if title != self.rendered_title {
            let text = CString::new(title.as_str()).map_err(|_| -1)?;
            unsafe {
                label_set_text(self.title, text.as_ptr())?;
            }
            self.rendered_title = title;
        }
        Ok(())
    }

    fn path(&self) -> wasm::Result<String> {
        let mut bytes = [0i8; 1024];
        unsafe {
            textarea_get_text(self.path, bytes.as_mut_ptr(), bytes.len())?;
        }
        let path = unsafe { CStr::from_ptr(bytes.as_ptr()) }
            .to_str()
            .unwrap_or("");
        if !path.starts_with('/') || path.ends_with('/') {
            return Err(-1);
        }
        Ok(path.to_owned())
    }

    fn edited_text(&self) -> wasm::Result<Option<String>> {
        let mut bytes = vec![0i8; PAGE_BYTES * 4 + 1];
        unsafe {
            textarea_get_text(self.text, bytes.as_mut_ptr(), bytes.len())?;
        }
        let text = unsafe { CStr::from_ptr(bytes.as_ptr()) }
            .to_str()
            .unwrap_or("");
        if text.len() > PAGE_BYTES || text.contains('\0') {
            return Ok(None);
        }
        Ok(Some(text.to_owned()))
    }

    fn commit_page(&mut self) -> wasm::Result<bool> {
        if self.working_unusable {
            return Ok(false);
        }
        if self.working.is_none() {
            return Ok(true);
        }
        let Some(text) = self.edited_text()? else {
            self.status(t!("errors.page_exceeds_byte_limit"))?;
            return Ok(false);
        };
        if text != self.original_page {
            let result = self.working.as_mut().ok_or(-1)?.replace_page(
                self.page_start,
                self.page_length,
                &text,
            );
            if result.is_err() {
                self.dirty = true;
                self.revision = self.revision.wrapping_add(1);
                self.working_unusable = true;
                self.status(t!("errors.edit_failed_reopen_source"))?;
                return Ok(false);
            }
            self.dirty = true;
            self.revision = self.revision.wrapping_add(1);
            self.show_page()?;
        }
        Ok(true)
    }

    fn navigate(&mut self, forward: bool) -> wasm::Result<()> {
        if !self.commit_page()? {
            return Ok(());
        }
        let Some(working) = self.working.as_mut() else {
            self.status(t!("status.open_or_create_first"))?;
            return Ok(());
        };
        if forward {
            let next = self.page_start + self.page_length as u64;
            if next < working.length() {
                self.page_start = next;
            }
        } else if self.page_start > 0 {
            self.page_start = working
                .previous_page_start(self.page_start)
                .map_err(|_| -1)?;
        }
        self.show_page()?;
        Ok(())
    }

    fn new_document(&mut self) -> wasm::Result<()> {
        if (!self.working_unusable && !self.commit_page()?) || !self.allow_discard()? {
            return Ok(());
        }
        let path = match self.path() {
            Ok(path) => path,
            Err(_) => {
                self.status(t!("file_path.placeholder"))?;
                return Ok(());
            }
        };
        let parent = Path::new(&path).parent().ok_or(-1)?;
        match WorkingCopy::create(parent) {
            Ok(working) => {
                self.working = Some(working);
                self.page_start = 0;
                self.page_length = 0;
                self.original_page.clear();
                self.dirty = true;
                self.working_unusable = false;
                self.revision = self.revision.wrapping_add(1);
                self.show_page()?;
                self.status(t!("file_path.new_file_default_name"))?;
            }
            Err(_) => self.status(t!("errors.working_copy_creation_failed"))?,
        }
        Ok(())
    }

    fn choose_path(&mut self, mode: FilePickerMode) -> wasm::Result<()> {
        if self.picker.is_some() {
            return Ok(());
        }
        if mode == FilePickerMode::OpenFile {
            if (!self.working_unusable && !self.commit_page()?) || !self.allow_discard()? {
                return Ok(());
            }
        } else if mode == FilePickerMode::SaveFile {
            if self.working_unusable {
                return self.status(t!("errors.edit_failed_reopen_source"));
            }
            if !self.commit_page()? {
                return Ok(());
            }
            if self.working.is_none() {
                return self.status(t!("status.open_or_create_first"));
            }
        }
        let mut initial = self.path().unwrap_or_else(|_| "/".into());
        if mode == FilePickerMode::Directory {
            initial = Path::new(&initial)
                .parent()
                .and_then(Path::to_str)
                .unwrap_or("/")
                .into();
        }
        let initial = CString::new(initial).map_err(|_| -1)?;
        unsafe {
            object_add_flag(self.keyboard, ObjectFlag::Hidden)?;
            let picker = if let Some(picker) = self.retained_picker {
                file_picker_reset(picker, mode as u32, initial.as_ptr())?;
                picker
            } else {
                let picker = file_picker_create(self.window, mode as u32, initial.as_ptr())?;
                self.retained_picker = Some(picker);
                picker
            };
            self.picker = Some((picker, mode));
        }
        Ok(())
    }

    fn poll_picker(&mut self) -> wasm::Result<()> {
        let Some((picker, mode)) = self.picker else {
            return Ok(());
        };
        let status = unsafe { file_picker_get_status(picker)? };
        if status == FilePickerStatus::Pending as u32 {
            return Ok(());
        }
        if status == FilePickerStatus::Selected as u32 {
            let mut bytes = [0i8; 1024];
            unsafe {
                file_picker_get_path(picker, bytes.as_mut_ptr(), bytes.len())?;
            }
            let selected = unsafe { CStr::from_ptr(bytes.as_ptr()) }
                .to_str()
                .map_err(|_| -1)?;
            let selected = if mode == FilePickerMode::Directory {
                let Some(destination) = storage::new_path(selected) else {
                    self.picker = None;
                    return self.status(t!("errors.folder_path_too_long"));
                };
                destination
            } else {
                selected.to_owned()
            };
            let selected = CString::new(selected).map_err(|_| -1)?;
            unsafe {
                textarea_set_text(self.path, selected.as_ptr())?;
            }
        }
        self.picker = None;
        if status == FilePickerStatus::Selected as u32 {
            match mode {
                FilePickerMode::OpenFile => self.open_document()?,
                FilePickerMode::SaveFile => self.save_document()?,
                FilePickerMode::Directory => self.status(t!("status.folder_selected"))?,
            }
        }
        Ok(())
    }

    fn open_document(&mut self) -> wasm::Result<()> {
        // choose_path checked dirty-document consent before opening the picker.
        let path = match self.path() {
            Ok(path) => path,
            Err(_) => {
                self.status(t!("file_path.placeholder"))?;
                return Ok(());
            }
        };
        match WorkingCopy::open(&path) {
            Ok(working) => {
                self.working = Some(working);
                self.page_start = 0;
                self.page_length = 0;
                self.original_page.clear();
                self.dirty = false;
                self.working_unusable = false;
                self.revision = self.revision.wrapping_add(1);
                self.show_page()?;
                self.status(t!("status.opened"))?;
            }
            Err(_) => self.status(t!("errors.open_failed"))?,
        }
        Ok(())
    }

    fn save_document(&mut self) -> wasm::Result<()> {
        if self.working_unusable {
            self.status(t!("errors.edit_failed_reopen_source"))?;
            return Ok(());
        }
        if !self.commit_page()? {
            return Ok(());
        }
        let path = match self.path() {
            Ok(path) => path,
            Err(_) => {
                self.status(t!("file_path.placeholder"))?;
                return Ok(());
            }
        };
        let save_result = match self.working.as_mut() {
            Some(working) => working.save_to(&path),
            None => return self.status(t!("status.open_or_create_first")),
        };
        if save_result.is_ok() {
            self.dirty = false;
            self.revision = self.revision.wrapping_add(1);
            self.status(t!("status.saved"))?;
        } else {
            self.status(t!("errors.save_failed_working_copy_preserved"))?;
        }
        Ok(())
    }

    fn allow_discard(&mut self) -> wasm::Result<bool> {
        if self.dirty && self.confirm_open != Some(self.revision) {
            self.confirm_open = Some(self.revision);
            self.status(t!("warnings.unsaved_new_or_open_again"))?;
            return Ok(false);
        }
        self.confirm_open = None;
        Ok(true)
    }

    fn show_page(&mut self) -> wasm::Result<()> {
        let mut bytes = [0; PAGE_BYTES];
        let mut page = Page { length: 0 };
        if let Some(working) = self.working.as_mut() {
            page = working
                .read_page(self.page_start, &mut bytes)
                .map_err(|_| -1)?;
        }
        let page_text = std::str::from_utf8(&bytes[..page.length]).map_err(|_| -1)?;
        self.original_page = page_text.to_owned();
        let text = CString::new(page_text).map_err(|_| -1)?;
        unsafe {
            textarea_set_text(self.text, text.as_ptr())?;
            let total_length = self.working.as_ref().map_or(0, WorkingCopy::length);
            let label = CString::new(format!(
                "{} / {}",
                self.page_start + page.length as u64,
                total_length
            ))
            .map_err(|_| -1)?;
            label_set_text(self.page_label, label.as_ptr().cast_mut())?;
        }
        self.page_length = page.length;
        Ok(())
    }

    fn status(&self, text: &str) -> wasm::Result<()> {
        let text = CString::new(text).map_err(|_| -1)?;
        unsafe { label_set_text(self.status, text.as_ptr().cast_mut()) }
    }
}

//! The settings and profiles files.
//!
//! Both live in the application's private configuration directory, which
//! the WebAssembly runner creates for each user with owner-only permissions
//! and passes in `XILA_APPLICATION_CONFIGURATION`.

use crate::configuration::Settings;
use crate::error::{Error, Result};
use crate::layout::Layout;
use crate::profile::Profiles;
use alloc::string::String;
use alloc::vec::Vec;
use std::{
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

pub const SETTINGS_FILE: &str = "settings.json";
pub const PROFILES_FILE: &str = "profiles.json";
/// Largest file read.
const MAXIMUM_FILE_SIZE: u64 = 16 * 1024;

pub struct Storage {
    directory: PathBuf,
}

impl Storage {
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
        }
    }

    /// The directory given by the runner.
    pub fn from_environment() -> Option<Self> {
        std::env::var_os("XILA_APPLICATION_CONFIGURATION").map(Self::new)
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    /// The settings, or `None` when there is no settings file yet.
    pub fn load_settings(&self) -> Result<Option<Settings>> {
        let Some(text) = self.read(SETTINGS_FILE)? else {
            return Ok(None);
        };
        Settings::parse(&text).map(Some)
    }

    /// Writes the settings, unless they are larger than [`Storage::load_settings`]
    /// reads: a file that cannot be read back would stop the application from
    /// starting.
    pub fn save_settings(&self, settings: &Settings) -> Result<()> {
        self.write(SETTINGS_FILE, settings.to_json().as_bytes())
    }

    /// The profiles, or `None` when there is no profiles file yet. Every
    /// `keys` text is checked with `layout`, so the report of ignored
    /// buttons matches the layout the shortcuts compile with.
    pub fn load_profiles(&self, layout: Layout) -> Result<Option<Profiles>> {
        let Some(text) = self.read(PROFILES_FILE)? else {
            return Ok(None);
        };
        Profiles::parse(&text, layout).map(Some)
    }

    /// Reads one file, `None` when it does not exist.
    fn read(&self, name: &str) -> Result<Option<String>> {
        let path = self.directory.join(name);
        match fs::metadata(&path) {
            Ok(metadata) if metadata.len() > MAXIMUM_FILE_SIZE => return Err(Error::TooLarge),
            Ok(_) => (),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(Error::Storage),
        }
        let mut bytes = Vec::new();
        fs::File::open(&path)
            .and_then(|file| file.take(MAXIMUM_FILE_SIZE + 1).read_to_end(&mut bytes))
            .map_err(|_| Error::Storage)?;
        if bytes.len() as u64 > MAXIMUM_FILE_SIZE {
            return Err(Error::TooLarge);
        }
        String::from_utf8(bytes)
            .map(Some)
            .map_err(|_| Error::Storage)
    }

    /// Writes a temporary file, then renames it over `name`, so a failed
    /// write leaves the previous file in place.
    fn write(&self, name: &str, contents: &[u8]) -> Result<()> {
        if contents.len() as u64 > MAXIMUM_FILE_SIZE {
            return Err(Error::TooLarge);
        }
        if !self.directory.is_dir() {
            fs::create_dir_all(&self.directory).map_err(|_| Error::Storage)?;
        }
        let temporary = self.directory.join(String::from(".") + name + ".new");
        let result = (|| {
            let mut file = fs::File::create(&temporary)?;
            file.write_all(contents)?;
            file.flush()?;
            drop(file);
            fs::rename(&temporary, self.directory.join(name))
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result.map_err(|_| Error::Storage)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn directory(name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("xila-shortcuts-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        path
    }

    #[test]
    fn settings_round_trip() {
        let path = directory("round-trip");
        let storage = Storage::new(&path);
        assert_eq!(storage.load_settings(), Ok(None));
        assert_eq!(storage.load_profiles(Layout::Qwerty), Ok(None));

        let settings = Settings {
            profile: crate::profile::Profile::Macos,
            layout: crate::layout::Layout::Azerty,
            mouse_pad: false,
        };
        storage.save_settings(&settings).unwrap();
        assert_eq!(storage.load_settings(), Ok(Some(settings.clone())));
        assert!(!path.join(".settings.json.new").exists());

        // Saving again replaces the file.
        let changed = Settings {
            mouse_pad: true,
            ..settings
        };
        storage.save_settings(&changed).unwrap();
        assert_eq!(storage.load_settings(), Ok(Some(changed)));

        // An invalid file is reported, not silently defaulted.
        fs::write(path.join(SETTINGS_FILE), "{").unwrap();
        assert_eq!(storage.load_settings(), Err(Error::Configuration));
        fs::write(path.join(SETTINGS_FILE), "not json").unwrap();
        assert_eq!(storage.load_settings(), Err(Error::Configuration));

        // A too large file is refused.
        fs::write(path.join(SETTINGS_FILE), "a".repeat(20 * 1024)).unwrap();
        assert_eq!(storage.load_settings(), Err(Error::TooLarge));
        // Small settings still save over the too large file.
        storage.save_settings(&Settings::default()).unwrap();
        assert_eq!(storage.load_settings(), Ok(Some(Settings::default())));
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn profiles_round_trip() {
        let path = directory("profiles");
        let storage = Storage::new(&path);
        let profiles = Profiles::parse(
            r#"{"profiles": [{"id": "windows", "buttons": [{"id": "screenshot", "keys": "Win+Shift+S"}]}]}"#,
            Layout::Qwerty,
        )
        .unwrap();
        // Profiles are only read, never written by the application, but the
        // storage can write them back for tests.
        let text = profiles.to_json();
        storage.write(PROFILES_FILE, text.as_bytes()).unwrap();
        assert_eq!(storage.load_profiles(Layout::Qwerty), Ok(Some(profiles)));
        fs::write(path.join(PROFILES_FILE), "{").unwrap();
        assert_eq!(storage.load_profiles(Layout::Qwerty), Err(Error::Profiles));
        fs::remove_dir_all(path).unwrap();
    }
}

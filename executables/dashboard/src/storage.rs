//! Configuration and token files.
//!
//! Both live in the application's private configuration directory, which the
//! WebAssembly runner creates for each user with owner-only permissions and
//! passes in `XILA_APPLICATION_CONFIGURATION`.

use crate::configuration::Configuration;
use crate::error::{Error, Result};
use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    string::String,
};

pub const CONFIGURATION_FILE: &str = "configuration.json";
pub const TOKEN_FILE: &str = "token";
/// Largest configuration file read; a full dashboard cache fits easily.
const MAXIMUM_CONFIGURATION_SIZE: u64 = 128 * 1024;
/// Largest token accepted.
pub const MAXIMUM_TOKEN_LENGTH: usize = 512;

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

    /// The configuration, or `None` when there is none yet.
    pub fn load(&self) -> Result<Option<Configuration>> {
        let path = self.directory.join(CONFIGURATION_FILE);
        match fs::metadata(&path) {
            Ok(metadata) if metadata.len() > MAXIMUM_CONFIGURATION_SIZE => {
                return Err(Error::TooLarge);
            }
            Ok(_) => (),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(Error::Storage),
        }
        let text = fs::read_to_string(path).map_err(|_| Error::Storage)?;
        Configuration::parse(&text).map(Some)
    }

    pub fn save(&self, configuration: &Configuration) -> Result<()> {
        self.write(CONFIGURATION_FILE, configuration.to_json().as_bytes())
    }

    /// The saved token, or `None` when there is none.
    pub fn token(&self) -> Result<Option<String>> {
        match fs::read_to_string(self.directory.join(TOKEN_FILE)) {
            Ok(text) => {
                let token = text.trim();
                if token.len() > MAXIMUM_TOKEN_LENGTH {
                    return Err(Error::TooLarge);
                }
                Ok((!token.is_empty()).then(|| String::from(token)))
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(_) => Err(Error::Storage),
        }
    }

    pub fn save_token(&self, token: &str) -> Result<()> {
        let token = token.trim();
        if token.is_empty()
            || token.len() > MAXIMUM_TOKEN_LENGTH
            || token.contains(char::is_whitespace)
        {
            return Err(Error::MissingToken);
        }
        self.write(TOKEN_FILE, token.as_bytes())
    }

    /// Writes a temporary file, then renames it over `name`, so a failed
    /// write leaves the previous file in place.
    fn write(&self, name: &str, contents: &[u8]) -> Result<()> {
        if !self.directory.is_dir() {
            fs::create_dir_all(&self.directory).map_err(|_| Error::Storage)?;
        }
        let temporary = self.directory.join(std::format!(".{name}.new"));
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
    use crate::configuration::Panel;

    fn directory(name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(std::format!("xila-dashboard-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        path
    }

    #[test]
    fn configuration_and_token_round_trip() {
        let path = directory("round-trip");
        let storage = Storage::new(&path);
        assert_eq!(storage.load(), Ok(None));
        assert_eq!(storage.token(), Ok(None));

        let configuration = Configuration {
            url: String::from("https://grafana.example.com"),
            data_source: String::from("prometheus"),
            refresh_seconds: 60,
            panels: std::vec![Panel {
                title: String::from("Load"),
                expression: String::from("node_load1"),
                ..Panel::new()
            }],
            ..Configuration::default()
        };
        storage.save(&configuration).unwrap();
        storage.save_token(" glsa_example\n").unwrap();
        assert_eq!(storage.load(), Ok(Some(configuration.clone())));
        assert_eq!(storage.token().unwrap().as_deref(), Some("glsa_example"));

        // Saving again replaces the file.
        let mut changed = configuration;
        changed.refresh_seconds = 120;
        storage.save(&changed).unwrap();
        assert_eq!(storage.load().unwrap().unwrap().refresh_seconds, 120);
        assert!(!path.join(".configuration.json.new").exists());

        assert_eq!(storage.save_token("  "), Err(Error::MissingToken));
        assert_eq!(storage.save_token("two words"), Err(Error::MissingToken));
        fs::write(path.join(CONFIGURATION_FILE), "{").unwrap();
        assert_eq!(storage.load(), Err(Error::Configuration));
        fs::remove_dir_all(path).unwrap();
    }
}

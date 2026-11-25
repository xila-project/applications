//! Configuration and token files.
//!
//! All live in the application's private configuration directory, which the
//! WebAssembly runner creates for each user with owner-only permissions and
//! passes in `XILA_APPLICATION_CONFIGURATION`. The access token never sits in
//! the configuration file: it stays in its own file, and is never logged.

use crate::configuration::Configuration;
use crate::error::{Error, Result};
use std::{
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    string::String,
    vec::Vec,
};

pub const CONFIGURATION_FILE: &str = "configuration.json";
pub const TOKEN_FILE: &str = "token";
/// Largest configuration file read.
const MAXIMUM_CONFIGURATION_SIZE: u64 = 16 * 1024;
/// Largest token accepted.
pub const MAXIMUM_TOKEN_LENGTH: usize = 512;
/// Largest import file read: only its first non-empty line is used.
const MAXIMUM_IMPORT_SIZE: u64 = 4 * 1024;

/// A token is sent in a header or a request body: printable ASCII only, so
/// that no character can end the line or break the body.
pub fn is_valid_token(token: &str) -> bool {
    !token.is_empty()
        && token.len() <= MAXIMUM_TOKEN_LENGTH
        && token.bytes().all(|byte| byte.is_ascii_graphic())
}

/// Checks a token before it is stored or sent: not empty, at most
/// [`MAXIMUM_TOKEN_LENGTH`] bytes, printable ASCII only. Each refusal says
/// which rule broke.
pub fn validate_token(token: &str) -> Result<()> {
    if token.is_empty() {
        return Err(Error::MissingToken);
    }
    if token.len() > MAXIMUM_TOKEN_LENGTH {
        return Err(Error::ImportTooLong);
    }
    if !is_valid_token(token) {
        return Err(Error::ImportInvalid);
    }
    Ok(())
}

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

    /// Writes the configuration, unless it is larger than [`Storage::load`]
    /// reads: a file that cannot be read back would stop the application from
    /// starting.
    pub fn save(&self, configuration: &Configuration) -> Result<()> {
        let json = configuration.to_json();
        if json.len() as u64 > MAXIMUM_CONFIGURATION_SIZE {
            return Err(Error::TooLarge);
        }
        self.write(CONFIGURATION_FILE, json.as_bytes())
    }

    /// The saved token, or `None` when there is none. A file that holds
    /// anything but one printable token (several lines, spaces, control
    /// characters) is [`Error::MissingToken`], so it is never sent.
    pub fn token(&self) -> Result<Option<String>> {
        let file = match fs::File::open(self.directory.join(TOKEN_FILE)) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(Error::Storage),
        };
        // One byte more than the longest token and its line end, so that a
        // larger file is noticed without being read whole.
        let mut bytes = Vec::new();
        file.take(MAXIMUM_TOKEN_LENGTH as u64 + 2)
            .read_to_end(&mut bytes)
            .map_err(|_| Error::Storage)?;
        let text = String::from_utf8(bytes).map_err(|_| Error::MissingToken)?;
        let token = text.trim();
        if token.is_empty() {
            return Ok(None);
        }
        if token.len() > MAXIMUM_TOKEN_LENGTH {
            return Err(Error::TooLarge);
        }
        if !is_valid_token(token) {
            return Err(Error::MissingToken);
        }
        Ok(Some(String::from(token)))
    }

    pub fn save_token(&self, token: &str) -> Result<()> {
        let token = token.trim();
        validate_token(token)?;
        self.write(TOKEN_FILE, token.as_bytes())
    }

    /// Deletes the token file.
    pub fn remove_token(&self) -> Result<()> {
        match fs::remove_file(self.directory.join(TOKEN_FILE)) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(Error::Storage),
        }
    }

    /// Reads a token from a file the user points at, such as one on an SD
    /// card: the first non-empty line, trimmed of spaces, its line end and a
    /// leading byte order mark. The source file is only read, never changed.
    /// The token passes the same rules as the token file, and each refusal
    /// says which rule broke.
    pub fn read_token_file(path: &str) -> Result<String> {
        let file = fs::File::open(path).map_err(|error| match error.kind() {
            io::ErrorKind::NotFound => Error::ImportMissing,
            _ => Error::ImportRead,
        })?;
        let mut bytes = Vec::new();
        file.take(MAXIMUM_IMPORT_SIZE)
            .read_to_end(&mut bytes)
            .map_err(|_| Error::ImportRead)?;
        // The read may stop in the middle of a character at the limit; the
        // first line is before it, so a lossy tail does not matter.
        let text = String::from_utf8_lossy(&bytes);
        let text = text.strip_prefix('\u{FEFF}').unwrap_or(&text);
        let line = text.lines().map(str::trim).find(|line| !line.is_empty());
        match line {
            None => Err(Error::ImportEmpty),
            Some(line) => validate_token(line).map(|()| String::from(line)),
        }
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

    fn directory(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(std::format!(
            "xila-home-assistant-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        path
    }

    fn configuration() -> Configuration {
        Configuration {
            url: String::from("http://homeassistant.local:8123"),
            refresh_seconds: 30,
            entities: std::vec![String::from("light.living_room")],
        }
    }

    #[test]
    fn configuration_and_token_round_trip() {
        let path = directory("round-trip");
        let storage = Storage::new(&path);
        assert_eq!(storage.load(), Ok(None));
        assert_eq!(storage.token(), Ok(None));

        let saved = configuration();
        storage.save(&saved).unwrap();
        storage.save_token("eyJhbGciOi\n").unwrap();
        assert_eq!(storage.load(), Ok(Some(saved.clone())));
        assert_eq!(storage.token().unwrap().as_deref(), Some("eyJhbGciOi"));

        // Saving again replaces the file.
        let mut changed = saved;
        changed.refresh_seconds = 60;
        storage.save(&changed).unwrap();
        assert_eq!(storage.load().unwrap().unwrap().refresh_seconds, 60);
        assert!(!path.join(".configuration.json.new").exists());

        assert_eq!(storage.save_token("  "), Err(Error::MissingToken));
        assert_eq!(storage.save_token("two words"), Err(Error::ImportInvalid));
        assert_eq!(storage.save_token("héllo"), Err(Error::ImportInvalid));
        assert_eq!(
            storage.save_token(&"a".repeat(MAXIMUM_TOKEN_LENGTH + 1)),
            Err(Error::ImportTooLong)
        );
        // A token file written by hand is checked as well: it must not carry
        // a header line of its own.
        for bad in ["eyJ\r\nX-Injected: 1", "two\nlines", "tab\tbed", "\u{1}"] {
            fs::write(path.join(TOKEN_FILE), bad).unwrap();
            assert_eq!(storage.token(), Err(Error::MissingToken), "{bad:?}");
        }
        fs::write(path.join(TOKEN_FILE), "a".repeat(100_000)).unwrap();
        assert_eq!(storage.token(), Err(Error::TooLarge));
        fs::write(path.join(TOKEN_FILE), " \n").unwrap();
        assert_eq!(storage.token(), Ok(None));
        storage.save_token("eyJhbGciOi").unwrap();
        fs::write(path.join(CONFIGURATION_FILE), "{").unwrap();
        assert_eq!(storage.load(), Err(Error::Configuration));

        storage.remove_token().unwrap();
        assert_eq!(storage.token(), Ok(None));
        storage.remove_token().unwrap();

        // A configuration that could not be read back is not written.
        let huge = Configuration {
            entities: (0..2_000)
                .map(|index| std::format!("sensor.sensor_{index}"))
                .collect(),
            ..configuration()
        };
        assert_eq!(storage.save(&huge), Err(Error::TooLarge));
        assert_eq!(
            fs::read_to_string(path.join(CONFIGURATION_FILE)).unwrap(),
            "{"
        );
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn reads_a_token_from_an_import_file() {
        let path = directory("import");
        let file = path.join("token.txt");
        fs::create_dir_all(&path).unwrap();

        assert_eq!(
            Storage::read_token_file(path.join("missing.txt").to_str().unwrap()),
            Err(Error::ImportMissing)
        );
        fs::write(&file, "\n  \n").unwrap();
        assert_eq!(
            Storage::read_token_file(file.to_str().unwrap()),
            Err(Error::ImportEmpty)
        );
        // The first non-empty line, trimmed of spaces and its line end.
        fs::write(&file, "\n  glsa_token  \r\nsecond line\n").unwrap();
        assert_eq!(
            Storage::read_token_file(file.to_str().unwrap()).unwrap(),
            "glsa_token"
        );
        // The source file is not changed.
        assert_eq!(
            fs::read_to_string(&file).unwrap(),
            "\n  glsa_token  \r\nsecond line\n"
        );
        // A leading byte order mark is trimmed.
        fs::write(&file, "\u{FEFF}glsa_token\n").unwrap();
        assert_eq!(
            Storage::read_token_file(file.to_str().unwrap()).unwrap(),
            "glsa_token"
        );
        for (contents, expected) in [
            ("two words\n", Error::ImportInvalid),
            ("tab\tbed\n", Error::ImportInvalid),
            ("héllo\n", Error::ImportInvalid),
            (
                &format!("{}\n", "a".repeat(MAXIMUM_TOKEN_LENGTH + 1)),
                Error::ImportTooLong,
            ),
        ] {
            fs::write(&file, contents).unwrap();
            assert_eq!(
                Storage::read_token_file(file.to_str().unwrap()),
                Err(expected)
            );
        }
        fs::remove_dir_all(path).unwrap();
    }
}

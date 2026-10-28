use alloc::{
    string::{String, ToString},
    vec::Vec,
};
use miniserde::{Deserialize, Serialize};
use xila::file_system::{AccessFlags, Path};
use xila::graphics::{Color, IconMotif, IconSpec};
use xila::virtual_file_system::File;

use crate::error::{Error, Result};

const MAX_SHORTCUT_BYTES: usize = 4096;

pub const SHORTCUT_PATH: &Path = Path::from_str("/configuration/shared/applications/shortcuts");

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Shortcut {
    pub name: String,
    pub command: String,
    #[serde(rename = "terminal")]
    pub _terminal: bool,
    pub arguments: Vec<String>,
    pub icon_string: Option<String>,
    icon_color: Option<[u8; 3]>,
    icon: Option<miniserde::json::Value>,
}

impl Shortcut {
    pub async fn read_from_path(
        context: &'static xila::executable::ExecutableContext,
        path: &Path,
        buffer: &mut Vec<u8>,
    ) -> Result<Shortcut> {
        let virtual_file_system = &context.virtual_file_system;
        let task = context.task_manager.get_current_task_identifier().await;

        let mut shortcut_file =
            File::open(virtual_file_system, task, path, AccessFlags::Read.into())
                .await
                .map_err(Error::FailedToReadShortcutFile)?;

        buffer.clear();

<<<<<<< HEAD
        shortcut_file
            .read_to_end(buffer, 2048)
            .await
            .map_err(Error::FailedToReadShortcutFile)?;

        shortcut_file
=======
        let result = async {
            let mut chunk = [0; 256];
            loop {
                let count = shortcut_file
                    .read(&mut chunk)
                    .await
                    .map_err(Error::FailedToReadShortcutFile)?;
                if count == 0 {
                    break;
                }
                if count > MAX_SHORTCUT_BYTES.saturating_sub(buffer.len()) {
                    return Err(Error::FailedToReadShortcutFile(
                        xila::virtual_file_system::Error::FileSystem(
                            xila::file_system::Error::InvalidParameter,
                        ),
                    ));
                }
                buffer.extend_from_slice(&chunk[..count]);
            }
            Ok(())
        }
        .await;
        let close = shortcut_file
>>>>>>> a3a5a74 (feat(shell): render application-provided procedural icons)
            .close(virtual_file_system)
            .await
            .map_err(Error::FailedToReadShortcutFile);
        result.and(close)?;

        let string = core::str::from_utf8(buffer).map_err(Error::InvalidUtf8)?;

        let shortcut = Shortcut::from_str(string)?;

        Ok(shortcut)
    }

    pub async fn read(
        context: &'static xila::executable::ExecutableContext,
        entry_name: &str,
        buffer: &mut Vec<u8>,
    ) -> Result<Shortcut> {
        let shortcut_file_path = SHORTCUT_PATH
            .append(entry_name)
            .ok_or(Error::FailedToGetShortcutFilePath)?;

        let shortcut = Shortcut::read_from_path(context, &shortcut_file_path, buffer).await?;

        Ok(shortcut)
    }

    pub fn from_str(string: &str) -> Result<Self> {
        miniserde::json::from_str(string).map_err(Error::FailedToDeserializeShortcut)
    }

<<<<<<< HEAD
    /// Resolve bundle-relative executable/asset arguments against the
    /// directory containing this shortcut. Never interpret a manifest here.
    pub fn resolve_bundle_paths(&mut self, bundle: &Path) -> Result<()> {
        fn resolve(value: &str, bundle: &Path) -> Result<String> {
            let Some(relative) = value.strip_prefix("@bundle/") else {
                return Ok(value.to_string());
            };
            if relative.is_empty()
                || relative.contains('\\')
                || relative
                    .split('/')
                    .any(|segment| segment.is_empty() || segment == "." || segment == "..")
            {
                return Err(Error::FailedToGetShortcutFilePath);
            }
            bundle
                .append(relative)
                .map(|path| path.as_str().to_string())
                .ok_or(Error::FailedToGetShortcutFilePath)
        }
        self.command = resolve(&self.command, bundle)?;
        for arg in &mut self.arguments {
            *arg = resolve(arg, bundle)?;
        }
        Ok(())
    }

    pub fn get_icon_color(&self) -> Color {
        Color::new(self.icon_color[0], self.icon_color[1], self.icon_color[2])
=======
    /// Resolve a compact, validated icon; invalid custom data falls back to auto.
    pub fn icon_spec(&self, identity: &str) -> (IconSpec, bool) {
        use miniserde::json::{Number, Value};

        let color = self.icon_color.unwrap_or([70, 80, 100]);
        let text = self.icon_string.as_deref().unwrap_or("");
        let fallback =
            IconSpec::automatic(identity, text, Color::new(color[0], color[1], color[2]));
        let Some(Value::Object(icon)) = self.icon.as_ref() else {
            return (fallback, false);
        };
        let parse = || -> Option<IconSpec> {
            if !matches!(icon.get("version"), Some(Value::Number(Number::U64(1)))) {
                return None;
            }
            if !matches!(icon.get("mode"), Some(Value::String(mode)) if mode == "procedural") {
                return None;
            }
            let motif = match icon.get("motif") {
                None => fallback.motif,
                Some(Value::String(motif)) => match motif.as_str() {
                    "rings" => IconMotif::Rings,
                    "diagonal" => IconMotif::Diagonal,
                    "grid" => IconMotif::Grid,
                    "steps" => IconMotif::Steps,
                    "orbit" => IconMotif::Orbit,
                    "rays" => IconMotif::Rays,
                    _ => return None,
                },
                _ => return None,
            };
            let text = match icon.get("monogram") {
                None => fallback.text(),
                Some(Value::String(text)) if text.chars().count() <= 2 && !text.contains('\0') => {
                    text
                }
                _ => return None,
            };
            fn color(value: Option<&Value>, default: Color) -> Option<Color> {
                let Some(value) = value else {
                    return Some(default);
                };
                let Value::String(value) = value else {
                    return None;
                };
                let value = value.strip_prefix('#')?;
                if value.len() != 6 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
                    return None;
                }
                let rgb = u32::from_str_radix(value, 16).ok()?;
                Some(Color::new((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8))
            }
            let background = color(icon.get("background"), fallback.background)?;
            let defaults = IconSpec::automatic(identity, text, background);
            let foreground = color(icon.get("foreground"), defaults.foreground)?;
            let accent = color(icon.get("accent"), defaults.accent)?;
            let seed = match icon.get("seed") {
                None => fallback.seed,
                Some(Value::Number(Number::U64(value))) => u32::try_from(*value).ok()?,
                _ => return None,
            };
            Some(IconSpec::procedural(
                motif, text, background, foreground, accent, seed,
            ))
        };
        parse()
            .map(|icon| (icon, true))
            .unwrap_or((fallback, false))
>>>>>>> a3a5a74 (feat(shell): render application-provided procedural icons)
    }

    // pub fn is_terminal(&self) -> bool {
    //     self._Terminal
    // }
}

#[cfg(test)]
<<<<<<< HEAD
mod shortcut_tests {
    use super::*;

    const SHORTCUT: &str = r#"{
        "$schema": "https://raw.githubusercontent.com/xila-project/applications/main/schemas/shell-shortcut.schema.json",
        "name": "Calculator",
        "command": "/applications/executables/wasm",
        "arguments": ["@bundle/executables/calculator.wasm", "--", "hello"],
        "terminal": false,
        "icon_string": "Ca",
        "icon_color": [158, 158, 158]
    }"#;

    #[test]
    fn resolves_bundle_relative_executable_without_manifest() {
        let mut shortcut = Shortcut::from_str(SHORTCUT).unwrap();
        shortcut
            .resolve_bundle_paths(Path::from_str("/applications/bundles/xila.calculator"))
            .unwrap();
        assert_eq!(shortcut.command, "/applications/executables/wasm");
        assert_eq!(
            shortcut.arguments[0],
            "/applications/bundles/xila.calculator/executables/calculator.wasm"
        );
        assert_eq!(shortcut.arguments[1], "--");
        assert_eq!(shortcut.arguments[2], "hello");
    }

    #[test]
    fn bundle_reference_cannot_escape_its_directory() {
        for path in [
            "@bundle/../escape",
            "@bundle/foo/../../escape",
            "@bundle/",
            "@bundle//x",
        ] {
            let mut shortcut = Shortcut::from_str(SHORTCUT).unwrap();
            shortcut.arguments[0] = path.into();
            assert!(
                shortcut
                    .resolve_bundle_paths(Path::from_str("/applications/bundles/test"))
                    .is_err()
            );
        }
=======
mod icon_metadata_tests {
    use super::*;
    use xila::graphics::IconMotif;

    const BASE: &str = r##"{"name":"Calculator","command":"/wasm","arguments":[],"terminal":false,"icon":{"version":1,"mode":"procedural","motif":"rings","monogram":"C","background":"#4F46E5","foreground":"#FFFFFF","accent":"#A5B4FC","seed":17}}"##;

    #[test]
    fn explicit_icon_is_preserved_without_legacy_fields() {
        let shortcut = Shortcut::from_str(BASE).unwrap();
        let (icon, explicit) = shortcut.icon_spec("calculator.json");
        assert!(explicit);
        assert_eq!(icon.motif, IconMotif::Rings);
        assert_eq!(icon.text(), "C");
        assert_eq!(icon.background, Color::new(0x4f, 0x46, 0xe5));
        assert_eq!(icon.seed, 17);
    }

    #[test]
    fn malformed_custom_icon_falls_back() {
        let source = BASE.replace("\"rings\"", "\"unknown\"");
        let shortcut = Shortcut::from_str(&source).unwrap();
        let (_, explicit) = shortcut.icon_spec("calculator.json");
        assert!(!explicit);
>>>>>>> a3a5a74 (feat(shell): render application-provided procedural icons)
    }
}

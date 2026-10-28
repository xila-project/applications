use alloc::{string::{String, ToString}, vec::Vec};
use miniserde::{Deserialize, Serialize};
use xila::file_system::{AccessFlags, Path};
use xila::graphics::Color;
use xila::virtual_file_system::File;

use crate::error::{Error, Result};

pub const SHORTCUT_PATH: &Path = Path::from_str("/configuration/shared/applications/shortcuts");

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Shortcut {
    pub name: String,
    pub command: String,
    #[serde(rename = "terminal")]
    pub _terminal: bool,
    pub arguments: Vec<String>,
    pub icon_string: String,
    icon_color: [u8; 3],
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

        shortcut_file
            .read_to_end(buffer, 2048)
            .await
            .map_err(Error::FailedToReadShortcutFile)?;

        shortcut_file
            .close(virtual_file_system)
            .await
            .map_err(Error::FailedToReadShortcutFile)?;

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

    /// Resolve bundle-relative executable/asset arguments against the
    /// directory containing this shortcut. Never interpret a manifest here.
    pub fn resolve_bundle_paths(&mut self, bundle: &Path) -> Result<()> {
        fn resolve(value: &str, bundle: &Path) -> Result<String> {
            let Some(relative) = value.strip_prefix("@bundle/") else {
                return Ok(value.to_string());
            };
            if relative.is_empty()
                || relative.contains('\\')
                || relative.split('/').any(|segment| segment.is_empty() || segment == "." || segment == "..")
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
    }

    // pub fn is_terminal(&self) -> bool {
    //     self._Terminal
    // }
}

#[cfg(test)]
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
        for path in ["@bundle/../escape", "@bundle/foo/../../escape", "@bundle/", "@bundle//x"] {
            let mut shortcut = Shortcut::from_str(SHORTCUT).unwrap();
            shortcut.arguments[0] = path.into();
            assert!(shortcut.resolve_bundle_paths(Path::from_str("/applications/bundles/test")).is_err());
        }
    }
}

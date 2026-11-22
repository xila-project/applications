//! `package info` output, shared by installed manifests and registry entries
//! so both describe a package the same way.

use alloc::{
    format,
    string::{String, ToString},
    vec::Vec,
};
use xila_package_manifest::{Manifest, Metadata, Origin, RegistryEntry, XilaVersion};

use crate::repository::Offer;

/// The fields an installed manifest and a registry entry have in common.
pub struct Details<'a> {
    pub name: &'a str,
    pub version: &'a str,
    pub display_name: Option<&'a str>,
    pub target: &'a str,
    pub xila_version: XilaVersion,
    pub capabilities: &'a [String],
    pub metadata: Option<&'a Metadata>,
}

impl<'a> From<&'a Manifest> for Details<'a> {
    fn from(manifest: &'a Manifest) -> Self {
        Self {
            name: &manifest.name,
            version: &manifest.version,
            display_name: manifest
                .application
                .as_ref()
                .map(|application| application.display_name.as_str()),
            target: &manifest.target,
            xila_version: manifest.xila_version,
            capabilities: &manifest.capabilities,
            metadata: manifest.metadata.as_ref(),
        }
    }
}

impl<'a> From<&'a RegistryEntry> for Details<'a> {
    fn from(entry: &'a RegistryEntry) -> Self {
        Self {
            name: &entry.name,
            version: &entry.version,
            display_name: entry.display_name.as_deref(),
            target: &entry.target,
            xila_version: entry.xila_version,
            capabilities: &entry.capabilities,
            metadata: entry.metadata.as_ref(),
        }
    }
}

/// `label: value` lines describing `details`, followed by `extra` lines.
pub fn render(details: &Details<'_>, extra: &[(&str, String)]) -> Vec<String> {
    let mut lines = Vec::new();
    let mut push = |label: &str, value: &str| lines.push(format!("{label:<16}{value}"));
    push("name:", details.name);
    push("version:", details.version);
    if let Some(display_name) = details.display_name {
        push("display name:", display_name);
    }
    if let Some(metadata) = details.metadata {
        push("summary:", &metadata.summary);
        push("license:", &metadata.license);
        push("source:", &metadata.source);
        if let Some(homepage) = &metadata.homepage {
            push("homepage:", homepage);
        }
    }
    push("target:", details.target);
    push("xila version:", &details.xila_version.to_string());
    let capabilities = if details.capabilities.is_empty() {
        "none".to_string()
    } else {
        details.capabilities.join(", ")
    };
    push("capabilities:", &capabilities);
    for (label, value) in extra {
        push(label, value);
    }
    lines
}

/// Lines describing an installed package from its verified manifest and
/// recorded origin; no network access.
pub fn installed(manifest: &Manifest, origin: Option<&Origin>) -> Vec<String> {
    let bytes: u64 = manifest.files.iter().map(|file| file.size).sum();
    let source = match origin {
        Some(Origin {
            repository: Some(repository),
            ..
        }) => repository.clone(),
        Some(_) => "local archive".to_string(),
        None => "unknown (no origin record)".to_string(),
    };
    let mut extra = alloc::vec![
        ("status:", "installed".to_string()),
        ("installed from:", source),
        (
            "files:",
            format!("{} ({bytes} bytes)", manifest.files.len())
        ),
    ];
    if let Some(origin) = origin {
        extra.push(("archive sha256:", origin.archive_sha256.clone()));
    }
    render(&Details::from(manifest), &extra)
}

/// Lines describing a registry entry offered by `repository`.
pub fn available(entry: &RegistryEntry, repository: &str, installed: Option<&str>) -> Vec<String> {
    let status = match installed {
        Some(version) => format!("available ({version} installed)"),
        None => "available".to_string(),
    };
    render(
        &Details::from(entry),
        &[
            ("status:", status),
            ("repository:", repository.to_string()),
            ("publisher:", entry.publisher.clone()),
            ("archive:", format!("{} bytes", entry.archive.size)),
            ("archive sha256:", entry.archive.sha256.clone()),
        ],
    )
}

/// The warnings and blocks `package info` prints for the repositories'
/// offers of `wanted`: one block per repository that has the package and
/// one warning per repository that could not be checked, which is skipped
/// like in `package search`. An error only when no repository has it.
pub fn offered(
    offers: &[Offer],
    wanted: &str,
    installed: Option<&str>,
) -> (Vec<String>, Result<Vec<Vec<String>>, String>) {
    let mut blocks = Vec::new();
    let mut warnings = Vec::new();
    for offer in offers {
        match &offer.entry {
            Ok(Some(entry)) => blocks.push(available(entry, &offer.repository, installed)),
            Ok(None) => {}
            Err(problem) => warnings.push(format!(
                "package: skipping repository {}: {problem}",
                offer.repository
            )),
        }
    }
    let result = if !blocks.is_empty() {
        Ok(blocks)
    } else if !offers.is_empty() && warnings.len() == offers.len() {
        Err(format!(
            "cannot look up {wanted}: no configured repository could be checked"
        ))
    } else if !warnings.is_empty() {
        Err(format!(
            "{wanted} not found for this device in the repositories that could be checked"
        ))
    } else {
        Err(format!(
            "{wanted} not found for this device in the configured repositories"
        ))
    };
    (warnings, result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use xila_package_manifest::{
        Application, Archive, FORMAT_VERSION, ManifestFile, REGISTRY_FORMAT_VERSION,
    };

    fn metadata() -> Metadata {
        Metadata {
            summary: "A clock".into(),
            license: "Apache-2.0".into(),
            source: "https://github.com/xila-project/applications".into(),
            homepage: None,
        }
    }

    fn manifest() -> Manifest {
        Manifest {
            schema: None,
            format_version: FORMAT_VERSION,
            name: "xila.clock".into(),
            version: "0.1.0".into(),
            target: "wasm32-wasip1".into(),
            xila_version: XilaVersion::new(0, 1),
            capabilities: alloc::vec![],
            files: alloc::vec![
                ManifestFile {
                    path: "executables/clock.wasm".into(),
                    size: 1000,
                    sha256: "a".repeat(64),
                },
                ManifestFile {
                    path: "icon.png".into(),
                    size: 24,
                    sha256: "b".repeat(64),
                },
            ],
            application: Some(Application {
                entrypoint: "executables/clock.wasm".into(),
                display_name: "Clock".into(),
                icon: "icon.png".into(),
                show_in_launcher: true,
            }),
            metadata: Some(metadata()),
        }
    }

    fn entry() -> RegistryEntry {
        RegistryEntry {
            schema: None,
            format_version: REGISTRY_FORMAT_VERSION,
            name: "xila.clock".into(),
            version: "0.2.0".into(),
            target: "wasm32-wasip1".into(),
            xila_version: XilaVersion::new(0, 1),
            capabilities: alloc::vec!["network".into(), "time".into()],
            display_name: None,
            metadata: None,
            publisher: "xila".into(),
            archive: Archive {
                url: "https://xila.example/clock.xpk".into(),
                sha256: "c".repeat(64),
                size: 4096,
            },
        }
    }

    fn value<'a>(lines: &'a [String], label: &str) -> Option<&'a str> {
        lines
            .iter()
            .find_map(|line| line.strip_prefix(label))
            .map(str::trim_start)
    }

    #[test]
    fn describes_an_installed_package_offline() {
        let origin = Origin {
            schema: None,
            format_version: 1,
            repository: Some("xila".into()),
            name: "xila.clock".into(),
            version: "0.1.0".into(),
            archive_sha256: "d".repeat(64),
        };
        let lines = installed(&manifest(), Some(&origin));
        assert_eq!(lines[0], "name:           xila.clock");
        assert_eq!(value(&lines, "version:"), Some("0.1.0"));
        assert_eq!(value(&lines, "display name:"), Some("Clock"));
        assert_eq!(value(&lines, "summary:"), Some("A clock"));
        assert_eq!(value(&lines, "homepage:"), None);
        assert_eq!(value(&lines, "xila version:"), Some("0.1"));
        assert_eq!(value(&lines, "capabilities:"), Some("none"));
        assert_eq!(value(&lines, "installed from:"), Some("xila"));
        assert_eq!(value(&lines, "files:"), Some("2 (1024 bytes)"));
        assert_eq!(
            value(&lines, "archive sha256:"),
            Some("d".repeat(64).as_str())
        );

        let mut local = origin.clone();
        local.repository = None;
        let lines = installed(&manifest(), Some(&local));
        assert_eq!(value(&lines, "installed from:"), Some("local archive"));
        let lines = installed(&manifest(), None);
        assert_eq!(
            value(&lines, "installed from:"),
            Some("unknown (no origin record)")
        );
        assert_eq!(value(&lines, "archive sha256:"), None);
    }

    #[test]
    fn describes_a_registry_entry() {
        let lines = available(&entry(), "xila", Some("0.1.0"));
        assert_eq!(value(&lines, "version:"), Some("0.2.0"));
        assert_eq!(value(&lines, "display name:"), None);
        assert_eq!(value(&lines, "summary:"), None);
        assert_eq!(value(&lines, "capabilities:"), Some("network, time"));
        assert_eq!(
            value(&lines, "status:"),
            Some("available (0.1.0 installed)")
        );
        assert_eq!(value(&lines, "repository:"), Some("xila"));
        assert_eq!(value(&lines, "publisher:"), Some("xila"));
        assert_eq!(value(&lines, "archive:"), Some("4096 bytes"));
        let lines = available(&entry(), "community", None);
        assert_eq!(value(&lines, "status:"), Some("available"));
    }

    #[test]
    fn describes_a_match_when_another_repository_fails() {
        let offer = |repository: &str, entry: Result<Option<RegistryEntry>, String>| Offer {
            repository: repository.into(),
            entry,
        };
        let (warnings, blocks) = offered(
            &[
                offer("offline", Err("cannot reach offline".into())),
                offer("empty", Ok(None)),
                offer("xila", Ok(Some(entry()))),
            ],
            "xila.clock",
            None,
        );
        assert_eq!(
            warnings,
            ["package: skipping repository offline: cannot reach offline"]
        );
        let blocks = blocks.unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(value(&blocks[0], "repository:"), Some("xila"));

        let (warnings, result) = offered(
            &[
                offer("offline", Err("down".into())),
                offer("empty", Ok(None)),
            ],
            "xila.clock",
            None,
        );
        assert_eq!(warnings.len(), 1);
        assert_eq!(
            result.unwrap_err(),
            "xila.clock not found for this device in the repositories that could be checked"
        );
        let (_, result) = offered(&[offer("offline", Err("down".into()))], "xila.clock", None);
        assert_eq!(
            result.unwrap_err(),
            "cannot look up xila.clock: no configured repository could be checked"
        );
        let (warnings, result) = offered(&[offer("empty", Ok(None))], "xila.clock", None);
        assert!(warnings.is_empty());
        assert_eq!(
            result.unwrap_err(),
            "xila.clock not found for this device in the configured repositories"
        );
    }
}

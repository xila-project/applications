//! Small, explicit CLI parser for the native Xila package executable.
//! No global bypass: callers must opt into unauthenticated installation.
use alloc::string::String;

/// The key path is explicitly trusted by the invoking administrator.
/// Without it, the OS-wide trusted publisher key is used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command<'a> {
    Install {
        archive: &'a str,
        public_key: Option<&'a str>,
        allow_untrusted: bool,
    },
    List,
    Help,
}

pub const USAGE: &str = "Usage:\n  package install <archive.xpk> [--public-key <trusted-key-file> | --allow-untrusted]\n  package list\n  package --help\n\nVerification is performed at installation only. By default the installer\nuses /configuration/system/packages/trusted.pub (raw Ed25519 public key).\n--public-key explicitly approves another signer for this invocation.\n--allow-untrusted skips publisher authentication but NEVER skips SHA-256,\nTAR structure checks or the package manifest validation.\n";

pub fn parse(args: &[String]) -> Result<Command<'_>, &'static str> {
    match args.first().map(String::as_str) {
        Some("--help" | "-h" | "help") if args.len() == 1 => Ok(Command::Help),
        Some("list") if args.len() == 1 => Ok(Command::List),
        Some("install") if args.len() >= 2 => {
            let archive = args[1].as_str();
            if archive.is_empty() || archive.starts_with('-') {
                return Err("install requires an archive path");
            }
            let mut allow_untrusted = false;
            let mut public_key = None;
            let mut i = 2;
            while i < args.len() {
                match args[i].as_str() {
                    "--allow-untrusted" if !allow_untrusted => {
                        allow_untrusted = true;
                        i += 1;
                    }
                    "--public-key" if public_key.is_none() && i + 1 < args.len() => {
                        let key = args[i + 1].as_str();
                        if key.is_empty() || key.starts_with('-') {
                            return Err("--public-key requires a path");
                        }
                        public_key = Some(key);
                        i += 2;
                    }
                    _ => return Err("unknown, missing or repeated install option"),
                }
            }
            if allow_untrusted && public_key.is_some() {
                return Err("--allow-untrusted and --public-key are mutually exclusive");
            }
            Ok(Command::Install {
                archive,
                public_key,
                allow_untrusted,
            })
        }
        _ => Err("unknown package command"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::{string::ToString, vec, vec::Vec};
    fn args(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|part| (*part).to_string()).collect()
    }

    #[test]
    fn default_install_requires_a_trusted_signer() {
        assert_eq!(
            parse(&args(&["install", "/temporary/example.xpk"])),
            Ok(Command::Install {
                archive: "/temporary/example.xpk",
                public_key: None,
                allow_untrusted: false
            })
        );
    }

    #[test]
    fn unfamiliar_publisher_requires_an_explicit_override_or_key() {
        assert_eq!(
            parse(&args(&[
                "install",
                "/temporary/example.xpk",
                "--allow-untrusted"
            ])),
            Ok(Command::Install {
                archive: "/temporary/example.xpk",
                public_key: None,
                allow_untrusted: true,
            })
        );
        assert_eq!(
            parse(&args(&[
                "install",
                "x.xpk",
                "--public-key",
                "/keys/user.pub"
            ])),
            Ok(Command::Install {
                archive: "x.xpk",
                public_key: Some("/keys/user.pub"),
                allow_untrusted: false,
            })
        );
        assert!(
            parse(&args(&[
                "install",
                "x.xpk",
                "--allow-untrusted",
                "--public-key",
                "/keys/test.pub"
            ]))
            .is_err()
        );
    }

    #[test]
    fn reject_ambiguous_flags() {
        for parts in [
            vec!["install", "x.xpk", "--allow-untrusted", "--allow-untrusted"],
            vec!["install", "x.xpk", "--public-key"],
            vec!["install", "x.xpk", "--unknown"],
            vec!["install", "--allow-untrusted"],
            vec!["install", "x.xpk", "--public-key", "a", "--public-key", "b"],
        ] {
            assert!(parse(&args(&parts)).is_err());
        }
        assert!(matches!(parse(&args(&["help"])), Ok(Command::Help)));
        assert!(matches!(parse(&args(&["list"])), Ok(Command::List)));
    }
}

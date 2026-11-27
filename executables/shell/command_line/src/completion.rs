use alloc::{string::String, vec::Vec};
use core::{ops::Range, time::Duration};
use embassy_futures::select::{Either, select};
use xila::{
    file_system::{Kind, Path},
    virtual_file_system::Directory,
};

use crate::{
    Shell,
    commands::BUILTINS,
    tokenizer::{self, Kind as TokenKind, Segment, Segments},
};

const DISPLAY_LIMIT: usize = 12;
const MATCH_LIMIT: usize = 64;
const MATCH_BYTE_LIMIT: usize = 4096;
const DIRECTORY_LIMIT: usize = 16;
const ENTRY_LIMIT: usize = 256;
const SEARCH_TIMEOUT: Duration = Duration::from_millis(150);

struct Word {
    range: Range<usize>,
    prefix: String,
    command: bool,
}

impl Word {
    fn at(line: &str, cursor: usize) -> Option<Self> {
        let mut closed_word;
        let tokens = match tokenizer::tokenize(line) {
            Ok(tokens) => tokens,
            Err(_) if cursor == line.len() => {
                // The tokenizer needs a closing quote. The user can complete an
                // open quoted word at the end of the line before typing it.
                closed_word = String::from(line);
                closed_word.push('\'');
                if tokenizer::tokenize(&closed_word).is_err() {
                    closed_word.pop();
                    closed_word.push('"');
                }
                tokenizer::tokenize(&closed_word).ok()?
            }
            Err(_) => return None,
        };
        let mut command = true;
        let mut redirect = false;
        let mut previous_end = 0;
        for token in tokens {
            let start = token.position;
            if cursor < start {
                break;
            }
            match token.kind {
                TokenKind::Word(source) => {
                    previous_end = (start + source.len()).min(line.len());
                    if cursor <= previous_end {
                        let mut prefix = String::new();
                        for segment in Segments::new(&line[start..cursor]) {
                            match segment.ok()? {
                                Segment::Literal(text) => prefix.push_str(text),
                                Segment::Variable(_) | Segment::Status => return None,
                            }
                        }
                        return Some(Self {
                            // Replace only up to the cursor, as readline does:
                            // the rest of the word stays as typed.
                            range: start..cursor,
                            prefix,
                            command: command && !redirect,
                        });
                    }
                    if !redirect {
                        command = false;
                    }
                    redirect = false;
                }
                kind => {
                    let length = match kind {
                        TokenKind::And
                        | TokenKind::Or
                        | TokenKind::Append
                        | TokenKind::ErrorOutput => 2,
                        TokenKind::ErrorAppend => 3,
                        _ => 1,
                    };
                    previous_end = start + length;
                    if cursor < previous_end {
                        return None;
                    }
                    redirect = kind.is_redirect();
                    command |= !redirect;
                }
            }
        }
        // The tokenizer skips comments. Ignore the cursor inside one.
        if !line[previous_end..cursor]
            .bytes()
            .all(|byte| byte.is_ascii_whitespace())
            || line[cursor..].starts_with('#')
        {
            return None;
        }
        Some(Self {
            range: cursor..cursor,
            prefix: String::new(),
            command: command && !redirect,
        })
    }
}

#[derive(Default)]
pub(super) struct Matches {
    names: Vec<String>,
    bytes: usize,
    incomplete: bool,
}

impl Matches {
    fn add(&mut self, name: &str, prefix: &str, directory: bool) {
        if !name.starts_with(prefix)
            || matches!(name, "." | "..")
            || name.chars().any(char::is_control)
            || (name.starts_with('.') && !prefix.starts_with('.'))
        {
            return;
        }
        if self.names.iter().any(|candidate| {
            if directory {
                candidate.strip_suffix('/') == Some(name)
            } else {
                candidate == name
            }
        }) {
            return;
        }
        let length = name.len() + usize::from(directory);
        if self.names.len() == MATCH_LIMIT || self.bytes + length > MATCH_BYTE_LIMIT {
            self.incomplete = true;
            return;
        }
        self.bytes += length;
        let mut candidate = String::with_capacity(length);
        candidate.push_str(name);
        if directory {
            candidate.push('/');
        }
        self.names.push(candidate);
    }

    pub fn is_ambiguous(&self) -> bool {
        self.names.len() > 1 || (self.incomplete && !self.names.is_empty())
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }

    pub fn visible(&self) -> &[String] {
        &self.names[..self.names.len().min(DISPLAY_LIMIT)]
    }

    pub fn hidden_count(&self) -> usize {
        self.names.len().saturating_sub(DISPLAY_LIMIT)
    }

    pub fn is_incomplete(&self) -> bool {
        self.incomplete
    }

    fn common_prefix(&self) -> Option<&str> {
        if self.incomplete {
            return None;
        }
        let first = self.names.first()?;
        let mut length = first.len();
        for name in &self.names[1..] {
            length = first.as_bytes()[..length]
                .iter()
                .zip(name.bytes())
                .take_while(|(first, second)| **first == *second)
                .count();
        }
        Some(&first[..first.floor_char_boundary(length)])
    }
}

pub(super) struct Completion {
    pub replacement: Option<(Range<usize>, String)>,
    pub matches: Matches,
}

impl Completion {
    fn new(line: &str, word: Word, matches: Matches) -> Self {
        let (parent, prefix) = split_path(&word.prefix);
        let replacement = matches.common_prefix().and_then(|common| {
            let unique = matches.names.len() == 1;
            if !unique && common.len() == prefix.len() {
                return None;
            }
            let directory = common.ends_with('/');
            let mut replacement = String::new();
            for character in parent.chars().chain(common.chars()) {
                if character.is_ascii_whitespace() || "'\"\\|&;<>#$`".contains(character) {
                    replacement.push('\\');
                }
                replacement.push(character);
            }
            if unique && !directory {
                replacement.push(' ');
                if line[word.range.end..]
                    .starts_with(|character: char| character.is_ascii_whitespace())
                {
                    replacement.pop();
                }
            }
            Some((word.range, replacement))
        });
        Self {
            replacement,
            matches,
        }
    }
}

fn split_path(path: &str) -> (&str, &str) {
    path.rsplit_once('/').map_or(("", path), |(parent, name)| {
        (&path[..parent.len() + 1], name)
    })
}

impl Shell {
    pub(super) async fn complete(&mut self, line: &str, cursor: usize) -> Option<Completion> {
        let word = Word::at(line, cursor)?;
        let (parent, prefix) = split_path(&word.prefix);
        let mut matches = Matches::default();
        if word.command && parent.is_empty() {
            for (name, _, _) in BUILTINS {
                matches.add(name, prefix, false);
            }
            matches.add("sudo", prefix, false);
        }
        let mut remaining_entries = ENTRY_LIMIT;
        let search = self.find_completion_matches(
            parent,
            prefix,
            word.command,
            &mut remaining_entries,
            &mut matches,
        );
        if matches!(
            select(search, xila::task::Manager::sleep(SEARCH_TIMEOUT)).await,
            Either::Second(())
        ) {
            matches.incomplete = true;
        }
        matches.names.sort_unstable();
        Some(Completion::new(line, word, matches))
    }

    async fn find_completion_matches(
        &mut self,
        parent: &str,
        prefix: &str,
        command: bool,
        remaining_entries: &mut usize,
        matches: &mut Matches,
    ) {
        let user = if command {
            let Ok(user) = self.context.task_manager.get_user(self.task).await else {
                return;
            };
            Some(user)
        } else {
            None
        };
        let mut search = Search {
            context: self.context,
            task: self.task,
            user,
            prefix,
            remaining_entries,
            matches,
        };
        if command && parent.is_empty() {
            if let Ok(paths) = self
                .context
                .task_manager
                .get_environment_variable(self.task, "Paths")
                .await
            {
                for (index, path) in paths
                    .get_value()
                    .split(':')
                    .filter(|path| !path.is_empty())
                    .enumerate()
                {
                    if index == DIRECTORY_LIMIT || *search.remaining_entries == 0 {
                        search.matches.incomplete = true;
                        break;
                    }
                    search.scan(Path::from_str(path), false).await;
                }
            }
        } else {
            let path = Path::from_str(parent);
            if path.is_absolute() {
                search.scan(path, true).await;
            } else if parent.is_empty() {
                search.scan(&self.current_directory, true).await;
            } else if let Some(path) = self.current_directory.clone().join(path) {
                search.scan(&path, true).await;
            }
        }
    }
}

struct Search<'a> {
    context: &'static xila::executable::ExecutableContext,
    task: xila::task::TaskIdentifier,
    user: Option<xila::users::UserIdentifier>,
    prefix: &'a str,
    remaining_entries: &'a mut usize,
    matches: &'a mut Matches,
}

impl Search<'_> {
    async fn scan(&mut self, path: &Path, include_directories: bool) {
        let context = self.context;
        let filesystem = context.virtual_file_system;
        // Dropping the listing closes it without waiting, including when the
        // search deadline cancels a pending read.
        let Ok(mut directory) = Directory::open(filesystem, self.task, path).await else {
            return;
        };
        while !self.matches.incomplete && *self.remaining_entries > 0 {
            let entry = match directory.read().await {
                Ok(Some(entry)) => entry,
                Ok(None) => return,
                Err(_) => {
                    self.matches.incomplete = true;
                    return;
                }
            };
            *self.remaining_entries -= 1;
            if !entry.name.starts_with(self.prefix) || matches!(entry.name.as_str(), "." | "..") {
                continue;
            }
            if entry.kind == Kind::Directory {
                if include_directories {
                    self.matches.add(&entry.name, self.prefix, true);
                }
                continue;
            }
            if let Some(user) = self.user {
                if !matches!(
                    entry.kind,
                    Kind::File | Kind::SymbolicLink | Kind::CharacterDevice
                ) {
                    continue;
                }
                let Some(path) = entry.join_path(path) else {
                    continue;
                };
                let Ok(statistics) = filesystem.get_statistics(&path).await else {
                    continue;
                };
                if !xila::executable::is_execute_allowed(context.users_manager, &statistics, user)
                    .await
                {
                    continue;
                }
            }
            self.matches.add(&entry.name, self.prefix, false);
        }
        self.matches.incomplete = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn complete(line: &str, cursor: usize, names: &[&str]) -> Completion {
        let word = Word::at(line, cursor).unwrap();
        let (_, prefix) = split_path(&word.prefix);
        let mut matches = Matches::default();
        for name in names {
            matches.add(name, prefix, false);
        }
        Completion::new(line, word, matches)
    }

    #[test]
    fn completes_one_match_and_leaves_zero_matches_alone() {
        let result = complete("ec", 2, &["echo", "exit"]);
        assert_eq!(result.replacement, Some((0..2, String::from("echo "))));
        assert!(!result.matches.is_ambiguous());
        let result = complete("zz", 2, &["echo", "exit"]);
        assert_eq!(result.replacement, None);
        assert!(result.matches.is_empty());
        assert!(result.matches.visible().is_empty());
    }

    #[test]
    fn completes_a_common_prefix_and_keeps_distinct_matches() {
        let result = complete("echo hello | ca", 15, &["cat", "catch", "cat"]);
        assert_eq!(result.replacement, Some((13..15, String::from("cat"))));
        assert!(result.matches.is_ambiguous());
        assert_eq!(result.matches.visible(), ["cat", "catch"]);
        let result = complete("x", 1, &["xé", "xê"]);
        assert_eq!(result.replacement, None);
    }

    #[test]
    fn completes_mid_word_only_up_to_the_cursor() {
        // The cursor is after "ec": the match replaces "ec" and leaves "X".
        let result = complete("ecX", 2, &["echo", "echidna"]);
        assert_eq!(result.replacement, Some((0..2, String::from("ech"))));
        let result = complete("ecX", 2, &["echo"]);
        assert_eq!(result.replacement, Some((0..2, String::from("echo "))));
    }

    #[test]
    fn completes_one_path_component_and_preserves_the_next_argument() {
        let result = complete("cp ../aX /output", 7, &["a file"]);
        assert_eq!(
            result.replacement,
            Some((3..7, String::from("../a\\ file ")))
        );
        assert!(!Word::at("echo > lo", 9).unwrap().command);
        assert!(Word::at("echo; ec", 8).unwrap().command);
        assert!(Word::at("echo # ec", 9).is_none());
    }

    #[test]
    fn completes_directories_and_open_quoted_words() {
        for line in ["cd 'test f", "cd \"test f"] {
            let word = Word::at(line, line.len()).unwrap();
            let mut matches = Matches::default();
            matches.add("test folder", "test f", true);
            let result = Completion::new(line, word, matches);
            assert_eq!(
                result.replacement,
                Some((3..line.len(), String::from("test\\ folder/")))
            );
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    #[xila::task::test(task_path = xila::task)]
    async fn finds_executables_in_search_paths_and_file_arguments_in_the_current_directory() {
        use xila::{
            executable::mount_executables,
            file_system::{AccessFlags, CreateFlags, Flags},
            virtual_file_system::File,
        };

        let standard = testing::initialize(false, false).await;
        let context = standard.context;
        let task = context.task_manager.get_current_task_identifier().await;
        let filesystem = context.virtual_file_system;
        Directory::create(filesystem, task, "/completion")
            .await
            .unwrap();
        Directory::create(filesystem, task, "/completion/zeta_folder")
            .await
            .unwrap();
        File::open(
            filesystem,
            task,
            "/completion/zeta_file",
            Flags::new(AccessFlags::Write, Some(CreateFlags::Create), None),
        )
        .await
        .unwrap()
        .close(filesystem)
        .await
        .unwrap();
        mount_executables!(
            context,
            filesystem,
            task,
            &[("/completion/zeta_tool", crate::ShellExecutable)]
        )
        .await
        .unwrap();
        for (name, value) in [
            ("Paths", "/missing:/completion:/completion"),
            ("Current_directory", "/completion"),
        ] {
            context
                .task_manager
                .set_environment_variable(task, name, value)
                .await
                .unwrap();
        }

        let mut shell = Shell::new(standard).await;
        let result = shell.complete("zeta", 4).await.unwrap();
        assert_eq!(result.matches.visible(), ["zeta_tool"]);
        assert_eq!(result.replacement, Some((0..4, String::from("zeta_tool "))));

        let result = shell.complete("cat zeta_f", 10).await.unwrap();
        assert_eq!(result.matches.visible(), ["zeta_file", "zeta_folder/"]);
        assert!(result.matches.is_ambiguous());
    }

    #[test]
    fn bounds_matches_and_shows_how_many_are_hidden() {
        let mut matches = Matches::default();
        for index in 0..MATCH_LIMIT + 1 {
            matches.add(&alloc::format!("file{index}"), "", false);
        }
        assert_eq!(matches.visible().len(), DISPLAY_LIMIT);
        assert_eq!(matches.hidden_count(), MATCH_LIMIT - DISPLAY_LIMIT);
        assert!(matches.is_incomplete());
        assert!(matches.common_prefix().is_none());
    }
}

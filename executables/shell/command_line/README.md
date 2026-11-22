# Command-line shell

The shell runs built-in commands and native or WebAssembly executables through Xila's task and virtual filesystem APIs. Its parser supports `no_std` and introduces no additional dependencies.

Run a VFS script with `shell -s PATH`; relative paths use the shell's current directory. `shell -c COMMAND` continues to parse one explicit command string, and other already-separated executable arguments remain literal. Script lines use the same parser and commands as interactive input, including chaining, redirection, expansion, comments, quoting and escaping.

## Composing commands

```sh
export NAME='hello world'; echo "$NAME"
echo first > /temporary/example; echo second >> /temporary/example
cat < /temporary/example | wc -l
missing-command 2> /temporary/errors || echo recovered
long-running-command > /temporary/result & echo foreground
```

`|` binds more tightly than `&&` and `||`. The conditional operators have equal precedence and evaluate left to right. `;` separates sequential command lists; `&` starts the preceding conditional list in a background task. Operators do not need surrounding spaces. Redirections belong to a single command, are applied left to right, and override its pipeline endpoint when they target the same stream.

- `< path` reads standard input from a file.
- `> path` creates or truncates standard output; `>> path` appends.
- `2> path` and `2>> path` redirect standard error.
- `cat` and `wc` consume standard input when no file is given, or when a file argument is `-`. Other built-ins retain their existing input interfaces.

A zero exit status means success. Built-in failures return 1, unknown commands return 127, and malformed source passed to `shell -c` or a script returns 2. External executable statuses are preserved within 0–255. A pipeline returns its last stage's status and waits for every stage. `true` and `false` provide status 0 and 1. `$?` expands to the preceding foreground status; successfully launching a background list sets it to zero. Script command failures continue by default; the script returns the final executed command's status. Syntax, invalid UTF-8, read and overlong-line errors stop the script with status 2 and identify its path and line. `exit` stops the script and returns the current command status.

Foreground built-ins can change the shell's current directory and environment. A launched shell starts in the caller's inherited `Current_directory`, falling back to `/` when it is absent or invalid. A script keeps directory and environment changes across its lines in the launched shell task; the calling shell keeps its own state. Script source is read from its VFS file, while commands inherit the shell's standard input independently. Pipeline stages and background lists run in child tasks with inherited environments and isolated shell state. Background input is `/devices/standard_io/null`; output and error inherit the shell's streams unless redirected and may interleave with foreground output. Completed background tasks are reaped as new jobs start. Shell exit waits for outstanding jobs; job listing, cancellation and foreground/background switching are separate work.

## Arguments and expansion

Single quotes preserve their contents literally. Double quotes allow variable expansion. A backslash outside single quotes protects the following character; inside double quotes it protects `$`, `"` and `\`. A `#` at a word boundary starts a comment.

`$NAME`, `${NAME}` and `$?` expand when a command is prepared for execution. Names use ASCII letters, digits and underscores, with a letter or underscore first. Missing variables expand to an empty string. Expansion retains one argument per source word, including empty arguments: it does not split variable values on whitespace or interpret their contents as shell operators. Quoted and unquoted fragments can form one word.

Calling the executable with already separated arguments preserves those arguments literally. Use `shell -c 'COMMAND'` explicitly to parse one command string, or `shell -s PATH` for a script file. Interactive input and scripts are parsed one line at a time; both execute a final unterminated line. LF and CRLF line endings are supported. Script lines are limited to 4096 source bytes excluding CRLF. Nested non-interactive shell invocations are limited to four active levels; completed invocations do not consume the limit.

This is a bounded shell language, not a complete POSIX shell. Multiline syntax, substitutions, globbing, subshells, descriptor duplication such as `2>&1`, here-documents and shell assignment syntax are unsupported. Newline characters inside explicit command source currently count as whitespace; use `;` between commands.

## Memory and execution limits

| Resource                                             |      Limit |
| ---------------------------------------------------- | ---------: |
| Source line                                          | 4096 bytes |
| Script line buffer                                   | 4097 bytes including possible CR before LF |
| Tokens per line                                      |        256 |
| Commands per line                                    |         32 |
| Stages per pipeline                                  |          8 |
| Expanded arguments and redirection paths per command | 8192 bytes |
| Buffered formatted output per built-in               | 8192 bytes |
| Buffered bytes per pipe                              |        512 |
| Active background lists per shell                    |          8 |

The lexer borrows word slices and the parser validates one token vector without allocating a syntax tree. Execution allocates expanded words and task state only for the commands being prepared. Pipelines use concurrent tasks and bounded pipes with asynchronous backpressure; they do not collect complete command output in memory. Plain byte output streams directly. These limits bound shell-owned syntax and pipe storage, not allocations performed by individual commands, inherited environment values, executable runtimes or task infrastructure. Nested shell invocations have their own limits.

Duplicated file streams share their position and LittleFS context. Position storage becomes shared only on the first duplication, and duplicated streams reuse the existing LittleFS native cache rather than copying its pointers.

## Command discovery and utilities

Use `help` to list built-ins and executable entries in the task's `Paths` search
locations, or `help COMMAND` for built-in usage. Help does not launch commands.
External commands supply their own usage; names present in multiple search
locations may appear more than once. Built-in names and help use one registry.

Examples:

```text
sleep 2
sleep 250ms
chmod 0640 notes.txt
tree -L 2 /configuration
find /configuration --name settings.toml --type f
realpath ../notes.txt /configuration
cp notes.txt /temporary/notes-copy.txt
help chmod
```

`realpath PATH...` prints canonical absolute paths for existing files and
directories. Relative operands are resolved from the current directory; VFS
metadata checks report missing paths and access errors.

`sleep` accepts one nonnegative integer with `ms`, `s`, `m`, or `h`; omitted
units mean seconds. Fractions, multiple durations, and values exceeding the
underlying timer's u64 nanosecond range are rejected. Waiting yields to other
tasks.

`chmod MODE PATH` accepts octal read/write/execute permissions from `000` through
`777`, optionally with a leading zero. It uses VFS ownership checks. Symbolic
modes, special bits, and recursive changes are unsupported.

`tree` includes hidden entries, skips `.` and `..`, and streams entries in
filesystem order. `-L N` limits levels below the selected directory; `-L 0`
prints only that directory. Default and maximum depth are 32, with paths bounded
to 1024 bytes. Traversal stops and reports filesystem or output errors. It keeps
one directory handle per active level rather than collecting sibling entries.

`find [PATH]` prints matching paths, starting at `PATH` or the current directory.
It includes the starting path when it matches, searches hidden entries, and
skips `.` and `..`. `-n NAME` or `--name NAME` matches an exact entry name;
`-t TYPE` or `--type TYPE` filters by `f` (file), `d` (directory), `c`
(character device), `b` (block device), `p` (pipe), `s` (socket), or `l`
(symbolic link). `-d N` or `--max-depth N` limits levels below the starting
path; the default and maximum are 32. Paths are bounded to 1024 bytes. Missing
or inaccessible paths and errors while reading a directory stop the command
with an error. Traversal streams results and retains at most one directory
handle per active level.

`mv SOURCE DESTINATION` renames a file or directory to the exact destination
path. Relative paths use the current directory. Both paths must belong to the
same mounted filesystem; the command reports the filesystem's rename error if
the source or destination cannot be renamed.

`cp SOURCE DESTINATION` streams a file into a new or truncated destination. If
the destination is an existing directory, the source filename is appended to
it. Directory sources, recursive copies, and copying a file onto itself are
unsupported. The copy uses a fixed 512-byte buffer.

## Running a command as root

`sudo [-u USER] COMMAND [ARGUMENT]...` runs one external command as root, or
as `USER`, for example
`sudo /applications/executables/package install xila.text-editor`.

- Only members of the `administrator` group may use it. Anyone else is refused
  before any prompt. A root caller is not prompted.
- It asks for the caller's own password. Standard input must be a terminal
  whose echo can be turned off: sudo turns it off, reads the attributes back,
  and writes the prompt only once echo is known to be off. A pipe, a file or a
  console without terminal attributes is refused before the prompt. Input that
  ends during the prompt aborts it. The password is never taken from the
  arguments or the environment.
- The password is at most 256 bytes. A longer line is read to its end while
  echo is still off, so that none of it reaches the shell, and refused. If
  reading fails during the prompt, the rest of the line cannot be discarded, so
  the shell ends instead of reading it as a command.
- Account records are readable by root only, so the password is checked in a
  short-lived task of its own that runs as root and runs nothing else.
- The Shell overwrites its password buffer and each input byte after use. The
  hash device that the check writes the password to keeps its own state; see
  xila-project/platforms#168.
- Each user has at most one attempt in progress, across every shell; a
  concurrent attempt is refused. An attempt counts as a failure from the moment
  its password is read until it succeeds, so interrupting the check cannot hide
  a failure. After 3 failures in a row, the user is locked out for 30 seconds,
  doubled by each further failure up to 5 minutes.
- The command is an entry of `/applications/executables`, given by name or by
  absolute path (`/applications/executables/NAME`). Relative paths, other
  directories, `..` components and built-ins are refused before the prompt. The
  caller's `Paths` and working directory are not used.
- The command runs in its own task with the target user and its primary group.
  Its environment is fixed: `User` and `HOME` from the target's account, `Host`
  set to `xila`, `Paths` set to `/applications/executables`, `Sudo_user` set to
  the caller's name, and `Current_directory` set to the target's home if it
  exists, `/` otherwise. The shell's own task keeps the caller's identity.
- WebAssembly guests never run under `sudo`, even when the Wasm runner is the
  command given to it. Core marks the command's task, and every task it starts,
  as running with a granted identity, and the Wasm runtime refuses to start a
  guest in a marked task. The mark cannot be cleared, so it does not depend on
  `Sudo_user`, which only tells the command who ran it. Core also refuses to
  launch a registered file format with another identity.
- Every attempt is logged with the caller, the target user, the command and
  the outcome, with the fields that the caller chooses escaped.
- Successful authentications are not remembered.

use super::*;
use alloc::{boxed::Box, format, vec};
use core::num::NonZeroUsize;
use xila::executable::{ExecutableTrait, MainFuture, mount_executables};

async fn shell() -> Shell {
    let mut standard = testing::initialize(false, false).await;
    let context = standard.context;
    let task = context.task_manager.get_current_task_identifier().await;
    standard.standard_in = File::open(
        context.virtual_file_system,
        task,
        "/devices/standard_io/null",
        AccessFlags::Read.into(),
    )
    .await
    .unwrap();
    standard.standard_out = File::open(
        context.virtual_file_system,
        task,
        "/shell-output",
        Flags::new(
            AccessFlags::READ_WRITE,
            Some(CreateFlags::CREATE_TRUNCATE),
            None,
        ),
    )
    .await
    .unwrap();
    standard.standard_error = File::open(
        context.virtual_file_system,
        task,
        "/shell-errors",
        Flags::new(
            AccessFlags::READ_WRITE,
            Some(CreateFlags::CREATE_TRUNCATE),
            None,
        ),
    )
    .await
    .unwrap();
    Shell::new(standard).await
}
async fn contents(shell: &Shell, path: &str) -> String {
    let mut output = Vec::new();
    File::read_from_path(
        shell.context.virtual_file_system,
        shell.task,
        path,
        &mut output,
    )
    .await
    .unwrap();
    String::from_utf8(output).unwrap()
}

async fn write_file(shell: &Shell, path: &str, bytes: &[u8]) {
    let mut file = File::open(
        shell.context.virtual_file_system,
        shell.task,
        path,
        Flags::new(AccessFlags::Write, Some(CreateFlags::CREATE_TRUNCATE), None),
    )
    .await
    .unwrap();
    write_all(&mut file, bytes).await.unwrap();
    file.close(shell.context.virtual_file_system).await.unwrap();
}

#[xila::task::test(task_path = xila::task)]
async fn launched_scripts_inherit_the_callers_directory() {
    let mut shell = shell().await;
    mount_executables!(
        shell.context,
        shell.context.virtual_file_system,
        shell.task,
        &[(&"/applications/executables/shell", crate::ShellExecutable)]
    )
    .await
    .unwrap();
    write_file(
        &shell,
        "/temporary/job",
        b"pwd\necho child > marker\ncd /\n",
    )
    .await;
    assert_eq!(shell.execute_line("cd /temporary").await.unwrap(), 0);

    for path in ["job", "/temporary/job"] {
        assert_eq!(
            shell
                .execute_line(&format!("/applications/executables/shell -s {path}"))
                .await
                .unwrap(),
            0
        );
        assert_eq!(contents(&shell, "/temporary/marker").await, "child\n");
        assert_eq!(shell.current_directory.as_str(), "/temporary");
    }
    assert_eq!(
        contents(&shell, "/shell-output").await,
        "/temporary\n/temporary\n"
    );
}

#[xila::task::test(task_path = xila::task)]
async fn sequential_shell_invocations_do_not_accumulate_nesting_depth() {
    let mut shell = shell().await;
    for _ in 0..6 {
        assert_eq!(shell.main(vec!["true".into()]).await, Ok(()));
    }
    assert!(
        shell
            .context
            .task_manager
            .get_environment_variable(shell.task, crate::SCRIPT_DEPTH_VARIABLE)
            .await
            .is_err()
    );
}

#[xila::task::test(task_path = xila::task)]
async fn shell_invocations_restore_inherited_depth_after_errors() {
    let mut shell = shell().await;
    shell
        .context
        .task_manager
        .set_environment_variable(shell.task, crate::SCRIPT_DEPTH_VARIABLE, "1")
        .await
        .unwrap();
    for arguments in [
        vec!["-s".into(), "/missing".into()],
        vec!["-c".into(), "echo '".into()],
    ] {
        assert!(shell.main(arguments).await.is_err());
        assert_eq!(
            shell
                .context
                .task_manager
                .get_environment_variable(shell.task, crate::SCRIPT_DEPTH_VARIABLE)
                .await
                .unwrap()
                .get_value(),
            "1"
        );
    }
}

#[xila::task::test(task_path = xila::task)]
async fn script_mode_streams_commands_and_keeps_shell_state_without_consuming_stdin() {
    let mut shell = shell().await;
    write_file(
        &shell,
        "/script",
        b"\n# comment\nexport NAME='hello world'\ncd /temporary\necho \"$NAME\"\ncat\n",
    )
    .await;

    assert_eq!(
        shell.main(vec!["-s".into(), "/script".into()]).await,
        Ok(())
    );
    assert_eq!(contents(&shell, "/shell-output").await, "hello world\n");
    assert_eq!(shell.current_directory.as_str(), "/temporary");
    assert_eq!(
        shell
            .context
            .task_manager
            .get_environment_variable(shell.task, "NAME")
            .await
            .unwrap()
            .get_value(),
        "hello world"
    );
}

#[xila::task::test(task_path = xila::task)]
async fn script_mode_handles_crlf_final_line_and_hash_quoting() {
    let mut shell = shell().await;
    write_file(
        &shell,
        "/script",
        b"echo \"a#b\" 'c#d' e\\#f # trailing comment\r\necho final",
    )
    .await;

    assert_eq!(
        shell.main(vec!["-s".into(), "/script".into()]).await,
        Ok(())
    );
    assert_eq!(
        contents(&shell, "/shell-output").await,
        "a#b c#d e#f\nfinal\n"
    );
}

#[xila::task::test(task_path = xila::task)]
async fn script_mode_continues_after_command_failure_and_returns_last_status() {
    let mut shell = shell().await;
    write_file(&shell, "/script", b"false\necho continued\nfalse").await;

    assert_eq!(
        shell
            .main(vec!["-s".into(), "/script".into()])
            .await
            .unwrap_err()
            .get(),
        1
    );
    assert_eq!(contents(&shell, "/shell-output").await, "continued\n");
}

#[xila::task::test(task_path = xila::task)]
async fn script_mode_reports_line_errors_stops_and_closes_the_file() {
    let mut shell = shell().await;
    write_file(
        &shell,
        "/script",
        b"echo first\necho 'unterminated\necho wrong\n",
    )
    .await;

    assert_eq!(
        shell
            .main(vec!["-s".into(), "/script".into()])
            .await
            .unwrap_err()
            .get(),
        2
    );
    assert_eq!(contents(&shell, "/shell-output").await, "first\n");
    assert!(
        contents(&shell, "/shell-errors")
            .await
            .contains("/script:2:")
    );
    assert_eq!(shell.execute_line("mv /script /renamed").await.unwrap(), 0);
}

#[xila::task::test(task_path = xila::task)]
async fn script_mode_reports_missing_and_oversized_files_with_nonzero_status() {
    let mut shell = shell().await;
    assert_eq!(
        shell
            .main(vec!["-s".into(), "/missing-script".into()])
            .await
            .unwrap_err()
            .get(),
        1
    );
    assert!(
        contents(&shell, "/shell-errors")
            .await
            .contains("/missing-script")
    );

    let mut maximum_line = vec![b'#'; crate::tokenizer::MAX_INPUT_BYTES];
    maximum_line.extend_from_slice(b"\r\n");
    write_file(&shell, "/script", &maximum_line).await;
    assert_eq!(
        shell.main(vec!["-s".into(), "/script".into()]).await,
        Ok(())
    );

    write_file(
        &shell,
        "/script",
        &vec![b'a'; crate::tokenizer::MAX_INPUT_BYTES + 1],
    )
    .await;
    assert_eq!(
        shell
            .main(vec!["-s".into(), "/script".into()])
            .await
            .unwrap_err()
            .get(),
        2
    );
    assert!(
        contents(&shell, "/shell-errors")
            .await
            .contains("/script:1:")
    );

    let mut unreadable = File::open(
        shell.context.virtual_file_system,
        shell.task,
        "/unreadable-script",
        Flags::new(AccessFlags::Write, Some(CreateFlags::CREATE_TRUNCATE), None),
    )
    .await
    .unwrap();
    write_all(&mut unreadable, b"echo hidden\n").await.unwrap();
    unreadable
        .set_permissions(xila::file_system::Permissions::from_octal(0).unwrap())
        .await
        .unwrap();
    drop(unreadable);
    let administrator = shell
        .context
        .users_manager
        .get_user_identifier("administrator")
        .await
        .unwrap();
    shell
        .context
        .task_manager
        .set_user(shell.task, administrator)
        .await
        .unwrap();
    assert_eq!(
        shell
            .main(vec!["-s".into(), "/unreadable-script".into()])
            .await
            .unwrap_err()
            .get(),
        1
    );
    assert!(
        contents(&shell, "/shell-errors")
            .await
            .contains("/unreadable-script")
    );
}

#[xila::task::test(task_path = xila::task)]
async fn quoting_expansion_and_short_circuit_keep_shell_state() {
    let mut shell = shell().await;
    assert_eq!(shell.execute_line("export NAME='hello world'; echo \"$NAME\" '$NAME' \\$NAME; false && echo wrong || echo fallback; true || echo wrong; echo $?").await.unwrap(), 0);
    assert_eq!(
        contents(&shell, "/shell-output").await,
        "hello world $NAME $NAME\nfallback\n0\n"
    );
    assert_eq!(
        shell
            .execute_line("cd /temporary && pwd; cd / | cat; pwd")
            .await
            .unwrap(),
        0
    );
    assert!(
        contents(&shell, "/shell-output")
            .await
            .ends_with("/temporary\n/temporary\n")
    );
    assert_eq!(shell.execute_line("false").await.unwrap(), 1);
    assert_eq!(shell.execute_line("missing-command").await.unwrap(), 127);
}

#[xila::task::test(task_path = xila::task)]
async fn redirection_appends_truncates_and_reads_without_extra_newlines() {
    let mut shell = shell().await;
    assert_eq!(
        shell
            .execute_line("echo first>/one; echo second>>/one; cat</one>/copy")
            .await
            .unwrap(),
        0
    );
    assert_eq!(contents(&shell, "/copy").await, "first\nsecond\n");
    assert_eq!(
        shell.execute_line("echo x>/one; cat /one").await.unwrap(),
        0
    );
    assert_eq!(contents(&shell, "/one").await, "x\n");
    assert_eq!(contents(&shell, "/shell-output").await, "x\n");
    assert_eq!(
        shell
            .execute_line("cat /missing 2>/errors || echo recovered")
            .await
            .unwrap(),
        0
    );
    assert!(!contents(&shell, "/errors").await.is_empty());
    assert!(
        contents(&shell, "/shell-output")
            .await
            .ends_with("recovered\n")
    );
}

#[xila::task::test(task_path = xila::task)]
async fn grep_streams_matching_lines_and_reports_status_and_line_errors() {
    let mut shell = shell().await;
    File::write_to_path(
        shell.context.virtual_file_system,
        shell.task,
        "/grep-input",
        b"miss\nhit one\nno match\nhit eof",
    )
    .await
    .unwrap();

    assert_eq!(
        shell
            .execute_line("grep hit /grep-input > /grep-output")
            .await
            .unwrap(),
        0
    );
    assert_eq!(contents(&shell, "/grep-output").await, "hit one\nhit eof");
    assert_eq!(
        shell.execute_line("grep absent /grep-input").await.unwrap(),
        1
    );

    assert_eq!(
        shell
            .execute_line("echo 'pipeline hit'|grep hit|cat>/grep-pipe")
            .await
            .unwrap(),
        0
    );
    assert_eq!(contents(&shell, "/grep-pipe").await, "pipeline hit\n");
    assert_eq!(
        shell
            .execute_line("echo 'dash hit'|grep hit -|cat>/grep-dash")
            .await
            .unwrap(),
        0
    );
    assert_eq!(contents(&shell, "/grep-dash").await, "dash hit\n");

    let long_line = vec![b'x'; 257];
    File::write_to_path(
        shell.context.virtual_file_system,
        shell.task,
        "/grep-long-line",
        &long_line,
    )
    .await
    .unwrap();
    assert_eq!(
        shell.execute_line("grep x /grep-long-line").await.unwrap(),
        1
    );
    assert!(
        contents(&shell, "/shell-errors")
            .await
            .contains(xila::internationalization::t!("errors.line_too_long"))
    );
}

struct ReadOne;
impl ExecutableTrait for ReadOne {
    fn main(mut standard: Standard<'static>, _: Vec<String>) -> MainFuture {
        alloc::boxed::Box::pin(async move {
            standard.input().read(&mut [0; 1]).await.unwrap();
            Ok(())
        })
    }
}
struct Fail;
impl ExecutableTrait for Fail {
    fn main(_: Standard<'static>, _: Vec<String>) -> MainFuture {
        alloc::boxed::Box::pin(async { Err(NonZeroUsize::new(7).unwrap()) })
    }
}

#[xila::task::test(task_path = xila::task)]
async fn bounded_pipelines_stream_large_output_and_close_early_consumers() {
    let mut shell = shell().await;
    let context = shell.context;
    let task = shell.task;
    let virtual_file_system = context.virtual_file_system;
    mount_executables!(
        context,
        virtual_file_system,
        task,
        &[
            (&"/applications/executables/read-one", ReadOne),
            (&"/applications/executables/fail", Fail)
        ]
    )
    .await
    .unwrap();
    let payload = "x".repeat(3000);
    assert_eq!(
        shell
            .execute_line(&format!("echo '{payload}'|cat|wc -c>/count"))
            .await
            .unwrap(),
        0
    );
    assert_eq!(contents(&shell, "/count").await.trim(), "3001");
    assert_eq!(
        shell
            .execute_line(&format!(
                "echo '{payload}'|/applications/executables/read-one"
            ))
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        shell
            .execute_line("/applications/executables/fail && echo wrong || echo correct")
            .await
            .unwrap(),
        0
    );
    assert!(
        contents(&shell, "/shell-output")
            .await
            .ends_with("correct\n")
    );
    assert_eq!(
        shell
            .execute_line("true|/applications/executables/fail")
            .await
            .unwrap(),
        7
    );
    assert_eq!(
        shell
            .execute_line("/applications/executables/fail|true")
            .await
            .unwrap(),
        0
    );
}

#[xila::task::test(task_path = xila::task)]
async fn malformed_source_does_not_execute_a_valid_prefix() {
    let mut shell = shell().await;
    assert!(
        shell
            .execute_line("echo changed>/must-not-exist; echo 'unfinished")
            .await
            .is_err()
    );
    assert!(
        File::open(
            shell.context.virtual_file_system,
            shell.task,
            "/must-not-exist",
            AccessFlags::Read.into()
        )
        .await
        .is_err()
    );
    assert_eq!(shell.execute_line("exit; echo wrong").await.unwrap(), 0);
    assert_eq!(contents(&shell, "/shell-output").await, "");
}

struct Delayed;
impl ExecutableTrait for Delayed {
    fn main(mut standard: Standard<'static>, _: Vec<String>) -> MainFuture {
        alloc::boxed::Box::pin(async move {
            assert_eq!(standard.input().read(&mut [0; 1]).await.unwrap(), 0);
            xila::task::sleep(core::time::Duration::from_millis(20)).await;
            write_all(standard.out(), b"background\n").await.unwrap();
            Ok(())
        })
    }
}
#[xila::task::test(task_path = xila::task)]
async fn background_jobs_release_the_prompt_and_have_independent_input() {
    let mut shell = shell().await;
    let context = shell.context;
    let task = shell.task;
    let virtual_file_system = context.virtual_file_system;
    mount_executables!(
        context,
        virtual_file_system,
        task,
        &[(&"/applications/executables/delayed", Delayed)]
    )
    .await
    .unwrap();
    assert_eq!(
        shell
            .execute_line(
                "/applications/executables/delayed >/background & echo foreground>/foreground"
            )
            .await
            .unwrap(),
        0
    );
    assert!(!shell.background_jobs[0].is_finished());
    assert_eq!(contents(&shell, "/foreground").await, "foreground\n");
    for job in shell.background_jobs.drain(..) {
        assert_eq!(job.join().await, 0);
    }
    assert_eq!(contents(&shell, "/background").await, "background\n");
}

#[xila::task::test(task_path = xila::task)]
async fn literal_arguments_are_not_reparsed_and_command_mode_returns_failure() {
    let mut shell = shell().await;
    assert_eq!(
        shell
            .run_arguments(&["echo".into(), "$literal; not syntax".into()])
            .await,
        0
    );
    shell.standard.out().flush().await.unwrap();
    assert_eq!(
        contents(&shell, "/shell-output").await,
        "$literal; not syntax\n"
    );
    assert_eq!(
        shell
            .main(vec!["-c".into(), "false".into()])
            .await
            .unwrap_err()
            .get(),
        1
    );
}

#[xila::task::test(task_path = xila::task)]
async fn redirection_checks_existing_file_permissions_before_truncating() {
    let mut shell = shell().await;
    assert_eq!(
        shell
            .execute_line("echo original>/protected")
            .await
            .unwrap(),
        0
    );
    // Use the namespace API: read-only LittleFS opens cannot commit metadata.
    // Persist the fixture permissions instead of only changing an open cache.
    shell
        .context
        .virtual_file_system
        .set_permissions(
            shell.task,
            "/protected",
            xila::file_system::Permissions::from_octal(0o444).unwrap(),
        )
        .await
        .unwrap();
    let administrator = shell
        .context
        .users_manager
        .get_user_identifier("administrator")
        .await
        .unwrap();
    shell
        .context
        .task_manager
        .set_user(shell.task, administrator)
        .await
        .unwrap();
    assert_eq!(
        shell.execute_line("echo changed>/protected").await.unwrap(),
        1
    );
    assert_eq!(contents(&shell, "/protected").await, "original\n");
    assert_eq!(
        shell
            .execute_line("echo changed>/forbidden-new-file")
            .await
            .unwrap(),
        1
    );
    assert!(
        File::open(
            shell.context.virtual_file_system,
            shell.task,
            "/forbidden-new-file",
            AccessFlags::Read.into()
        )
        .await
        .is_err()
    );
}

#[xila::task::test(task_path = xila::task)]
async fn duplicated_pipe_writers_must_all_close_before_eof() {
    let shell = shell().await;
    let (reader, writer) = File::create_unnamed_pipe(
        shell.context.virtual_file_system,
        8,
        StateFlags::from_bits_truncate(0),
    )
    .await
    .unwrap();
    let mut reader = reader;
    let duplicate = writer.duplicate().await.unwrap();
    writer
        .close(shell.context.virtual_file_system)
        .await
        .unwrap();
    duplicate
        .close(shell.context.virtual_file_system)
        .await
        .unwrap();
    assert_eq!(reader.read(&mut [0; 1]).await.unwrap(), 0);
    reader
        .close(shell.context.virtual_file_system)
        .await
        .unwrap();
}

#[xila::task::test(task_path = xila::task)]
async fn duplicated_regular_streams_share_their_position() {
    let mut shell = shell().await;
    let mut duplicate = shell.standard.out().duplicate().await.unwrap();
    write_all(shell.standard.out(), b"a").await.unwrap();
    write_all(&mut duplicate, b"b").await.unwrap();
    write_all(shell.standard.out(), b"c").await.unwrap();
    duplicate.flush().await.unwrap();
    drop(duplicate);
    assert_eq!(contents(&shell, "/shell-output").await, "abc");
}

struct FailingFlush;
impl xila::file_system::DirectBaseOperations for FailingFlush {
    fn read(
        &self,
        _cx: &mut core::task::Context<'_>,
        _: &mut [u8],
        _: xila::file_system::Size,
    ) -> core::task::Poll<xila::file_system::Result<usize>> {
        xila::file_system::ready(|| Ok(0))
    }
    fn write(
        &self,
        _cx: &mut core::task::Context<'_>,
        bytes: &[u8],
        _: xila::file_system::Size,
    ) -> core::task::Poll<xila::file_system::Result<usize>> {
        xila::file_system::ready(|| Ok(bytes.len()))
    }
    fn flush(
        &self,
        _cx: &mut core::task::Context<'_>,
    ) -> core::task::Poll<xila::file_system::Result<()>> {
        xila::file_system::ready(|| Err(xila::file_system::IoErrorKind::InputOutput))
    }
}
impl xila::file_system::MountOperations for FailingFlush {}
impl xila::file_system::CharacterDevice for FailingFlush {}

struct FailingRead;
impl xila::file_system::DirectBaseOperations for FailingRead {
    fn read(
        &self,
        _cx: &mut core::task::Context<'_>,
        _: &mut [u8],
        _: xila::file_system::Size,
    ) -> core::task::Poll<xila::file_system::Result<usize>> {
        xila::file_system::ready(|| Err(xila::file_system::IoErrorKind::InputOutput))
    }
    fn write(
        &self,
        _cx: &mut core::task::Context<'_>,
        bytes: &[u8],
        _: xila::file_system::Size,
    ) -> core::task::Poll<xila::file_system::Result<usize>> {
        xila::file_system::ready(|| Ok(bytes.len()))
    }
    fn flush(
        &self,
        _cx: &mut core::task::Context<'_>,
    ) -> core::task::Poll<xila::file_system::Result<()>> {
        xila::file_system::ready(|| Ok(()))
    }
}
impl xila::file_system::MountOperations for FailingRead {
    fn unmount(&self) -> xila::file_system::Result<()> {
        Ok(())
    }
}
impl xila::file_system::CharacterDevice for FailingRead {}

#[xila::task::test(task_path = xila::task)]
async fn script_mode_reports_read_errors_and_closes_the_script_file() {
    let mut shell = shell().await;
    shell
        .context
        .virtual_file_system
        .mount_character_device(shell.task, "/devices/failing-read", FailingRead)
        .await
        .unwrap();

    assert_eq!(
        shell
            .main(vec!["-s".into(), "/devices/failing-read".into()])
            .await
            .unwrap_err()
            .get(),
        2
    );
    assert!(
        contents(&shell, "/shell-errors")
            .await
            .contains("/devices/failing-read:1:")
    );
    shell
        .context
        .virtual_file_system
        .unmount(&"/devices/failing-read", false)
        .await
        .unwrap();
}

#[xila::task::test(task_path = xila::task)]
async fn script_mode_rejects_shell_nesting_beyond_the_limit() {
    let mut shell = shell().await;
    shell
        .context
        .task_manager
        .set_environment_variable(
            shell.task,
            crate::SCRIPT_DEPTH_VARIABLE,
            &crate::MAX_SHELL_DEPTH.to_string(),
        )
        .await
        .unwrap();

    assert_eq!(
        shell
            .main(vec!["-s".into(), "/script".into()])
            .await
            .unwrap_err()
            .get(),
        2
    );
    assert!(
        contents(&shell, "/shell-errors")
            .await
            .contains("/script: Maximum nested shell depth exceeded")
    );
}

#[xila::task::test(task_path = xila::task)]
async fn script_mode_exit_stops_execution_and_closes_the_file() {
    let mut shell = shell().await;
    write_file(&shell, "/script", b"exit\necho wrong\n").await;

    assert_eq!(
        shell.main(vec!["-s".into(), "/script".into()]).await,
        Ok(())
    );
    assert_eq!(contents(&shell, "/shell-output").await, "");
    shell
        .context
        .virtual_file_system
        .rename(shell.task, &"/script", &"/renamed-script")
        .await
        .unwrap();
}

#[xila::task::test(task_path = xila::task)]
async fn pipeline_flush_failures_control_conditional_execution() {
    let mut shell = shell().await;
    shell
        .context
        .virtual_file_system
        .mount_character_device(shell.task, "/devices/failing-flush", FailingFlush)
        .await
        .unwrap();
    assert_eq!(
        shell
            .execute_line("echo value|cat>/devices/failing-flush && echo wrong")
            .await
            .unwrap(),
        1
    );
    assert_eq!(contents(&shell, "/shell-output").await, "");
}

#[xila::task::test(task_path = xila::task)]
async fn expansion_is_bounded_and_does_not_reinterpret_variable_contents() {
    let mut shell = shell().await;
    shell
        .context
        .task_manager
        .set_environment_variable(shell.task, "VALUE", "one two; echo wrong")
        .await
        .unwrap();
    assert_eq!(
        shell
            .execute_line("echo ${VALUE} '$VALUE' \"$MISSING\"")
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        contents(&shell, "/shell-output").await,
        "one two; echo wrong $VALUE \n"
    );
    shell
        .context
        .task_manager
        .set_environment_variable(shell.task, "VALUE", &"x".repeat(MAX_EXPANDED_BYTES))
        .await
        .unwrap();
    assert_eq!(shell.execute_line("echo $VALUE").await.unwrap(), 1);
}

#[xila::task::test(task_path = xila::task)]
async fn duplicated_device_keeps_its_mount_busy() {
    let shell = shell().await;
    let filesystem = shell.context.virtual_file_system;
    filesystem
        .mount_character_device(shell.task, "/devices/duplicate", drivers_core::NullDevice)
        .await
        .unwrap();
    let original = File::open(
        filesystem,
        shell.task,
        "/devices/duplicate",
        AccessFlags::Read.into(),
    )
    .await
    .unwrap();
    let duplicate = original.duplicate().await.unwrap();
    drop(original);
    assert_eq!(
        filesystem.unmount(&"/devices/duplicate", false).await,
        Err(xila::virtual_file_system::IoErrorKind::ResourceBusy)
    );
    drop(duplicate);
}

#[xila::task::test(task_path = xila::task)]
async fn interactive_eof_executes_the_final_line_once() {
    let mut shell = shell().await;
    shell
        .context
        .task_manager
        .set_environment_variable(shell.task, "User", "root")
        .await
        .unwrap();
    // Create input directly so EOF, rather than a trailing newline, ends the command.
    let mut input = File::open(
        shell.context.virtual_file_system,
        shell.task,
        "/shell-input",
        Flags::new(
            AccessFlags::READ_WRITE,
            Some(CreateFlags::CREATE_TRUNCATE),
            None,
        ),
    )
    .await
    .unwrap();
    write_all(&mut input, b"echo final").await.unwrap();
    input
        .set_position(&xila::file_system::Position::Start(0))
        .await
        .unwrap();
    shell.standard.standard_in = input;
    shell.main(Vec::new()).await.unwrap();
    let output = contents(&shell, "/shell-output").await;
    assert_eq!(output.matches("final\n").count(), 1);
}

#[xila::task::test(task_path = xila::task)]
async fn utility_commands_compose_with_streaming_and_redirection() {
    let mut shell = shell().await;
    assert_eq!(
        shell
            .execute_line("sleep 0 && help sleep | cat > /sleep-help")
            .await
            .unwrap(),
        0
    );
    assert!(
        contents(&shell, "/sleep-help")
            .await
            .contains("Usage: sleep")
    );
    assert_eq!(shell.execute_line("mkdir /utility-tree; echo saved > /utility-tree/file; chmod 600 /utility-tree/file; tree -L 1 /utility-tree | cat > /tree-output").await.unwrap(), 0);
    assert_eq!(
        contents(&shell, "/tree-output").await,
        "/utility-tree\n|-- file\n"
    );
    assert_eq!(shell.execute_line("mkdir /utility-tree/sub; echo found > /utility-tree/sub/target.txt; find /utility-tree --name target.txt --type f > /find-output").await.unwrap(), 0);
    assert_eq!(
        contents(&shell, "/find-output").await,
        "/utility-tree/sub/target.txt\n"
    );
    assert_eq!(
        shell
            .context
            .virtual_file_system
            .get_statistics(&"/utility-tree/file")
            .await
            .unwrap()
            .permissions
            .as_u16(),
        0o600
    );
    assert_eq!(
        shell
            .execute_line("false 2> /false-errors || true")
            .await
            .unwrap(),
        0
    );
    assert_eq!(contents(&shell, "/false-errors").await, "");
}

#[xila::task::test(task_path = xila::task)]
async fn realpath_resolves_relative_and_multiple_existing_operands() {
    let mut shell = shell().await;
    assert_eq!(
        shell
            .execute_line("mkdir /realpath; echo value > /realpath/file; cd /realpath; realpath . file /realpath/../realpath/file")
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        contents(&shell, "/shell-output").await,
        "/realpath\n/realpath/file\n/realpath/file\n"
    );
    assert_eq!(shell.execute_line("help realpath").await.unwrap(), 0);
    assert!(
        contents(&shell, "/shell-output")
            .await
            .contains("Usage: realpath PATH...")
    );
}

#[xila::task::test(task_path = xila::task)]
async fn realpath_rejects_missing_operands() {
    let mut shell = shell().await;
    assert_eq!(shell.execute_line("realpath /missing").await.unwrap(), 1);
    assert_eq!(contents(&shell, "/shell-output").await, "");
}

#[xila::task::test(task_path = xila::task)]
async fn mv_renames_a_file_to_a_new_path() {
    let mut shell = shell().await;
    assert_eq!(
        shell
            .execute_line("echo moved > /mv-source; mv /mv-source /mv-destination")
            .await
            .unwrap(),
        0
    );
    assert_eq!(contents(&shell, "/mv-destination").await, "moved\n");
    assert!(
        shell
            .context
            .virtual_file_system
            .get_statistics(&"/mv-source")
            .await
            .is_err()
    );
}

#[xila::task::test(task_path = xila::task)]
async fn mv_rejects_invalid_operands_and_reports_rename_errors() {
    let mut shell = shell().await;
    assert_eq!(shell.execute_line("mv").await.unwrap(), 1);
    assert_eq!(shell.execute_line("mv source").await.unwrap(), 1);
    assert_eq!(
        shell
            .execute_line("mv source destination extra")
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        shell
            .execute_line("mv -f source destination")
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        shell
            .execute_line("echo source > /mv-error-source; echo destination > /mv-error-destination; mv /missing /new")
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        shell
            .execute_line("mv /mv-error-source /mv-error-destination")
            .await
            .unwrap(),
        0
    );
    assert_eq!(contents(&shell, "/mv-error-destination").await, "source\n");
    assert!(
        shell
            .context
            .virtual_file_system
            .get_statistics(&"/mv-error-source")
            .await
            .is_err()
    );
}

#[xila::task::test(task_path = xila::task)]
async fn mv_checks_write_permission_on_both_parent_directories() {
    use xila::users::{GroupIdentifier, UserIdentifier};

    let mut shell = shell().await;
    assert_eq!(
        shell
            .execute_line("mkdir /mv-protected; echo protected > /mv-protected/source")
            .await
            .unwrap(),
        0
    );

    let owner = UserIdentifier::new(265);
    let other = UserIdentifier::new(266);
    for (user, name) in [(owner, "mv_owner"), (other, "mv_other")] {
        shell
            .context
            .users_manager
            .add_user(
                user,
                name,
                GroupIdentifier::ROOT,
                &alloc::format!("/users/{name}"),
            )
            .await
            .unwrap();
    }

    let vfs = shell.context.virtual_file_system;
    vfs.set_ownership(shell.task, "/mv-protected", Some(owner), None)
        .await
        .unwrap();
    vfs.set_permissions(
        shell.task,
        "/mv-protected",
        xila::file_system::Permissions::from_octal(0o700).unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(
        shell
            .execute_line("mkdir /mv-public; echo public > /mv-public/source")
            .await
            .unwrap(),
        0
    );
    vfs.set_ownership(shell.task, "/mv-public", Some(other), None)
        .await
        .unwrap();
    vfs.set_permissions(
        shell.task,
        "/mv-public",
        xila::file_system::Permissions::from_octal(0o700).unwrap(),
    )
    .await
    .unwrap();

    shell
        .context
        .task_manager
        .set_user(shell.task, other)
        .await
        .unwrap();
    assert_eq!(
        shell
            .execute_line("mv /mv-protected/source /mv-protected/denied")
            .await
            .unwrap(),
        1
    );
    // The rejected operation must leave the source in place. Check it as its
    // owner because the current user cannot traverse this directory.
    shell
        .context
        .task_manager
        .set_user(shell.task, owner)
        .await
        .unwrap();
    assert!(vfs.get_statistics(&"/mv-protected/source").await.is_ok());
    shell
        .context
        .task_manager
        .set_user(shell.task, other)
        .await
        .unwrap();
    assert_eq!(
        shell
            .execute_line("mv /mv-public/source /mv-protected/denied")
            .await
            .unwrap(),
        1
    );
    assert!(vfs.get_statistics(&"/mv-public/source").await.is_ok());

    shell
        .context
        .task_manager
        .set_user(shell.task, owner)
        .await
        .unwrap();
    assert_eq!(
        shell
            .execute_line("mv /mv-protected/source /mv-protected/allowed")
            .await
            .unwrap(),
        0
    );
    assert!(vfs.get_statistics(&"/mv-protected/source").await.is_err());
}

#[xila::task::test(task_path = xila::task)]
async fn rename_checks_parent_paths_relative_to_the_mounted_filesystem() {
    let shell = shell().await;
    let device = xila::file_system::MemoryDevice::<512>::new_static(1024 * 512);
    little_fs::FileSystem::format(device, little_fs::Options::new(256)).unwrap();
    let file_system = little_fs::FileSystem::new(device, little_fs::Options::new(256)).unwrap();
    xila::file_system::FileSystemOperations::set_attributes(
        &file_system,
        xila::file_system::Path::ROOT,
        &xila::file_system::Attributes::new()
            .set_user(xila::users::UserIdentifier::ROOT)
            .set_group(xila::users::GroupIdentifier::ROOT)
            .set_permissions(xila::file_system::Permissions::ALL_FULL),
    )
    .unwrap();
    let source_path = xila::file_system::Path::from_str("/source");
    xila::file_system::FileSystemOperations::create_file(&file_system, source_path).unwrap();
    xila::file_system::FileSystemOperations::set_attributes(
        &file_system,
        source_path,
        &xila::file_system::Attributes::new()
            .set_user(xila::users::UserIdentifier::ROOT)
            .set_group(xila::users::GroupIdentifier::ROOT)
            .set_permissions(xila::file_system::Permissions::FILE_DEFAULT)
            .set_kind(xila::file_system::Kind::File),
    )
    .unwrap();

    let vfs = shell.context.virtual_file_system;
    vfs.mount_file_system(file_system, "/mv-nested-mount", shell.task)
        .await
        .unwrap();

    vfs.rename(
        shell.task,
        &"/mv-nested-mount/source",
        &"/mv-nested-mount/destination",
    )
    .await
    .unwrap();
}

/// Scripted standard input that keeps terminal attributes and records, for
/// every byte read, whether echo was on at that moment.
///
/// Getting the attributes fails with `get_error` when it is set, and each
/// attempt to set them fails with the next entry of `set_errors` (`None`
/// or no entry left: the attempt succeeds).
struct LoginTerminalState {
    input: std::sync::Mutex<(Vec<u8>, Vec<bool>)>,
    attributes: std::sync::Mutex<xila::file_system::character_device::TerminalAttributes>,
    get_error: Option<xila::file_system::IoErrorKind>,
    set_errors: std::sync::Mutex<Vec<Option<xila::file_system::IoErrorKind>>>,
    set_attempts: std::sync::Mutex<usize>,
    /// While set, reads stay pending.
    blocked: std::sync::Mutex<bool>,
    /// Fail one read after this many bytes have been read.
    read_error_after: std::sync::Mutex<Option<usize>>,
    /// When set, setting attributes reports success but changes nothing.
    ignores_changes: bool,
}

impl LoginTerminalState {
    fn new(input: &[u8]) -> &'static Self {
        Self::with_failures(input, None, &[])
    }

    fn with_failures(
        input: &[u8],
        get_error: Option<xila::file_system::IoErrorKind>,
        set_errors: &[Option<xila::file_system::IoErrorKind>],
    ) -> &'static Self {
        Self::build(input, get_error, set_errors, false)
    }

    /// A terminal that accepts attribute changes without applying them.
    fn ignoring_changes(input: &[u8]) -> &'static Self {
        Self::build(input, None, &[], true)
    }

    fn build(
        input: &[u8],
        get_error: Option<xila::file_system::IoErrorKind>,
        set_errors: &[Option<xila::file_system::IoErrorKind>],
        ignores_changes: bool,
    ) -> &'static Self {
        Box::leak(Box::new(Self {
            input: std::sync::Mutex::new((input.iter().rev().copied().collect(), Vec::new())),
            attributes: std::sync::Mutex::new(
                xila::file_system::character_device::TerminalAttributes::INTERACTIVE,
            ),
            get_error,
            set_errors: std::sync::Mutex::new(set_errors.iter().rev().copied().collect()),
            set_attempts: std::sync::Mutex::new(0),
            blocked: std::sync::Mutex::new(false),
            read_error_after: std::sync::Mutex::new(None),
            ignores_changes,
        }))
    }

    fn set_blocked(&self, blocked: bool) {
        *self.blocked.lock().unwrap() = blocked;
    }

    /// Input not read yet.
    fn unread(&self) -> Vec<u8> {
        self.input.lock().unwrap().0.iter().rev().copied().collect()
    }

    fn set_attempts(&self) -> usize {
        *self.set_attempts.lock().unwrap()
    }

    fn echo_per_byte(&self) -> Vec<bool> {
        self.input.lock().unwrap().1.clone()
    }

    fn attributes(&self) -> xila::file_system::character_device::TerminalAttributes {
        *self.attributes.lock().unwrap()
    }
}

struct LoginTerminal(&'static LoginTerminalState);

impl xila::file_system::DirectBaseOperations for LoginTerminal {
    fn read(
        &self,
        _: &mut core::task::Context<'_>,
        buffer: &mut [u8],
        _: xila::file_system::Size,
    ) -> core::task::Poll<xila::file_system::Result<usize>> {
        if *self.0.blocked.lock().unwrap() {
            return core::task::Poll::Pending;
        }
        let echo = self.0.attributes().is_echo_enabled();
        let (remaining, echo_per_byte) = &mut *self.0.input.lock().unwrap();
        let mut read_error_after = self.0.read_error_after.lock().unwrap();
        if *read_error_after == Some(echo_per_byte.len()) {
            *read_error_after = None;
            return core::task::Poll::Ready(Err(xila::file_system::IoErrorKind::InputOutput));
        }
        let Some(byte) = remaining.pop() else {
            return core::task::Poll::Ready(Ok(0));
        };
        buffer[0] = byte;
        echo_per_byte.push(echo);
        core::task::Poll::Ready(Ok(1))
    }

    fn write(
        &self,
        _: &mut core::task::Context<'_>,
        buffer: &[u8],
        _: xila::file_system::Size,
    ) -> core::task::Poll<xila::file_system::Result<usize>> {
        core::task::Poll::Ready(Ok(buffer.len()))
    }

    fn control(
        &self,
        _: &mut core::task::Context<'_>,
        command: xila::file_system::ControlCommandIdentifier,
        input: &xila::shared::AnyByLayout,
        output: &mut xila::shared::AnyByLayout,
    ) -> core::task::Poll<xila::file_system::Result<()>> {
        use xila::file_system::{
            ControlCommand,
            character_device::{GET_TERMINAL_ATTRIBUTES, SET_TERMINAL_ATTRIBUTES},
        };
        xila::file_system::ready(|| match command {
            GET_TERMINAL_ATTRIBUTES::IDENTIFIER => {
                if let Some(error) = self.0.get_error {
                    return Err(error);
                }
                *GET_TERMINAL_ATTRIBUTES::cast_output(output)? = self.0.attributes();
                Ok(())
            }
            SET_TERMINAL_ATTRIBUTES::IDENTIFIER => {
                *self.0.set_attempts.lock().unwrap() += 1;
                if let Some(error) = self.0.set_errors.lock().unwrap().pop().flatten() {
                    return Err(error);
                }
                if self.0.ignores_changes {
                    return Ok(());
                }
                *self.0.attributes.lock().unwrap() = *SET_TERMINAL_ATTRIBUTES::cast_input(input)?;
                Ok(())
            }
            _ => Err(xila::file_system::IoErrorKind::Unsupported),
        })
    }
}

impl xila::file_system::MountOperations for LoginTerminal {}
impl xila::file_system::DirectCharacterDevice for LoginTerminal {}

async fn shell_with_login_terminal(
    path: &str,
    input: &[u8],
) -> (Shell, &'static LoginTerminalState) {
    shell_with_scripted_terminal(path, LoginTerminalState::new(input)).await
}

async fn shell_with_scripted_terminal(
    path: &str,
    state: &'static LoginTerminalState,
) -> (Shell, &'static LoginTerminalState) {
    let mut shell = shell().await;
    let file_system = shell.context.virtual_file_system;
    file_system
        .mount_character_device(shell.task, path, LoginTerminal(state))
        .await
        .unwrap();
    // Like `Standard::open`, standard input is opened read-only.
    shell.standard.standard_in =
        File::open(file_system, shell.task, path, AccessFlags::Read.into())
            .await
            .unwrap();
    (shell, state)
}

#[xila::task::test(task_path = xila::task)]
async fn login_hides_the_password_and_restores_echo() {
    let (mut shell, state) =
        shell_with_login_terminal("/devices/login_terminal", b"administrator\n\n").await;

    assert_eq!(
        shell.authenticate().await.unwrap().as_deref(),
        Some("administrator")
    );

    // The user name and its line feed are read with echo on, the (empty)
    // password line with echo off.
    let mut expected = vec![true; "administrator\n".len()];
    expected.push(false);
    assert_eq!(state.echo_per_byte(), expected);
    assert_eq!(
        state.attributes(),
        xila::file_system::character_device::TerminalAttributes::INTERACTIVE
    );
}

#[xila::task::test(task_path = xila::task)]
async fn login_restores_echo_when_reading_the_password_fails() {
    let mut input = b"administrator\n".to_vec();
    input.extend(core::iter::repeat_n(b'x', 300));
    let (mut shell, state) = shell_with_login_terminal("/devices/long_password", &input).await;

    assert!(matches!(
        shell.authenticate().await,
        Err(crate::Error::LineTooLong)
    ));
    assert!(state.echo_per_byte().ends_with(&[false]));
    assert_eq!(
        state.attributes(),
        xila::file_system::character_device::TerminalAttributes::INTERACTIVE
    );
}

/// Standard error written so far, flushed first so that a new handle reads it.
async fn errors_written(shell: &mut Shell) -> String {
    shell.standard.error().flush().await.unwrap();
    contents(shell, "/shell-errors").await
}

#[xila::task::test(task_path = xila::task)]
async fn login_without_terminal_attributes_reads_the_password_as_before() {
    let mut shell = shell().await;
    let mut input = File::open(
        shell.context.virtual_file_system,
        shell.task,
        "/login-input",
        Flags::new(
            AccessFlags::READ_WRITE,
            Some(CreateFlags::CREATE_TRUNCATE),
            None,
        ),
    )
    .await
    .unwrap();
    write_all(&mut input, b"administrator\n\n").await.unwrap();
    input
        .set_position(&xila::file_system::Position::Start(0))
        .await
        .unwrap();
    shell.standard.standard_in = input;

    assert_eq!(
        shell.authenticate().await.unwrap().as_deref(),
        Some("administrator")
    );
    // No diagnostic is written when the input has no terminal attributes.
    assert_eq!(errors_written(&mut shell).await, "");
}

const LOGIN_INPUT: &[u8] = b"administrator\n\n";

/// Echo state of every byte of `LOGIN_INPUT` read with echo on throughout.
fn echo_on_for_the_whole_login() -> Vec<bool> {
    vec![true; LOGIN_INPUT.len()]
}

#[xila::task::test(task_path = xila::task)]
async fn login_reads_the_password_with_echo_when_getting_attributes_is_unsupported() {
    let state = LoginTerminalState::with_failures(
        LOGIN_INPUT,
        Some(xila::file_system::IoErrorKind::Unsupported),
        &[],
    );
    let (mut shell, state) =
        shell_with_scripted_terminal("/devices/no_get_attributes", state).await;

    assert_eq!(
        shell.authenticate().await.unwrap().as_deref(),
        Some("administrator")
    );
    assert_eq!(state.echo_per_byte(), echo_on_for_the_whole_login());
    assert_eq!(state.set_attempts(), 0);
    assert_eq!(errors_written(&mut shell).await, "");
}

#[xila::task::test(task_path = xila::task)]
async fn login_reads_the_password_with_echo_when_setting_attributes_is_unsupported() {
    let state = LoginTerminalState::with_failures(
        LOGIN_INPUT,
        None,
        &[Some(xila::file_system::IoErrorKind::Unsupported)],
    );
    let (mut shell, state) =
        shell_with_scripted_terminal("/devices/no_set_attributes", state).await;

    assert_eq!(
        shell.authenticate().await.unwrap().as_deref(),
        Some("administrator")
    );
    assert_eq!(state.echo_per_byte(), echo_on_for_the_whole_login());
    // Nothing was changed, so nothing is restored.
    assert_eq!(state.set_attempts(), 1);
    assert_eq!(errors_written(&mut shell).await, "");
}

#[xila::task::test(task_path = xila::task)]
async fn login_fails_without_reading_the_password_when_getting_attributes_fails() {
    let state = LoginTerminalState::with_failures(
        LOGIN_INPUT,
        Some(xila::file_system::IoErrorKind::WouldBlock),
        &[],
    );
    let (mut shell, state) =
        shell_with_scripted_terminal("/devices/busy_get_attributes", state).await;

    let error = shell.authenticate().await.unwrap_err();
    assert!(matches!(
        error,
        crate::Error::FailedToHidePassword(xila::file_system::IoErrorKind::WouldBlock)
    ));
    assert!(!error.to_string().is_empty());
    // Only the user name line was read: the password stays unread.
    assert_eq!(state.echo_per_byte(), vec![true; "administrator\n".len()]);
    assert_eq!(state.set_attempts(), 0);
}

#[xila::task::test(task_path = xila::task)]
async fn login_fails_without_reading_the_password_when_turning_echo_off_fails() {
    let state = LoginTerminalState::with_failures(
        LOGIN_INPUT,
        None,
        &[Some(xila::file_system::IoErrorKind::InputOutput)],
    );
    let (mut shell, state) =
        shell_with_scripted_terminal("/devices/failing_set_attributes", state).await;

    assert!(matches!(
        shell.authenticate().await,
        Err(crate::Error::FailedToHidePassword(
            xila::file_system::IoErrorKind::InputOutput
        ))
    ));
    assert_eq!(state.echo_per_byte(), vec![true; "administrator\n".len()]);
    assert_eq!(state.set_attempts(), 1);
    assert_eq!(
        state.attributes(),
        xila::file_system::character_device::TerminalAttributes::INTERACTIVE
    );
}

#[xila::task::test(task_path = xila::task)]
async fn login_ends_the_session_when_the_password_cannot_be_hidden() {
    let state = LoginTerminalState::with_failures(
        b"administrator\nsecret\n",
        Some(xila::file_system::IoErrorKind::WouldBlock),
        &[],
    );
    let (mut shell, state) =
        shell_with_scripted_terminal("/devices/busy_login_session", state).await;

    let error = crate::Error::FailedToHidePassword(xila::file_system::IoErrorKind::WouldBlock);
    let expected_diagnostic = format!("{error}\n");
    let expected_status = NonZeroUsize::from(error);
    assert_eq!(shell.main(vec![]).await, Err(expected_status));

    // The shell neither prompts again nor reads the password typed ahead:
    // only the user name line was read, with echo on.
    assert_eq!(state.echo_per_byte(), vec![true; "administrator\n".len()]);
    assert_eq!(errors_written(&mut shell).await, expected_diagnostic);
    shell.standard.out().flush().await.unwrap();
    let output = contents(&shell, "/shell-output").await;
    assert!(!output.contains("secret"));
    let user_name_prompt = format!(
        "{}",
        xila::internationalization::t!("authentication.prompts.username")
    );
    assert_eq!(output.matches(user_name_prompt.as_str()).count(), 1);
}

#[xila::task::test(task_path = xila::task)]
async fn login_retries_a_failed_restore_once() {
    let state = LoginTerminalState::with_failures(
        LOGIN_INPUT,
        None,
        &[None, Some(xila::file_system::IoErrorKind::WouldBlock)],
    );
    let (mut shell, state) = shell_with_scripted_terminal("/devices/restore_retry", state).await;

    assert_eq!(
        shell.authenticate().await.unwrap().as_deref(),
        Some("administrator")
    );
    // Echo off, a failed restore, then a successful one.
    assert_eq!(state.set_attempts(), 3);
    assert_eq!(
        state.attributes(),
        xila::file_system::character_device::TerminalAttributes::INTERACTIVE
    );
    assert_eq!(errors_written(&mut shell).await, "");
}

#[xila::task::test(task_path = xila::task)]
async fn login_reports_a_restore_that_fails_twice() {
    let state = LoginTerminalState::with_failures(
        LOGIN_INPUT,
        None,
        &[
            None,
            Some(xila::file_system::IoErrorKind::WouldBlock),
            Some(xila::file_system::IoErrorKind::WouldBlock),
        ],
    );
    let (mut shell, state) = shell_with_scripted_terminal("/devices/restore_failure", state).await;

    // The password was read, so the login itself goes on.
    assert_eq!(
        shell.authenticate().await.unwrap().as_deref(),
        Some("administrator")
    );
    assert_eq!(state.set_attempts(), 3);
    assert!(!state.attributes().is_echo_enabled());
    let expected = format!(
        "{}\n",
        crate::Error::FailedToRestoreTerminalEcho(xila::file_system::IoErrorKind::WouldBlock)
    );
    assert_eq!(errors_written(&mut shell).await, expected);
}

/// Identity and environment of one run of [`Identify`].
#[derive(Debug)]
struct IdentifyRun {
    marker: String,
    user: xila::users::UserIdentifier,
    group: xila::users::GroupIdentifier,
    environment: Vec<(String, String)>,
}

impl IdentifyRun {
    fn variable(&self, name: &str) -> Option<&str> {
        self.environment
            .iter()
            .find(|(variable, _)| variable == name)
            .map(|(_, value)| value.as_str())
    }
}

/// Every run of [`Identify`], shared by the tests of a process: each test
/// looks up its own runs by the marker it passes as the first argument.
static IDENTIFY_RUNS: std::sync::Mutex<Vec<IdentifyRun>> = std::sync::Mutex::new(Vec::new());

fn identify_runs(marker: &str) -> Vec<IdentifyRun> {
    let mut runs = IDENTIFY_RUNS.lock().unwrap();
    let (matching, others) = core::mem::take(&mut *runs)
        .into_iter()
        .partition(|run| run.marker == marker);
    *runs = others;
    matching
}

/// Records the identity and environment it runs with.
struct Identify;
impl ExecutableTrait for Identify {
    fn main(standard: Standard<'static>, arguments: Vec<String>) -> MainFuture {
        Box::pin(async move {
            let manager = standard.context.task_manager;
            let task = manager.get_current_task_identifier().await;
            let run = IdentifyRun {
                marker: arguments.first().cloned().unwrap_or_default(),
                user: manager.get_user(task).await.unwrap(),
                group: manager.get_group(task).await.unwrap(),
                environment: manager
                    .get_environment_variables(task)
                    .await
                    .unwrap()
                    .iter()
                    .map(|variable| {
                        (
                            variable.get_name().to_string(),
                            variable.get_value().to_string(),
                        )
                    })
                    .collect(),
            };
            IDENTIFY_RUNS.lock().unwrap().push(run);
            Ok(())
        })
    }
}

const SUDO_PASSWORD: &str = "secret";

/// A Shell whose standard input is a login terminal sending `input`; see
/// [`sudo_shell_with`].
async fn sudo_shell(
    name: &str,
    administrator: bool,
    input: &[u8],
) -> (
    Shell,
    &'static LoginTerminalState,
    xila::users::UserIdentifier,
    xila::users::GroupIdentifier,
) {
    sudo_shell_with(name, administrator, LoginTerminalState::new(input)).await
}

/// A Shell whose standard input is the login terminal `state`, running as
/// the new account `name` (password [`SUDO_PASSWORD`]). The account is an
/// administrator when `administrator` is set, otherwise the only member of a
/// group of its own.
///
/// As in seeded images, account records are readable by root only.
/// `identify` is installed in the secure path and `elsewhere` (the same
/// program) in `/sudo_paths`, which is also the Shell's working directory and
/// in its `Paths`. The caller's environment also holds a forged `Host` and a
/// `Sudo_test_secret`, which commands run by `sudo` must not see.
async fn sudo_shell_with(
    name: &str,
    administrator: bool,
    state: &'static LoginTerminalState,
) -> (
    Shell,
    &'static LoginTerminalState,
    xila::users::UserIdentifier,
    xila::users::GroupIdentifier,
) {
    use xila::{file_system::Permissions, users::ADMINISTRATOR_GROUP_NAME};

    let (mut shell, state) = shell_with_scripted_terminal(&format!("/devices/{name}"), state).await;
    let context = shell.context;
    let task = shell.task;
    let virtual_file_system = context.virtual_file_system;
    let authentication_context = xila::authentication::Context {
        virtual_file_system,
        task_manager: context.task_manager,
        users_manager: context.users_manager,
        task,
    };
    let group = if administrator {
        context
            .users_manager
            .get_group_identifier(ADMINISTRATOR_GROUP_NAME)
            .await
            .unwrap()
    } else {
        xila::authentication::create_group(&authentication_context, &format!("{name}_group"), None)
            .await
            .unwrap()
    };
    let user = xila::authentication::create_user(
        &authentication_context,
        name,
        SUDO_PASSWORD,
        group,
        None,
    )
    .await
    .unwrap();

    xila::virtual_file_system::Directory::create(virtual_file_system, task, "/sudo_paths")
        .await
        .unwrap();
    mount_executables!(
        context,
        virtual_file_system,
        task,
        &[
            (&"/applications/executables/identify", Identify),
            (&"/sudo_paths/elsewhere", Identify)
        ]
    )
    .await
    .unwrap();
    for (path, permissions) in [
        ("/", 0o755),
        ("/applications", 0o755),
        ("/applications/executables", 0o755),
        ("/sudo_paths", 0o755),
        ("/system/accounts", 0o700),
    ] {
        virtual_file_system
            .set_permissions(task, path, Permissions::from_octal(permissions).unwrap())
            .await
            .unwrap();
    }

    let manager = context.task_manager;
    for (variable, value) in [
        ("Paths", "/sudo_paths:/applications/executables"),
        ("Host", "forged"),
        ("Sudo_test_secret", "inherited"),
    ] {
        manager
            .set_environment_variable(task, variable, value)
            .await
            .unwrap();
    }
    shell.current_directory = Path::from_str("/sudo_paths").to_owned();
    manager.set_user(task, user).await.unwrap();
    manager.set_group(task, group).await.unwrap();
    (shell, state, user, group)
}

/// The Shell task's identity.
async fn shell_identity(
    shell: &Shell,
) -> (xila::users::UserIdentifier, xila::users::GroupIdentifier) {
    let manager = shell.context.task_manager;
    (
        manager.get_user(shell.task).await.unwrap(),
        manager.get_group(shell.task).await.unwrap(),
    )
}

/// Switch the Shell task to root, to set up files or read results back.
async fn become_root(shell: &Shell) {
    let manager = shell.context.task_manager;
    manager
        .set_user(shell.task, xila::users::UserIdentifier::ROOT)
        .await
        .unwrap();
    manager
        .set_group(shell.task, xila::users::GroupIdentifier::ROOT)
        .await
        .unwrap();
}

/// Standard error written so far, read back as root.
async fn sudo_errors(shell: &mut Shell) -> String {
    become_root(shell).await;
    errors_written(shell).await
}

/// Standard output written so far, read back as root.
async fn sudo_output(shell: &mut Shell) -> String {
    become_root(shell).await;
    shell.standard.out().flush().await.unwrap();
    contents(shell, "/shell-output").await
}

#[xila::task::test(task_path = xila::task)]
async fn sudo_runs_the_command_as_root_without_echo_and_keeps_the_shell_identity() {
    let (mut shell, state, user, group) = sudo_shell("sudo_success", true, b"secret\n").await;

    assert_eq!(
        shell
            .execute_line("sudo identify sudo_success")
            .await
            .unwrap(),
        0
    );

    let runs = identify_runs("sudo_success");
    assert_eq!(runs.len(), 1);
    let run = &runs[0];
    assert_eq!(run.user, xila::users::UserIdentifier::ROOT);
    assert_eq!(run.group, xila::users::GroupIdentifier::ROOT);
    // The password is read with echo off, and echo is restored afterwards.
    assert_eq!(state.echo_per_byte(), vec![false; b"secret\n".len()]);
    assert_eq!(
        state.attributes(),
        xila::file_system::character_device::TerminalAttributes::INTERACTIVE
    );
    // The Shell's task keeps the caller's identity.
    assert_eq!(shell_identity(&shell).await, (user, group));
    // Only the fixed, minimal environment reaches the command: nothing of
    // the caller's but the marker naming the caller.
    let mut names: Vec<&str> = run
        .environment
        .iter()
        .map(|(name, _)| name.as_str())
        .collect();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "Current_directory",
            "HOME",
            "Host",
            "Paths",
            "Sudo_user",
            "User"
        ]
    );
    assert_eq!(run.variable("User"), Some("root"));
    assert_eq!(run.variable("HOME"), Some(xila::users::ROOT_HOME));
    assert_eq!(run.variable("Host"), Some(crate::sudo::SUDO_HOST));
    // Root's home does not exist here, so the command starts in `/`, not in
    // the caller's directory.
    assert_eq!(run.variable("Current_directory"), Some("/"));
    assert_eq!(run.variable("Paths"), Some(crate::sudo::SECURE_PATH));
    assert_eq!(
        run.variable(crate::SUDO_USER_VARIABLE),
        Some("sudo_success")
    );
    assert_eq!(sudo_errors(&mut shell).await, "");
}

#[xila::task::test(task_path = xila::task)]
async fn sudo_runs_the_command_as_another_user_in_its_home() {
    let (mut shell, _, user, group) = sudo_shell("sudo_other", true, b"secret\n").await;
    let manager = shell.context.task_manager;
    // Accounts are created as root.
    become_root(&shell).await;
    let authentication_context = xila::authentication::Context {
        virtual_file_system: shell.context.virtual_file_system,
        task_manager: manager,
        users_manager: shell.context.users_manager,
        task: shell.task,
    };
    let target_group =
        xila::authentication::create_group(&authentication_context, "sudo_target_group", None)
            .await
            .unwrap();
    let target = xila::authentication::create_user(
        &authentication_context,
        "sudo_target",
        "other",
        target_group,
        None,
    )
    .await
    .unwrap();
    shell
        .context
        .virtual_file_system
        .ensure_user_home(shell.task, target)
        .await
        .unwrap();
    manager.set_user(shell.task, user).await.unwrap();
    manager.set_group(shell.task, group).await.unwrap();

    assert_eq!(
        shell
            .execute_line("sudo -u sudo_target identify sudo_other")
            .await
            .unwrap(),
        0
    );
    let runs = identify_runs("sudo_other");
    assert_eq!(runs.len(), 1);
    assert_eq!((runs[0].user, runs[0].group), (target, target_group));
    assert_eq!(runs[0].variable("User"), Some("sudo_target"));
    assert_eq!(runs[0].variable("HOME"), Some("/users/sudo_target"));
    assert_eq!(
        runs[0].variable("Current_directory"),
        Some("/users/sudo_target")
    );
    assert_eq!(shell_identity(&shell).await, (user, group));
}

#[xila::task::test(task_path = xila::task)]
async fn sudo_refuses_a_wrong_password_and_keeps_the_shell_identity() {
    let (mut shell, state, user, group) = sudo_shell("sudo_wrong", true, b"wrong\n").await;

    assert_eq!(
        shell
            .execute_line("sudo identify sudo_wrong")
            .await
            .unwrap(),
        1
    );

    assert!(identify_runs("sudo_wrong").is_empty());
    assert_eq!(state.echo_per_byte(), vec![false; b"wrong\n".len()]);
    assert_eq!(shell_identity(&shell).await, (user, group));
    let expected = format!(
        "{}\n",
        crate::Error::AuthenticationFailed(xila::authentication::Error::InvalidPassword)
    );
    assert_eq!(sudo_errors(&mut shell).await, expected);
}

#[xila::task::test(task_path = xila::task)]
async fn sudo_refuses_a_caller_outside_the_administrator_group_without_prompting() {
    let (mut shell, state, user, group) = sudo_shell("sudo_outsider", false, b"secret\n").await;

    assert_eq!(
        shell
            .execute_line("sudo identify sudo_outsider")
            .await
            .unwrap(),
        1
    );

    assert!(identify_runs("sudo_outsider").is_empty());
    // Nothing was read: the caller is refused before the prompt.
    assert!(state.echo_per_byte().is_empty());
    assert_eq!(shell_identity(&shell).await, (user, group));
    assert_eq!(
        sudo_errors(&mut shell).await,
        format!("{}\n", crate::Error::SudoNotAllowed)
    );
    assert_eq!(sudo_output(&mut shell).await, "");
}

#[xila::task::test(task_path = xila::task)]
async fn sudo_aborts_when_input_ends_during_the_prompt() {
    let (mut shell, state, user, group) = sudo_shell("sudo_end", true, b"").await;

    assert_eq!(
        shell.execute_line("sudo identify sudo_end").await.unwrap(),
        1
    );

    assert!(identify_runs("sudo_end").is_empty());
    assert!(state.echo_per_byte().is_empty());
    assert_eq!(
        state.attributes(),
        xila::file_system::character_device::TerminalAttributes::INTERACTIVE
    );
    assert_eq!(shell_identity(&shell).await, (user, group));
    assert_eq!(
        sudo_errors(&mut shell).await,
        format!("{}\n", crate::Error::SudoAborted)
    );
}

#[xila::task::test(task_path = xila::task)]
async fn sudo_locks_the_caller_out_after_three_failures() {
    let (mut shell, state, _, _) =
        sudo_shell("sudo_lockout", true, b"one\ntwo\nthree\nsecret\n").await;

    for _ in 0..3 {
        assert_eq!(
            shell
                .execute_line("sudo identify sudo_lockout")
                .await
                .unwrap(),
            1
        );
    }
    let read = state.echo_per_byte().len();
    assert_eq!(read, b"one\ntwo\nthree\n".len());

    // The right password is not even read while the caller is locked out.
    assert_eq!(
        shell
            .execute_line("sudo identify sudo_lockout")
            .await
            .unwrap(),
        1
    );
    assert_eq!(state.echo_per_byte().len(), read);
    assert!(identify_runs("sudo_lockout").is_empty());
    let errors = sudo_errors(&mut shell).await;
    let last = errors.lines().last().unwrap();
    assert_eq!(
        format!("{last}\n"),
        format!("{}\n", crate::Error::SudoLockedOut(30))
    );
}

#[xila::task::test(task_path = xila::task)]
async fn sudo_admits_one_attempt_per_user_across_shells() {
    use embassy_futures::join::join;

    let (mut first, first_state, _, _) = sudo_shell("sudo_concurrent", true, b"secret\n").await;
    // A second Shell of the same user, on a terminal of its own.
    let second_state = LoginTerminalState::new(b"secret\n");
    let virtual_file_system = first.context.virtual_file_system;
    become_root(&first).await;
    virtual_file_system
        .mount_character_device(
            first.task,
            "/devices/sudo_concurrent_second",
            LoginTerminal(second_state),
        )
        .await
        .unwrap();
    let mut standard = first.standard.duplicate().await.unwrap();
    standard.standard_in = File::open(
        virtual_file_system,
        first.task,
        "/devices/sudo_concurrent_second",
        AccessFlags::Read.into(),
    )
    .await
    .unwrap();
    let mut second = Shell::new(standard).await;
    let user = first
        .context
        .users_manager
        .get_user_identifier("sudo_concurrent")
        .await
        .unwrap();
    first
        .context
        .task_manager
        .set_user(first.task, user)
        .await
        .unwrap();

    // The first attempt waits at its prompt while the second one starts.
    first_state.set_blocked(true);
    let (first_status, second_status) =
        join(first.execute_line("sudo identify sudo_concurrent"), async {
            let status = second
                .execute_line("sudo identify sudo_concurrent")
                .await
                .unwrap();
            first_state.set_blocked(false);
            status
        })
        .await;

    assert_eq!(first_status.unwrap(), 0);
    assert_eq!(second_status, 1);
    // The second Shell was refused without reading its input.
    assert!(second_state.echo_per_byte().is_empty());
    assert_eq!(identify_runs("sudo_concurrent").len(), 1);
    let errors = sudo_errors(&mut second).await;
    assert!(errors.contains(&format!("{}", crate::Error::SudoBusy)));
}

#[xila::task::test(task_path = xila::task)]
async fn sudo_refuses_built_ins_and_commands_outside_the_secure_path() {
    let (mut shell, state, _, _) = sudo_shell("sudo_paths", true, b"secret\n").await;

    assert_eq!(shell.execute_line("sudo cd /").await.unwrap(), 1);
    // `elsewhere` is only in the caller's `Paths`, not in the secure path.
    assert_eq!(
        shell
            .execute_line("sudo elsewhere sudo_paths")
            .await
            .unwrap(),
        127
    );
    // Relative to the caller's working directory, outside the secure path,
    // or leaving it with `..`.
    for command in [
        "sudo ./elsewhere sudo_paths",
        "sudo /sudo_paths/elsewhere sudo_paths",
        "sudo /applications/executables/../../sudo_paths/elsewhere sudo_paths",
    ] {
        assert_eq!(shell.execute_line(command).await.unwrap(), 1, "{command}");
    }
    assert_eq!(shell.execute_line("sudo").await.unwrap(), 1);
    // None of them prompted.
    assert!(state.echo_per_byte().is_empty());
    assert!(identify_runs("sudo_paths").is_empty());
    assert_eq!(
        sudo_errors(&mut shell).await,
        format!(
            "{}\n{}\n{}\n{}\n{}\n{}\n",
            crate::Error::SudoBuiltIn("cd".into()),
            crate::Error::CommandNotFound,
            crate::Error::SudoCommandNotAllowed("./elsewhere".into()),
            crate::Error::SudoCommandNotAllowed("/sudo_paths/elsewhere".into()),
            crate::Error::SudoCommandNotAllowed(
                "/applications/executables/../../sudo_paths/elsewhere".into()
            ),
            crate::Error::SudoUsage
        )
    );
}

#[xila::task::test(task_path = xila::task)]
async fn sudo_accepts_an_absolute_path_in_the_secure_path() {
    let (mut shell, _, _, _) = sudo_shell("sudo_explicit", true, b"secret\n").await;

    assert_eq!(
        shell
            .execute_line("sudo /applications/executables/identify sudo_explicit")
            .await
            .unwrap(),
        0
    );
    let runs = identify_runs("sudo_explicit");
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].user, xila::users::UserIdentifier::ROOT);
}

/// Run `sudo identify NAME` on the terminal `state` and check that it is
/// refused before the prompt because echo cannot be turned off.
async fn assert_sudo_refuses_without_hidden_echo(
    name: &str,
    state: &'static LoginTerminalState,
    expected: xila::file_system::IoErrorKind,
) {
    let (mut shell, state, _, _) = sudo_shell_with(name, true, state).await;

    assert_eq!(
        shell
            .execute_line(&format!("sudo identify {name}"))
            .await
            .unwrap(),
        1
    );

    assert!(identify_runs(name).is_empty());
    // Nothing was read and no prompt was written.
    assert!(state.echo_per_byte().is_empty());
    assert_eq!(sudo_output(&mut shell).await, "");
    assert_eq!(
        state.attributes(),
        xila::file_system::character_device::TerminalAttributes::INTERACTIVE
    );
    assert_eq!(
        sudo_errors(&mut shell).await,
        format!("{}\n", crate::Error::SudoEchoNotSuppressed(expected))
    );
}

#[xila::task::test(task_path = xila::task)]
async fn sudo_refuses_input_without_terminal_attributes() {
    use xila::file_system::IoErrorKind;
    assert_sudo_refuses_without_hidden_echo(
        "sudo_no_attributes",
        LoginTerminalState::with_failures(b"secret\n", Some(IoErrorKind::Unsupported), &[]),
        IoErrorKind::Unsupported,
    )
    .await;
}

#[xila::task::test(task_path = xila::task)]
async fn sudo_refuses_input_whose_echo_cannot_be_turned_off() {
    use xila::file_system::IoErrorKind;
    assert_sudo_refuses_without_hidden_echo(
        "sudo_no_set_attributes",
        LoginTerminalState::with_failures(b"secret\n", None, &[Some(IoErrorKind::Unsupported)]),
        IoErrorKind::Unsupported,
    )
    .await;
    assert_sudo_refuses_without_hidden_echo(
        "sudo_failing_set_attributes",
        LoginTerminalState::with_failures(b"secret\n", None, &[Some(IoErrorKind::InputOutput)]),
        IoErrorKind::InputOutput,
    )
    .await;
}

#[xila::task::test(task_path = xila::task)]
async fn sudo_refuses_input_whose_echo_stays_on() {
    assert_sudo_refuses_without_hidden_echo(
        "sudo_echo_stays_on",
        LoginTerminalState::ignoring_changes(b"secret\n"),
        xila::file_system::IoErrorKind::Unsupported,
    )
    .await;
}

#[xila::task::test(task_path = xila::task)]
async fn sudo_refuses_a_file_as_password_input() {
    let (mut shell, _, _, _) = sudo_shell("sudo_file_input", true, b"").await;
    become_root(&shell).await;
    write_file(&shell, "/sudo-password", b"secret\n").await;
    shell.standard.standard_in = File::open(
        shell.context.virtual_file_system,
        shell.task,
        "/sudo-password",
        AccessFlags::Read.into(),
    )
    .await
    .unwrap();
    let user = shell
        .context
        .users_manager
        .get_user_identifier("sudo_file_input")
        .await
        .unwrap();
    shell
        .context
        .task_manager
        .set_user(shell.task, user)
        .await
        .unwrap();

    assert_eq!(
        shell
            .execute_line("sudo identify sudo_file_input")
            .await
            .unwrap(),
        1
    );
    assert!(identify_runs("sudo_file_input").is_empty());
    let errors = sudo_errors(&mut shell).await;
    assert!(
        errors.starts_with(&format!(
            "{}",
            crate::Error::SudoEchoNotSuppressed(xila::file_system::IoErrorKind::Unsupported)
        )) || errors.starts_with(&format!(
            "{}",
            crate::Error::SudoEchoNotSuppressed(
                xila::file_system::IoErrorKind::InappropriateIoControl
            )
        )),
        "{errors}"
    );
}

#[xila::task::test(task_path = xila::task)]
async fn sudo_reads_an_overlong_password_to_the_end_of_its_line_and_refuses_it() {
    let mut input = vec![b'x'; crate::sudo::MAXIMUM_PASSWORD_BYTES + 50];
    input.extend_from_slice(b"\necho next\n");
    let (mut shell, state, _, _) = sudo_shell("sudo_long", true, &input).await;

    assert_eq!(
        shell.execute_line("sudo identify sudo_long").await.unwrap(),
        1
    );

    assert!(identify_runs("sudo_long").is_empty());
    // The whole line was read with echo off, and nothing after it.
    let line = crate::sudo::MAXIMUM_PASSWORD_BYTES + 50 + 1;
    assert_eq!(state.echo_per_byte(), vec![false; line]);
    assert_eq!(state.unread(), b"echo next\n");
    assert_eq!(
        state.attributes(),
        xila::file_system::character_device::TerminalAttributes::INTERACTIVE
    );
    assert_eq!(
        sudo_errors(&mut shell).await,
        format!("{}\n", crate::Error::SudoPasswordTooLong)
    );
}

#[xila::task::test(task_path = xila::task)]
async fn sudo_stops_the_shell_when_hidden_input_cannot_be_drained() {
    let mut input = b"sudo identify sudo_read_error\n".to_vec();
    input.extend(core::iter::repeat_n(b'x', 257));
    let error_after = input.len();
    input.extend_from_slice(b"unread password bytes\n");
    let (mut shell, state, _, _) = sudo_shell("sudo_read_error", true, &input).await;
    *state.read_error_after.lock().unwrap() = Some(error_after);

    shell.main_interactive().await.unwrap();

    assert_eq!(state.unread(), b"unread password bytes\n");
    assert_eq!(
        state.attributes(),
        xila::file_system::character_device::TerminalAttributes::INTERACTIVE
    );
}

#[xila::task::test(task_path = xila::task)]
async fn sudo_accepts_a_password_of_the_maximum_length() {
    let password = "p".repeat(crate::sudo::MAXIMUM_PASSWORD_BYTES);
    let mut input = password.clone().into_bytes();
    input.push(b'\n');
    let (mut shell, _, user, group) = sudo_shell("sudo_maximum", true, &input).await;
    become_root(&shell).await;
    xila::authentication::change_user_password(
        &xila::authentication::Context {
            virtual_file_system: shell.context.virtual_file_system,
            task_manager: shell.context.task_manager,
            users_manager: shell.context.users_manager,
            task: shell.task,
        },
        "sudo_maximum",
        &password,
    )
    .await
    .unwrap();
    let manager = shell.context.task_manager;
    manager.set_user(shell.task, user).await.unwrap();
    manager.set_group(shell.task, group).await.unwrap();

    assert_eq!(
        shell
            .execute_line("sudo identify sudo_maximum")
            .await
            .unwrap(),
        0
    );
    assert_eq!(identify_runs("sudo_maximum").len(), 1);
}

/// Command failures belong on the command's standard error, not in the log, so
/// they can be redirected and are seen whatever the console log level is.
#[xila::task::test(task_path = xila::task)]
async fn command_failures_are_written_to_standard_error() {
    let mut shell = shell().await;

    for line in ["ls /missing-directory", "web_request a b"] {
        assert_ne!(shell.execute_line(line).await.unwrap(), 0, "{line}");
    }

    assert_eq!(contents(&shell, "/shell-output").await, "");
    let errors = contents(&shell, "/shell-errors").await;
    assert_eq!(errors.lines().count(), 2, "{errors}");
}

#[xila::task::test(task_path = xila::task)]
async fn redirected_standard_error_receives_command_failures() {
    let mut shell = shell().await;

    assert_ne!(
        shell
            .execute_line("web_request --nonexistent 2> /temporary/errors")
            .await
            .unwrap(),
        0
    );

    assert_eq!(contents(&shell, "/shell-errors").await, "");
    assert!(!contents(&shell, "/temporary/errors").await.is_empty());
}

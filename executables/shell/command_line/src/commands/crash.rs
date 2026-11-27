use alloc::format;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use getargs_derive::{GetArgs, ValueEnum};
use xila::{
    crash::{self, BuildIdentifier, Header, Record, ReportIdentifier},
    file_system::Path,
    virtual_file_system::IoErrorKind,
};

use super::{CommandContext, UserCommand};
use crate::{Error, Result};

pub struct CrashCommand;

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(super) enum Operation {
    List,
    Show,
    Export,
    Clear,
}

#[derive(GetArgs)]
#[command(
    name = "crash",
    about = "List crash reports, show one, export one, or clear reports as an administrator. Show and export select the latest report unless you give a report name."
)]
pub(super) struct CrashArguments<'a> {
    #[arg(value_enum, default_value_t = Operation::List)]
    operation: Operation,
    /// Report name or latest.
    report: Option<&'a str>,
    /// Export binary bytes. The default export is Base64.
    #[arg(long)]
    raw: bool,
}

impl UserCommand for CrashCommand {
    async fn execute<'a, I, C>(
        &self,
        context: &mut C,
        options: &mut getargs::Options<&'a str, I>,
        _paths: &[&Path],
    ) -> Result<(), Error<'a>>
    where
        I: Iterator<Item = &'a str>,
        C: CommandContext,
    {
        let arguments = CrashArguments::parse(options)?;
        if (arguments.raw && arguments.operation != Operation::Export)
            || (arguments.report.is_some()
                && matches!(arguments.operation, Operation::List | Operation::Clear))
        {
            return Err(Error::InvalidArgument);
        }

        let filesystem = context.executable_context().virtual_file_system;
        let task = context.task_id();
        match arguments.operation {
            Operation::List => {
                let reports = crash::list(filesystem, task)
                    .await
                    .map_err(Error::CrashReports)?;
                for identifier in reports.into_iter().rev() {
                    let bytes = match crash::read(filesystem, task, identifier).await {
                        Ok(bytes) => bytes,
                        Err(error) => {
                            warn_skipped(context, identifier, error).await;
                            continue;
                        }
                    };
                    let record = match Record::read(&bytes) {
                        Ok(record) if record.bytes().len() == bytes.len() => record,
                        _ => {
                            warn_skipped(context, identifier, IoErrorKind::CorruptData).await;
                            continue;
                        }
                    };
                    let header = record.header;
                    let message = record.message;
                    let end = message.floor_char_boundary(80);
                    context
                        .write_out_fmt(format_args!(
                            "{identifier} {:?} {:?} task={} uptime={} us {}{}\n",
                            header.kind,
                            header.reset_reason,
                            header.task_name.escape_debug(),
                            header.uptime_microseconds,
                            message[..end].escape_debug(),
                            if end < message.len() { "..." } else { "" },
                        ))
                        .await?;
                }
                Ok(())
            }
            Operation::Show | Operation::Export => {
                let identifier = match arguments.report {
                    None | Some("latest") => crash::list(filesystem, task)
                        .await
                        .map_err(Error::CrashReports)?
                        .last()
                        .copied()
                        .ok_or(Error::CrashReports(IoErrorKind::NotFound))?,
                    Some(name) => {
                        ReportIdentifier::from_name(name).ok_or(Error::InvalidArgument)?
                    }
                };
                let bytes = crash::read(filesystem, task, identifier)
                    .await
                    .map_err(Error::CrashReports)?;
                match arguments.operation {
                    Operation::Show => show(context, identifier, decode(&bytes)?).await,
                    _ if arguments.raw => context.write_out(&bytes).await,
                    _ => export(context, &bytes).await,
                }
            }
            Operation::Clear => crash::clear(filesystem, task)
                .await
                .map_err(Error::CrashReports),
        }
    }
}

fn decode(bytes: &[u8]) -> Result<Record<'_>> {
    Record::read(bytes).map_err(|_| Error::CrashReports(IoErrorKind::CorruptData))
}

/// Report an unreadable entry on standard error and let the listing continue.
async fn warn_skipped<C: CommandContext>(
    context: &mut C,
    identifier: ReportIdentifier,
    reason: IoErrorKind,
) {
    let _ = crate::execution::write_all(
        context.standard().error(),
        format!("crash: warning: skipping unreadable report {identifier}: {reason}\n").as_bytes(),
    )
    .await;
}

async fn show<C: CommandContext>(
    context: &mut C,
    identifier: ReportIdentifier,
    record: Record<'_>,
) -> Result<()> {
    let Header {
        kind,
        architecture,
        reset_reason,
        raw_reset_reason,
        exception_code,
        consecutive_crashes,
        uptime_microseconds,
        build_epoch,
        program_counter,
        stack_pointer,
        link_register,
        fault_address,
        stack_base,
        code_load_offset,
        core_identifier,
        task_identifier,
        build_identifier,
        task_name,
        target_name,
        firmware_version,
        flags,
    } = record.header;
    context
        .write_out_fmt(format_args!(
            "{identifier}\nkind={kind:?} architecture={architecture:?} target={} version={} build_epoch={build_epoch}\nreset={reset_reason:?} raw_reset={raw_reset_reason:#x} exception={exception_code:#x} consecutive_crashes={consecutive_crashes} uptime={uptime_microseconds} us\ncore={core_identifier} task={task_identifier} name={}\nprogram_counter={program_counter:#x} stack_pointer={stack_pointer:#x} link_register={link_register:#x}\nfault_address={fault_address:#x} stack_base={stack_base:#x} code_load_offset={code_load_offset:#x} flags={flags:#x}\n",
            target_name.escape_debug(),
            firmware_version.escape_debug(),
            task_name.escape_debug(),
        ))
        .await?;
    let (identifier_kind, bytes) = match &build_identifier {
        BuildIdentifier::None => ("none", &[][..]),
        BuildIdentifier::ElfSha256(bytes) => ("ELF SHA-256", bytes.as_slice()),
        BuildIdentifier::GnuBuildIdentifier(bytes) => ("GNU", bytes.as_slice()),
        BuildIdentifier::WasmFingerprint(bytes) => ("WebAssembly", bytes.as_slice()),
    };
    context
        .write_out_fmt(format_args!("build_identifier={identifier_kind}"))
        .await?;
    if !bytes.is_empty() {
        context.write_out(b" ").await?;
        for byte in bytes {
            context.write_out_fmt(format_args!("{byte:02x}")).await?;
        }
    }
    context
        .write_out_fmt(format_args!(
            "\nmessage={}\n",
            record.message.escape_debug()
        ))
        .await?;
    for (index, frame) in record.frames().enumerate() {
        context
            .write_out_fmt(format_args!("frame[{index}]={frame:#x}\n"))
            .await?;
    }
    if !record.log_tail.is_empty() {
        context.write_out(b"log_tail\n").await?;
        for line in record.log_tail.lines() {
            context
                .write_out_fmt(format_args!("{}\n", line.escape_debug()))
                .await?;
        }
    }
    Ok(())
}

async fn export<C: CommandContext>(context: &mut C, bytes: &[u8]) -> Result<()> {
    // Each complete chunk has no padding, so the lines form one Base64 stream.
    for chunk in bytes.chunks(57) {
        let mut output = [0; 76];
        let length = STANDARD
            .encode_slice(chunk, &mut output)
            .map_err(|_| Error::Format)?;
        context.write_out_line(&output[..length]).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_list_and_accepts_named_raw_export() {
        let default = CrashArguments::from_args(core::iter::empty()).unwrap();
        assert!(default.operation == Operation::List);
        let export =
            CrashArguments::from_args(["export", "4-kernel.xcr", "--raw"].into_iter()).unwrap();
        assert!(export.operation == Operation::Export);
        assert_eq!(export.report, Some("4-kernel.xcr"));
        assert!(export.raw);
    }
}

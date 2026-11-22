use crate::host::wasi;
use xila::{file_system::IoErrorKind, io_error::PosixErrno};

/// Preview 1 errno results are always a 32-bit ABI value for both guest pointer widths.
pub type WasiResult = i32;

pub enum CombinedError {
    Wasi(wasi::Error),
    Wasmi(wasmi::Error),
}

impl From<wasmi::Error> for CombinedError {
    fn from(e: wasmi::Error) -> Self {
        CombinedError::Wasmi(e)
    }
}

impl From<Error> for CombinedError {
    fn from(e: Error) -> Self {
        CombinedError::Wasi(e)
    }
}

/// The errno (or the trap) a host function reports for `result`.
pub fn status(result: CombinedResult<()>) -> Result<WasiResult, wasmi::Error> {
    match result {
        Ok(()) => Ok(0),
        Err(CombinedError::Wasi(error)) => Ok(i32::from(error)),
        Err(CombinedError::Wasmi(error)) => Err(error),
    }
}

macro_rules! wrap_function {
    ($body:expr) => {{
        // Evaluates $body directly inside a try-block pattern
        let result: crate::host::wasi::error::CombinedResult<()> = (|| $body)();

        match result {
            Ok(()) => Ok(0),
            Err(crate::host::wasi::error::CombinedError::Wasi(e)) => Ok(i32::from(e)),
            Err(crate::host::wasi::error::CombinedError::Wasmi(e)) => Err(e),
        }
    }};
}

pub(crate) use wrap_function;

pub type CombinedResult<T> = Result<T, CombinedError>;

// Generate the range test from the enum so new variants are checked automatically.
macro_rules! preview1_errors {
    ($($(#[$doc:meta])* $name:ident = $value:literal,)+) => {
        #[repr(u32)]
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum Error {
            $($(#[$doc])* $name = $value,)+
        }

        #[test]
        fn guest_errnos_are_in_the_preview1_range() {
            for error in [$(Error::$name,)+] {
                assert!((1..=76).contains(&i32::from(error)), "{error:?}");
            }
        }
    };
}

preview1_errors! {
    /// Argument list too long (`E2BIG`).
    Toobig = 1,
    /// Permission denied (`EACCES`).
    Access = 2,
    /// Address already in use (`EADDRINUSE`).
    Addrinuse = 3,
    /// Address not available (`EADDRNOTAVAIL`).
    Addrnotavail = 4,
    /// Address family not supported by protocol (`EAFNOSUPPORT`).
    Afnosupport = 5,
    /// Resource temporarily unavailable / Try again (`EAGAIN`).
    Again = 6,
    /// Operation already in progress (`EALREADY`).
    Already = 7,
    /// Bad file descriptor (`EBADF`).
    Badf = 8,
    /// Bad message (`EBADMSG`).
    Badmsg = 9,
    /// Device or resource busy (`EBUSY`).
    Busy = 10,
    /// Operation canceled (`ECANCELED`).
    Canceled = 11,
    /// No child processes (`ECHILD`).
    Child = 12,
    /// Software caused connection abort (`ECONNABORTED`).
    Connaborted = 13,
    /// Connection refused (`ECONNREFUSED`).
    Connrefused = 14,
    /// Connection reset by peer (`ECONNRESET`).
    Connreset = 15,
    /// Resource deadlock avoided (`EDEADLK`).
    Deadlk = 16,
    /// Destination address required (`EDESTADDRREQ`).
    Destaddrreq = 17,
    /// Mathematics argument out of domain of function (`EDOM`).
    Dom = 18,
    /// Disk quota exceeded (`EDQUOT`).
    Dquot = 19,
    /// File exists (`EEXIST`).
    Exist = 20,
    /// Bad address (`EFAULT`).
    Fault = 21,
    /// File too large (`EFBIG`).
    Fbig = 22,
    /// No route to host (`EHOSTUNREACH`).
    Hostunreach = 23,
    /// Identifier removed (`EIDRM`).
    Idrm = 24,
    /// Illegal byte sequence (`EILSEQ`).
    Ilseq = 25,
    /// Operation now in progress (`EINPROGRESS`).
    Inprogress = 26,
    /// Interrupted system call (`EINTR`).
    Intr = 27,
    /// Invalid argument (`EINVAL`).
    Inval = 28,
    /// I/O error (`EIO`).
    Io = 29,
    /// Transport endpoint is already connected (`EISCONN`).
    Isconn = 30,
    /// Is a directory (`EISDIR`).
    Isdir = 31,
    /// Too many levels of symbolic links (`ELOOP`).
    Loop = 32,
    /// Too many open files for this process (`EMFILE`).
    Mfile = 33,
    /// Too many links (`EMLINK`).
    Mlink = 34,
    /// Message too long (`EMSGSIZE`).
    Msgsize = 35,
    /// Multihop attempted (`EMULTIHOP`).
    Multihop = 36,
    /// File name too long (`ENAMETOOLONG`).
    Nametoolong = 37,
    /// Network is down (`ENETDOWN`).
    Netdown = 38,
    /// Network dropped connection on reset (`ENETRESET`).
    Netreset = 39,
    /// Network is unreachable (`ENETUNREACH`).
    Netunreach = 40,
    /// Too many open files in system (`ENFILE`).
    Nfile = 41,
    /// No buffer space available (`ENOBUFS`).
    Nobufs = 42,
    /// No such device (`ENODEV`).
    Nodev = 43,
    /// No such file or directory (`ENOENT`).
    Noent = 44,
    /// Exec format error (`ENOEXEC`).
    Noexec = 45,
    /// No locks available (`ENOLCK`).
    Nolck = 46,
    /// Link has been severed (`ENOLINK`).
    Nolink = 47,
    /// Out of memory (`ENOMEM`).
    Nomem = 48,
    /// No message of desired type (`ENOMSG`).
    Nomsg = 49,
    /// Protocol not available (`ENOPROTOOPT`).
    Noprotoopt = 50,
    /// No space left on device (`ENOSPC`).
    Nospc = 51,
    /// Function not implemented (`ENOSYS`).
    Nosys = 52,
    /// Transport endpoint is not connected (`ENOTCONN`).
    Notconn = 53,
    /// Not a directory (`ENOTDIR`).
    Notdir = 54,
    /// Directory not empty (`ENOTEMPTY`).
    Notempty = 55,
    /// State not recoverable (`ENOTRECOVERABLE`).
    Notrecoverable = 56,
    /// Socket operation on non-socket (`ENOTSOCK`).
    Notsock = 57,
    /// Operation not supported (`ENOTSUP`).
    Notsup = 58,
    /// Inappropriate ioctl for device (`ENOTTY`).
    Notty = 59,
    /// No such device or address (`ENXIO`).
    Nxio = 60,
    /// Value too large for defined data type (`EOVERFLOW`).
    Overflow = 61,
    /// Owner died (`EOWNERDEAD`).
    Ownerdead = 62,
    /// Operation not permitted (`EPERM`).
    Perm = 63,
    /// Broken pipe (`EPIPE`).
    Pipe = 64,
    /// Protocol error (`EPROTO`).
    Proto = 65,
    /// Protocol not supported (`EPROTONOSUPPORT`).
    Protonosupport = 66,
    /// Protocol wrong type for socket (`EPROTOTYPE`).
    Prototype = 67,
    /// Result too large or numerical result out of range (`ERANGE`).
    Range = 68,
    /// Read-only file system (`EROFS`).
    Rofs = 69,
    /// Invalid seek (`ESPIPE`).
    Spipe = 70,
    /// No such process (`ESRCH`).
    Srch = 71,
    /// Stale file handle (`ESTALE`).
    Stale = 72,
    /// Connection timed out (`ETIMEDOUT`).
    Timedout = 73,
    /// Text file busy (`ETXTBSY`).
    Txtbsy = 74,
    /// Cross-device link (`EXDEV`).
    Xdev = 75,
    /// Capabilities insufficient (`ENOTCAPABLE`).
    Notcapable = 76,
}

impl From<Error> for i32 {
    fn from(err: Error) -> Self {
        err as i32
    }
}

/// The Preview 1 errno for a symbolic POSIX errno.
///
/// Preview 1 has no `EOPNOTSUPP` (it is `ENOTSUP`) and no `ENODATA`; a missing
/// extended attribute or inode data is reported as `ENOENT`, the closest
/// Preview 1 result for an absent item. A projection Core adds later falls back
/// to `EIO` until it is mapped here (the exhaustiveness test below catches it).
pub fn errno_error(errno: PosixErrno) -> Error {
    match errno {
        PosixErrno::E2BIG => Error::Toobig,
        PosixErrno::EACCES => Error::Access,
        PosixErrno::EADDRINUSE => Error::Addrinuse,
        PosixErrno::EADDRNOTAVAIL => Error::Addrnotavail,
        PosixErrno::EAFNOSUPPORT => Error::Afnosupport,
        PosixErrno::EAGAIN => Error::Again,
        PosixErrno::EALREADY => Error::Already,
        PosixErrno::EBADF => Error::Badf,
        PosixErrno::EBUSY => Error::Busy,
        PosixErrno::ECHILD => Error::Child,
        PosixErrno::ECONNABORTED => Error::Connaborted,
        PosixErrno::ECONNREFUSED => Error::Connrefused,
        PosixErrno::ECONNRESET => Error::Connreset,
        PosixErrno::EDEADLK => Error::Deadlk,
        PosixErrno::EDESTADDRREQ => Error::Destaddrreq,
        PosixErrno::EEXIST => Error::Exist,
        PosixErrno::EFBIG => Error::Fbig,
        PosixErrno::EHOSTUNREACH => Error::Hostunreach,
        PosixErrno::EINPROGRESS => Error::Inprogress,
        PosixErrno::EINTR => Error::Intr,
        PosixErrno::EINVAL => Error::Inval,
        PosixErrno::EIO => Error::Io,
        PosixErrno::EISCONN => Error::Isconn,
        PosixErrno::EISDIR => Error::Isdir,
        PosixErrno::ELOOP => Error::Loop,
        PosixErrno::EMFILE => Error::Mfile,
        PosixErrno::EMSGSIZE => Error::Msgsize,
        PosixErrno::ENAMETOOLONG => Error::Nametoolong,
        PosixErrno::ENETDOWN => Error::Netdown,
        PosixErrno::ENETUNREACH => Error::Netunreach,
        PosixErrno::ENFILE => Error::Nfile,
        PosixErrno::ENOBUFS => Error::Nobufs,
        PosixErrno::ENODATA | PosixErrno::ENOENT => Error::Noent,
        PosixErrno::ENODEV => Error::Nodev,
        PosixErrno::ENOEXEC => Error::Noexec,
        PosixErrno::ENOMEM => Error::Nomem,
        PosixErrno::ENOSPC => Error::Nospc,
        PosixErrno::ENOSYS => Error::Nosys,
        PosixErrno::ENOTCONN => Error::Notconn,
        PosixErrno::ENOTDIR => Error::Notdir,
        PosixErrno::ENOTEMPTY => Error::Notempty,
        PosixErrno::ENOTSOCK => Error::Notsock,
        PosixErrno::ENOTSUP | PosixErrno::EOPNOTSUPP => Error::Notsup,
        PosixErrno::ENOTTY => Error::Notty,
        PosixErrno::EOVERFLOW => Error::Overflow,
        PosixErrno::EPERM => Error::Perm,
        PosixErrno::EPIPE => Error::Pipe,
        PosixErrno::EPROTO => Error::Proto,
        PosixErrno::EPROTONOSUPPORT => Error::Protonosupport,
        PosixErrno::EROFS => Error::Rofs,
        PosixErrno::ESPIPE => Error::Spipe,
        PosixErrno::ESRCH => Error::Srch,
        PosixErrno::ESTALE => Error::Stale,
        PosixErrno::ETIMEDOUT => Error::Timedout,
        PosixErrno::EXDEV => Error::Xdev,
        _ => Error::Io,
    }
}

/// The Preview 1 errno a host function reports for an I/O failure kind.
///
/// Built on [`IoErrorKind::posix_errno`]; the only departures are the WASI
/// specifics: a path that escapes its preopened root is `ENOTCAPABLE`, not
/// `EACCES`. `WouldBlock` is `EAGAIN` (retry after readiness) while
/// `ResourceBusy` is `EBUSY` (administrative: waiting will not help, as for
/// renaming or removing a mount point or an open file) and `StaleHandle` is
/// `ESTALE` (reopen the file).
///
/// Descriptor exhaustion has two errnos: `SystemFileLimit` is `ENFILE` and
/// stands for a shared pool that ran out, such as the pipe and poll slots
/// that every process uses, while `ProcessFileLimit` is `EMFILE` and stands
/// for the descriptor table of one WASI process (the host reports it itself
/// as [`Error::Mfile`]). `TimedOut` is `ETIMEDOUT` and `SymlinkLoop` is `ELOOP`.
pub fn kind_error(kind: IoErrorKind) -> Error {
    match kind {
        IoErrorKind::PathEscapesRoot => Error::Notcapable,
        kind => errno_error(kind.posix_errno()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_has_an_explicit_preview1_errno() {
        for &kind in IoErrorKind::ALL {
            let error = kind_error(kind);
            assert!((1..=76).contains(&i32::from(error)), "{kind:?}");
            // `Io` is only the result for an I/O-class kind, never an unmapped errno.
            if error == Error::Io {
                assert_eq!(kind.posix_errno(), PosixErrno::EIO, "{kind:?} fell through");
            }
        }
    }

    #[test]
    fn file_creation_and_lookup_semantics_are_preserved() {
        assert_eq!(kind_error(IoErrorKind::NotFound), Error::Noent);
        assert_eq!(kind_error(IoErrorKind::MissingPathComponent), Error::Noent);
        assert_eq!(kind_error(IoErrorKind::MissingFinalEntry), Error::Noent);
        assert_eq!(kind_error(IoErrorKind::AlreadyExists), Error::Exist);
        assert_eq!(kind_error(IoErrorKind::PermissionDenied), Error::Access);
        assert_eq!(kind_error(IoErrorKind::OperationNotPermitted), Error::Perm);
        assert_eq!(kind_error(IoErrorKind::ReadOnlyFileSystem), Error::Rofs);
        assert_eq!(kind_error(IoErrorKind::NotADirectory), Error::Notdir);
        assert_eq!(kind_error(IoErrorKind::IsADirectory), Error::Isdir);
        assert_eq!(kind_error(IoErrorKind::DirectoryNotEmpty), Error::Notempty);
    }

    #[test]
    fn storage_and_memory_exhaustion_have_distinct_errnos() {
        assert_eq!(kind_error(IoErrorKind::NoSpace), Error::Nospc);
        assert_eq!(kind_error(IoErrorKind::OutOfMemory), Error::Nomem);
        assert_eq!(kind_error(IoErrorKind::FileTooLarge), Error::Fbig);
        assert_eq!(kind_error(IoErrorKind::NameTooLong), Error::Nametoolong);
        assert_eq!(kind_error(IoErrorKind::ProcessFileLimit), Error::Mfile);
        assert_eq!(kind_error(IoErrorKind::SystemFileLimit), Error::Nfile);
    }

    #[test]
    fn shared_slot_exhaustion_is_enfile_and_the_process_table_is_emfile() {
        // Pipe and poll slots are shared by every process, so running out of
        // them is the system-wide limit; only a process's own table is EMFILE.
        assert_eq!(kind_error(IoErrorKind::SystemFileLimit), Error::Nfile);
        assert_eq!(kind_error(IoErrorKind::ProcessFileLimit), Error::Mfile);
        assert_eq!(i32::from(Error::Nfile), 41);
        assert_eq!(i32::from(Error::Mfile), 33);
    }

    #[test]
    fn deadlines_and_link_loops_are_not_input_output_errors() {
        assert_eq!(kind_error(IoErrorKind::TimedOut), Error::Timedout);
        assert_eq!(i32::from(Error::Timedout), 73);
        assert_eq!(kind_error(IoErrorKind::SymlinkLoop), Error::Loop);
        assert_eq!(i32::from(Error::Loop), 32);
    }

    #[test]
    fn stale_handles_are_not_retryable_contention() {
        assert_eq!(kind_error(IoErrorKind::StaleHandle), Error::Stale);
        assert_eq!(i32::from(Error::Stale), 72);
        assert_ne!(kind_error(IoErrorKind::StaleHandle), Error::Again);
        assert_ne!(kind_error(IoErrorKind::ResourceBusy), Error::Again);
    }

    #[test]
    fn descriptor_link_and_mount_failures_have_distinct_errnos() {
        assert_eq!(kind_error(IoErrorKind::BadDescriptor), Error::Badf);
        assert_eq!(kind_error(IoErrorKind::SymlinkLoop), Error::Loop);
        assert_eq!(kind_error(IoErrorKind::CrossDevice), Error::Xdev);
        assert_eq!(kind_error(IoErrorKind::InvalidSeek), Error::Spipe);
        assert_eq!(
            kind_error(IoErrorKind::InappropriateIoControl),
            Error::Notty
        );
        assert_eq!(kind_error(IoErrorKind::DeviceUnavailable), Error::Nodev);
        assert_eq!(kind_error(IoErrorKind::CorruptData), Error::Io);
    }

    #[test]
    fn unsupported_and_unimplemented_operations_have_distinct_errnos() {
        assert_eq!(kind_error(IoErrorKind::Unsupported), Error::Notsup);
        assert_eq!(
            kind_error(IoErrorKind::SocketOperationUnsupported),
            Error::Notsup
        );
        assert_eq!(kind_error(IoErrorKind::NotImplemented), Error::Nosys);
    }

    #[test]
    fn invalid_arguments_are_invalid_for_guests() {
        for kind in [
            IoErrorKind::InvalidArgument,
            IoErrorKind::InvalidPath,
            IoErrorKind::InvalidFlags,
            IoErrorKind::InvalidState,
        ] {
            assert_eq!(kind_error(kind), Error::Inval, "{kind:?}");
        }
    }

    #[test]
    fn path_escapes_are_not_capable_instead_of_access_denied() {
        assert_eq!(kind_error(IoErrorKind::PathEscapesRoot), Error::Notcapable);
        assert_eq!(
            IoErrorKind::PathEscapesRoot.posix_errno(),
            PosixErrno::EACCES
        );
    }

    #[test]
    fn readiness_is_retryable_but_administrative_busy_is_not() {
        assert_eq!(kind_error(IoErrorKind::WouldBlock), Error::Again);
        assert_eq!(kind_error(IoErrorKind::ResourceBusy), Error::Busy);
        assert_eq!(kind_error(IoErrorKind::Interrupted), Error::Intr);
        assert_eq!(kind_error(IoErrorKind::TimedOut), Error::Timedout);
    }

    #[test]
    fn pipe_and_transport_failures_keep_their_errnos() {
        assert_eq!(kind_error(IoErrorKind::BrokenPipe), Error::Pipe);
        assert_eq!(
            kind_error(IoErrorKind::ConnectionRefused),
            Error::Connrefused
        );
        assert_eq!(kind_error(IoErrorKind::NetworkDown), Error::Netdown);
        assert_eq!(kind_error(IoErrorKind::HostUnreachable), Error::Hostunreach);
        assert_eq!(kind_error(IoErrorKind::NotASocket), Error::Notsock);
    }

    #[test]
    fn preview1_numerics_are_fixed() {
        // The Preview 1 ABI values are frozen: wasi-libc and guests hard-code them.
        assert_eq!(i32::from(Error::Again), 6);
        assert_eq!(i32::from(Error::Badf), 8);
        assert_eq!(i32::from(Error::Busy), 10);
        assert_eq!(i32::from(Error::Inval), 28);
        assert_eq!(i32::from(Error::Io), 29);
        assert_eq!(i32::from(Error::Loop), 32);
        assert_eq!(i32::from(Error::Noent), 44);
        assert_eq!(i32::from(Error::Nomem), 48);
        assert_eq!(i32::from(Error::Nospc), 51);
        assert_eq!(i32::from(Error::Notsup), 58);
        assert_eq!(i32::from(Error::Xdev), 75);
        assert_eq!(i32::from(Error::Notcapable), 76);
    }
}

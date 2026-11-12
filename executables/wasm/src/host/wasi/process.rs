use wasmi::Caller;

use crate::{
    define_wasi_module,
    host::{store::GlobalStore, wasi::error::WasiResult},
};

/// Highest signal number of the Preview 1 `signal` enumeration (`sys`).
const SIGNAL_MAXIMUM: i32 = 30;

/// What a signal does when nothing handles it. Preview 1 has no way to
/// install a handler, so the default disposition is the only one there is.
#[derive(Debug, PartialEq, Eq)]
enum Disposition {
    /// Not a signal of the enumeration (including 0).
    Invalid,
    /// `chld`, `urg`, `winch` and `cont` have nothing to act on.
    Ignore,
    /// `stop`, `tstp`, `ttin`, `ttou`: suspension needs job control Xila lacks.
    Stop,
    /// Everything else ends the process.
    Terminate,
}

fn disposition(signal: i32) -> Disposition {
    match signal {
        16 | 17 | 22 | 27 => Disposition::Ignore,
        18..=21 => Disposition::Stop,
        1..=SIGNAL_MAXIMUM => Disposition::Terminate,
        _ => Disposition::Invalid,
    }
}

define_wasi_module! {
    module: "wasi_snapshot_preview1";

    fn proc_exit(
        caller: Caller<GlobalStore>,
        code: i32,
    ) -> Result<(), wasmi::Error> {
        let mut caller = caller;
        caller.data_mut().wasi.exit_code = Some(code);
        Err(wasmi::Error::new("exit"))
    }

    // `raise` for a program that cannot install handlers: ignorable signals
    // return, terminating signals end the guest as `proc_exit(128 + signal)`
    // (the status a shell reports for a signalled child), and the stopping
    // signals are ENOTSUP because nothing can ever resume the guest.
    fn proc_raise(
        caller: Caller<GlobalStore>,
        signal: i32,
    ) -> Result<WasiResult, wasmi::Error> {
        match disposition(signal) {
            Disposition::Invalid => Ok(i32::from(super::Error::Inval)),
            Disposition::Ignore => Ok(0),
            Disposition::Stop => Ok(i32::from(super::Error::Notsup)),
            Disposition::Terminate => {
                let mut caller = caller;
                caller.data_mut().wasi.exit_code = Some(128 + signal);
                Err(wasmi::Error::new("exit"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_signal_of_the_enumeration_has_a_disposition() {
        for invalid in [i32::MIN, -1, 0, 31, i32::MAX] {
            assert_eq!(disposition(invalid), Disposition::Invalid, "{invalid}");
        }
        for ignored in [16, 17, 22, 27] {
            assert_eq!(disposition(ignored), Disposition::Ignore, "{ignored}");
        }
        for stopped in [18, 19, 20, 21] {
            assert_eq!(disposition(stopped), Disposition::Stop, "{stopped}");
        }
        // hup, int, quit, ill, trap, abrt, bus, fpe, kill, usr1, segv, usr2,
        // pipe, alrm, term, xcpu, xfsz, vtalrm, prof, poll, pwr, sys
        for terminating in (1..=15).chain([23, 24, 25, 26, 28, 29, 30]) {
            assert_eq!(disposition(terminating), Disposition::Terminate);
        }
    }
}

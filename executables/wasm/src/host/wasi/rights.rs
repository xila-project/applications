//! Per-descriptor rights.
//!
//! Xila does not hand out WASI rights piecemeal: a descriptor holds every
//! right unless the guest, or the directory it was opened from, reduced them.
//! Only reduced descriptors take an entry in [`WasiContext::rights`], so the
//! common case costs nothing. Reductions come from exactly two places:
//!
//! * `fd_fdstat_set_rights`, which can only remove rights;
//! * `path_open` through a directory with reduced inheriting rights, whose
//!   requested rights must then be a subset of what the directory may pass on.
//!
//! The reductions are enforced by every descriptor-based and path-based call
//! (each names the right it needs), and `fd_fdstat_get` reports them. Rights
//! requested by `path_open` through an unreduced directory are not recorded:
//! the new descriptor keeps every right and only the read/write intent is used
//! to choose the access mode. Rights are not Xila's confinement mechanism:
//! that is the preopened directory plus Xila's own permissions.

use super::{Error, WasiContext};

/// `rights` bit positions of the Preview 1 `rights` bitset.
pub(crate) mod right {
    pub const FD_DATASYNC: u64 = 1 << 0;
    pub const FD_READ: u64 = 1 << 1;
    pub const FD_SEEK: u64 = 1 << 2;
    pub const FD_FDSTAT_SET_FLAGS: u64 = 1 << 3;
    pub const FD_SYNC: u64 = 1 << 4;
    pub const FD_TELL: u64 = 1 << 5;
    pub const FD_WRITE: u64 = 1 << 6;
    pub const FD_ADVISE: u64 = 1 << 7;
    pub const FD_ALLOCATE: u64 = 1 << 8;
    pub const PATH_CREATE_DIRECTORY: u64 = 1 << 9;
    pub const PATH_CREATE_FILE: u64 = 1 << 10;
    pub const PATH_LINK_SOURCE: u64 = 1 << 11;
    pub const PATH_LINK_TARGET: u64 = 1 << 12;
    pub const PATH_OPEN: u64 = 1 << 13;
    pub const FD_READDIR: u64 = 1 << 14;
    pub const PATH_READLINK: u64 = 1 << 15;
    pub const PATH_RENAME_SOURCE: u64 = 1 << 16;
    pub const PATH_RENAME_TARGET: u64 = 1 << 17;
    pub const PATH_FILESTAT_GET: u64 = 1 << 18;
    pub const PATH_FILESTAT_SET_SIZE: u64 = 1 << 19;
    pub const PATH_FILESTAT_SET_TIMES: u64 = 1 << 20;
    pub const FD_FILESTAT_GET: u64 = 1 << 21;
    pub const FD_FILESTAT_SET_SIZE: u64 = 1 << 22;
    pub const FD_FILESTAT_SET_TIMES: u64 = 1 << 23;
    pub const PATH_SYMLINK: u64 = 1 << 24;
    pub const PATH_REMOVE_DIRECTORY: u64 = 1 << 25;
    pub const PATH_UNLINK_FILE: u64 = 1 << 26;
}

/// What a descriptor without a recorded reduction holds.
pub(crate) const ALL: (u64, u64) = (u64::MAX, u64::MAX);

impl WasiContext {
    /// The base and inheriting rights of `fd`.
    pub(crate) fn rights_of(&self, fd: u32) -> (u64, u64) {
        self.rights.get(&fd).copied().unwrap_or(ALL)
    }

    /// `EBADF` for a descriptor that is not open, `ENOTCAPABLE` when a needed
    /// right was removed from it.
    pub(crate) fn require_rights(&self, fd: i32, needed: u64) -> Result<(), Error> {
        let fd = fd as u32;
        if !self.files.contains_key(&fd) {
            return Err(Error::Badf);
        }
        if self.rights_of(fd).0 & needed != needed {
            return Err(Error::Notcapable);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> WasiContext {
        WasiContext {
            files: Default::default(),
            rights: Default::default(),
            next_fd: 3,
            arguments: Default::default(),
            environment: Default::default(),
            task: xila::task::TaskIdentifier::from(0usize),
            prestats: Default::default(),
            exit_code: None,
        }
    }

    #[test]
    fn right_bits_match_the_preview_one_bitset() {
        // The first and the last right are the cheapest way to catch a shifted table.
        assert_eq!(right::FD_DATASYNC, 1);
        assert_eq!(right::PATH_UNLINK_FILE, 1 << 26);
        assert_eq!(right::PATH_REMOVE_DIRECTORY, 1 << 25);
        assert_eq!(right::FD_FILESTAT_SET_TIMES, 1 << 23);
    }

    #[test]
    fn unknown_descriptors_are_not_open_even_without_a_reduction() {
        let wasi = context();
        assert_eq!(wasi.require_rights(5, right::FD_READ), Err(Error::Badf));
        assert_eq!(wasi.require_rights(-1, right::FD_READ), Err(Error::Badf));
    }
}

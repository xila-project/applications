use super::Error;
use alloc::{collections::btree_map::BTreeMap, string::String, vec::Vec};
use xila::{
    file_system::PathOwned,
    task::TaskIdentifier,
    virtual_file_system::{SynchronousDirectory, SynchronousFile},
};

pub struct WasiContext {
    pub files: BTreeMap<u32, FileSystemItem>,
    /// Reduced (base, inheriting) rights; absent descriptors hold every right.
    pub rights: BTreeMap<u32, (u64, u64)>,
    pub next_fd: u32,
    pub arguments: Vec<String>,
    pub environment: Vec<(String, String)>,
    pub task: TaskIdentifier,
    /// Preopen names by descriptor. They follow their descriptor when it is renumbered.
    pub prestats: BTreeMap<u32, Prestat>,
    pub exit_code: Option<i32>,
}

impl WasiContext {
    /// Allocate and insert the next available descriptor.
    pub fn insert_file_system_item(&mut self, item: FileSystemItem) -> Option<u32> {
        // Descriptors are only ever created here, but a table filled directly
        // (tests, embedders) may already hold the next number.
        while self.files.contains_key(&self.next_fd) {
            self.next_fd = self.next_fd.checked_add(1)?;
        }
        let fd = allocate_descriptor_id(&mut self.next_fd)?;
        self.files.insert(fd, item);
        Some(fd)
    }

    pub fn get_file_system_item(&mut self, fd: u32) -> Option<&mut FileSystemItem> {
        self.files.get_mut(&fd)
    }

    pub fn get_synchronous_file(&mut self, fd: u32) -> Option<&mut SynchronousFile<'static>> {
        self.files
            .get_mut(&fd)
            .and_then(|item| item.as_synchronous_file_mut())
    }

    /// The open file `fd`: `EBADF` when closed, `EISDIR` for a directory.
    pub(crate) fn file(&mut self, fd: i32) -> Result<&mut SynchronousFile<'static>, Error> {
        match self.files.get_mut(&(fd as u32)) {
            None => Err(Error::Badf),
            Some(item) => item.as_synchronous_file_mut().ok_or(Error::Isdir),
        }
    }

    pub fn is_directory(&self, fd: u32) -> bool {
        matches!(self.files.get(&fd), Some(FileSystemItem::Directory(_)))
    }

    pub fn get_synchronous_directory(
        &mut self,
        fd: u32,
    ) -> Option<&mut SynchronousDirectory<'static>> {
        self.files
            .get_mut(&fd)
            .and_then(|item| item.as_synchronous_directory_mut())
    }

    pub fn pop_file_system_item(&mut self, fd: u32) -> Option<FileSystemItem> {
        self.files.remove(&fd)
    }
}

pub struct Prestat {
    pub name: Vec<u8>,
}

pub struct FileVariant {
    pub file: SynchronousFile<'static>,
}

pub struct DirectoryVariant {
    /// Original preopen authority retained by directories opened through a guest path.
    pub confinement_root: Option<PathOwned>,
    pub path: PathOwned,
    pub directory: SynchronousDirectory<'static>,
}

pub enum FileSystemItem {
    StandardInput(FileVariant),
    StandardOutput(FileVariant),
    StandardError(FileVariant),
    File(FileVariant),
    Directory(DirectoryVariant),
}

impl FileSystemItem {
    pub fn file_type(&self) -> u8 {
        match self {
            FileSystemItem::StandardInput(_)
            | FileSystemItem::StandardOutput(_)
            | FileSystemItem::StandardError(_) => descriptor_type(DescriptorKind::CharacterDevice),
            FileSystemItem::File(_) => descriptor_type(DescriptorKind::RegularFile),
            FileSystemItem::Directory(_) => descriptor_type(DescriptorKind::Directory),
        }
    }

    pub fn as_synchronous_file_mut(&mut self) -> Option<&mut SynchronousFile<'static>> {
        match self {
            FileSystemItem::StandardInput(file) => Some(&mut file.file),
            FileSystemItem::StandardOutput(file) => Some(&mut file.file),
            FileSystemItem::StandardError(file) => Some(&mut file.file),
            FileSystemItem::File(file) => Some(&mut file.file),
            FileSystemItem::Directory(_) => None,
        }
    }

    pub fn as_synchronous_directory_mut(&mut self) -> Option<&mut SynchronousDirectory<'static>> {
        match self {
            FileSystemItem::Directory(dir) => Some(&mut dir.directory),
            _ => None,
        }
    }

    /// Compatibility name for borrowing the contained file.
    #[allow(
        clippy::wrong_self_convention,
        reason = "Preserve the existing public borrowed-view API"
    )]
    pub fn into_synchronous_file(&mut self) -> Option<&mut SynchronousFile<'static>> {
        self.as_synchronous_file_mut()
    }

    /// Compatibility name for borrowing the contained directory.
    #[allow(
        clippy::wrong_self_convention,
        reason = "Preserve the existing public borrowed-view API"
    )]
    pub fn into_synchronous_directory(&mut self) -> Option<&mut SynchronousDirectory<'static>> {
        self.as_synchronous_directory_mut()
    }
}

#[derive(Clone, Copy)]
enum DescriptorKind {
    CharacterDevice,
    RegularFile,
    Directory,
}

fn descriptor_type(kind: DescriptorKind) -> u8 {
    match kind {
        DescriptorKind::CharacterDevice => 2,
        DescriptorKind::Directory => 3,
        DescriptorKind::RegularFile => 4,
    }
}

fn allocate_descriptor_id(next_fd: &mut u32) -> Option<u32> {
    let fd = *next_fd;
    *next_fd = next_fd.checked_add(1)?;
    Some(fd)
}

#[cfg(test)]
mod tests {
    #[test]
    fn descriptor_ids_increase_without_wrapping() {
        let mut next_fd = 3u32;
        assert_eq!(super::allocate_descriptor_id(&mut next_fd), Some(3));
        assert_eq!(super::allocate_descriptor_id(&mut next_fd), Some(4));

        next_fd = u32::MAX;
        assert_eq!(super::allocate_descriptor_id(&mut next_fd), None);
    }

    #[test]
    fn descriptor_kinds_match_wasi_filetype_values() {
        assert_eq!(
            super::descriptor_type(super::DescriptorKind::RegularFile),
            4
        );
        assert_eq!(super::descriptor_type(super::DescriptorKind::Directory), 3);
        assert_eq!(
            super::descriptor_type(super::DescriptorKind::CharacterDevice),
            2
        );
    }
}

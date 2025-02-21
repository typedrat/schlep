#![allow(clippy::field_reassign_with_default)]

use std::{
    os::unix::fs::MetadataExt as _,
    sync::LazyLock,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use bitflags::bitflags;
use cap_primitives::fs::MetadataExt as _;
use cap_std::fs::OpenOptions;
use russh_sftp::protocol::FileAttributes;
use rustix::fs::{StatVfs, StatVfsMountFlags};
use tracing::{Level, event};

#[derive(Debug, Default, Copy, Clone)]
pub struct Metadata {
    pub(super) size: Option<u64>,
    pub(super) atime: Option<SystemTime>,
    pub(super) mtime: Option<SystemTime>,
    pub(super) is_directory: bool,
}

impl Metadata {
    #[must_use]
    pub fn size(&self) -> Option<u64> {
        self.size
    }

    #[must_use]
    pub fn atime(&self) -> Option<SystemTime> {
        self.atime
    }

    #[must_use]
    pub fn mtime(&self) -> Option<SystemTime> {
        self.mtime
    }

    #[must_use]
    pub fn is_directory(&self) -> bool {
        self.is_directory
    }

    pub fn file_attrs(&self, file_mode: u32, dir_mode: u32) -> FileAttributes {
        let mut attrs = FileAttributes::default();

        attrs.size = self.size;
        attrs.atime = self.atime.and_then(from_system_time);
        attrs.mtime = self.mtime.and_then(from_system_time);

        if self.is_directory {
            attrs.permissions = Some((0o004 << 12) | dir_mode);
        } else {
            attrs.permissions = Some((0o010 << 12) | file_mode);
        }

        attrs
    }
}

impl From<FileAttributes> for Metadata {
    fn from(value: FileAttributes) -> Self {
        fn to_system_time(x: u32) -> SystemTime {
            SystemTime::UNIX_EPOCH + Duration::from_secs(u64::from(x))
        }

        let mut out = Metadata::default();

        out.size = value.size;
        out.atime = value.atime.map(to_system_time);
        out.mtime = value.mtime.map(to_system_time);

        out
    }
}

impl From<std::fs::Metadata> for Metadata {
    fn from(value: std::fs::Metadata) -> Self {
        let mut out = Metadata::default();

        out.size = Some(value.size());
        out.atime = Some(to_system_time(value.atime()));
        out.mtime = Some(to_system_time(value.mtime()));
        out.is_directory = value.is_dir();

        out
    }
}

impl From<cap_std::fs::Metadata> for Metadata {
    fn from(value: cap_std::fs::Metadata) -> Self {
        let mut out = Metadata::default();

        out.size = Some(value.size());
        out.atime = Some(to_system_time(value.atime()));
        out.mtime = Some(to_system_time(value.mtime()));
        out.is_directory = value.is_dir();

        out
    }
}

fn from_system_time(system_time: SystemTime) -> Option<u32> {
    if let Ok(duration) = system_time.duration_since(UNIX_EPOCH) {
        if let Ok(duration) = duration.as_secs().try_into() {
            Some(duration)
        } else {
            event!(Level::DEBUG, "SystemTime won't fit in a u32");
            None
        }
    } else {
        event!(Level::DEBUG, "system time before UNIX EPOCH");
        None
    }
}

fn to_system_time(x: i64) -> SystemTime {
    #[allow(clippy::cast_sign_loss)]
    if x < 0 {
        SystemTime::UNIX_EPOCH - Duration::from_secs(-x as u64)
    } else {
        SystemTime::UNIX_EPOCH + Duration::from_secs(x as u64)
    }
}

/// The core filesystem metadata that VFSes need to report if they implement
/// [`Vfs::statvfs_fd`].
#[derive(Debug, Clone, Copy)]
pub struct FsMetadata {
    pub block_size: u64,
    pub num_blocks: u64,
    pub free_blocks: u64,
    pub num_files: u64,
    pub free_files: u64,
    pub read_only: bool,
    pub max_length: u64,
}

impl From<StatVfs> for FsMetadata {
    fn from(stat: StatVfs) -> Self {
        FsMetadata {
            block_size: stat.f_frsize,
            num_blocks: stat.f_blocks,
            free_blocks: stat.f_bfree,
            num_files: stat.f_files,
            free_files: stat.f_ffree,
            read_only: stat.f_flag.contains(StatVfsMountFlags::RDONLY),
            max_length: stat.f_namemax,
        }
    }
}

/// The simplified lowest-common-denominator of file-opening types that the VFS
/// needs to support.
#[repr(transparent)]
#[derive(Default, Copy, Clone, Eq, PartialEq)]
pub struct OpenFlags(u32);

bitflags! {
    impl OpenFlags: u32 {
        const READ     = 0x0000_0001;
        const WRITE    = 0x0000_0002;
        const APPEND   = 0x0000_0004;
        const CREATE   = 0x0000_0008;
        const TRUNCATE = 0x0000_0010;
        const EXCLUDE  = 0x0000_0020;
    }
}

impl OpenFlags {
    #[must_use]
    pub fn new() -> Self {
        Self(0)
    }
}

impl From<russh_sftp::protocol::OpenFlags> for OpenFlags {
    fn from(pflags: russh_sftp::protocol::OpenFlags) -> Self {
        use russh_sftp::protocol::OpenFlags as SftpOpenFlags;

        static FLAGS: LazyLock<Vec<(SftpOpenFlags, OpenFlags)>> = LazyLock::new(|| {
            vec![
                (SftpOpenFlags::READ, OpenFlags::READ),
                (SftpOpenFlags::WRITE, OpenFlags::WRITE),
                (SftpOpenFlags::APPEND, OpenFlags::APPEND),
                (SftpOpenFlags::CREATE, OpenFlags::CREATE),
                (SftpOpenFlags::TRUNCATE, OpenFlags::TRUNCATE),
                (SftpOpenFlags::EXCLUDE, OpenFlags::EXCLUDE),
            ]
        });

        let mut out = Self::empty();

        for (src, dst) in FLAGS.iter() {
            if pflags.contains(*src) {
                out |= *dst;
            }
        }

        out
    }
}

macro_rules! convert_flags {
    ( $input:ident, $output:ident, [ $(( $src:expr , $dst:ident ), )* ] ) => {
        {
            $(
                if ($input.contains($src)) {
                    $output.$dst(true);
                }
            )*
        }
    };
}

impl From<OpenFlags> for OpenOptions {
    fn from(flags: OpenFlags) -> Self {
        let mut out = OpenOptions::new();

        convert_flags!(
            flags,
            out,
            [
                (OpenFlags::READ, read),
                (OpenFlags::WRITE, write),
                (OpenFlags::APPEND, append),
                (OpenFlags::CREATE, create),
                (OpenFlags::TRUNCATE, truncate),
                (OpenFlags::EXCLUDE, create_new),
            ]
        );

        out
    }
}

use std::{
    os::unix::fs::MetadataExt as _,
    sync::LazyLock,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use bitflags::{bitflags, bitflags_match};
use cap_primitives::fs::MetadataExt as _;
use cap_std::fs::OpenOptions;
use russh_sftp::protocol::FileAttributes;
use tracing::{event, Level};

#[derive(Debug, Default, Copy, Clone)]
pub struct Metadata {
    pub size: Option<u64>,
    pub atime: Option<SystemTime>,
    pub mtime: Option<SystemTime>,
}

impl From<russh_sftp::protocol::FileAttributes> for Metadata {
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

impl From<Metadata> for russh_sftp::protocol::FileAttributes {
    fn from(metadata: Metadata) -> Self {
        fn from_system_time(system_time: SystemTime) -> Option<u32> {
            if let Ok(duration) = system_time.duration_since(UNIX_EPOCH) {
                Some(duration.as_secs() as u32)
            } else {
                event!(Level::DEBUG, "system time before UNIX EPOCH");
                None
            }
        }

        let mut attrs = FileAttributes::default();

        attrs.size = metadata.size;
        attrs.atime = metadata.atime.and_then(from_system_time);
        attrs.mtime = metadata.mtime.and_then(from_system_time);

        attrs
    }
}

impl From<std::fs::Metadata> for Metadata {
    fn from(value: std::fs::Metadata) -> Self {
        fn to_system_time(x: i64) -> SystemTime {
            if x < 0 {
                SystemTime::UNIX_EPOCH - Duration::from_secs(-x as u64)
            } else {
                SystemTime::UNIX_EPOCH + Duration::from_secs(x as u64)
            }
        }

        let mut out = Metadata::default();

        out.size = Some(value.size());
        out.atime = Some(to_system_time(value.atime()));
        out.mtime = Some(to_system_time(value.mtime()));

        out
    }
}

impl From<cap_std::fs::Metadata> for Metadata {
    fn from(value: cap_std::fs::Metadata) -> Self {
        fn to_system_time(x: i64) -> SystemTime {
            if x < 0 {
                SystemTime::UNIX_EPOCH - Duration::from_secs(-x as u64)
            } else {
                SystemTime::UNIX_EPOCH + Duration::from_secs(x as u64)
            }
        }

        let mut out = Metadata::default();

        out.size = Some(value.size());
        out.atime = Some(to_system_time(value.atime()));
        out.mtime = Some(to_system_time(value.mtime()));

        out
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

/// The simplified lowest-common-denominator of file-opening types that the VFS
/// needs to support.
#[repr(transparent)]
#[derive(Default, Copy, Clone, Eq, PartialEq)]
pub struct OpenFlags(u32);

bitflags! {
    impl OpenFlags: u32 {
        const READ = 0x00000001;
        const WRITE = 0x00000002;
        const APPEND = 0x00000004;
        const CREATE = 0x00000008;
        const TRUNCATE = 0x00000010;
        const EXCLUDE = 0x00000020;
    }
}

impl OpenFlags {
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

impl From<OpenFlags> for OpenOptions {
    fn from(flags: OpenFlags) -> Self {
        let mut opts = OpenOptions::new();

        bitflags_match!(flags, {
            OpenFlags::READ => { let _ = opts.read(true); }
            OpenFlags::WRITE => { let _ = opts.write(true); }
            OpenFlags::APPEND => { let _ = opts.append(true); }
            OpenFlags::CREATE => { let _ = opts.create(true); }
            OpenFlags::TRUNCATE => { let _ = opts.truncate(true); }
            OpenFlags::EXCLUDE => { let _ = opts.create_new(true); }
            _ => {}
        });

        opts
    }
}

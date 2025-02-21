//! To enable multiple different filesystem backends at various different paths
//! to be plugged into the various servers that Schlep implements, Schlep
//! includes an internal virtual filesystem layer to abstract the details of the
//! underlying filesystem implementation and handle routing different absolute
//! paths into relative ones.
//!
//! # The [`Vfs`] Trait
//! Obviously, this forms the core of this layer, serving as the abstraction
//! trait itself. While the documentation looks a bit hairy due to the use of
//! `#[async_trait]`, the actual structure is simpler. It simply provides a
//! uniform interface using uniform types for the servers and filesystem
//! backends to communicate with each other.

mod error;
mod local_dir;
mod options;
mod vfs_trait;

pub use error::Error;
pub use local_dir::*;
pub use options::*;
pub use vfs_trait::*;

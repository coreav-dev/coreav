//! Storage supplied to a decoder's picture allocator.

use std::sync::Arc;

use crate::Error;

/// Owned writable storage. A decoder borrows it once to obtain the
/// allocation's address and keeps the owner until its release callback.
pub trait DecodeBuffer: Send + Sync {
    /// The allocation, whose address and length stay fixed until drop.
    fn bytes_mut(&mut self) -> &mut [u8];

    /// The slot in the allocator's shared-memory domain, if any.
    fn shared_slot(&self) -> Option<u32> {
        None
    }
}

impl DecodeBuffer for Vec<u8> {
    fn bytes_mut(&mut self) -> &mut [u8] {
        self
    }
}

/// Allocates decoder-owned storage, which may outlive a displayed frame
/// while the decoder retains it as a reference picture.
pub trait BufferAllocator: Send + Sync {
    /// At least `len` initialized bytes, or a bounded allocation error.
    fn allocate(&self, len: usize) -> Result<Box<dyn DecodeBuffer>, Error>;
}

/// An allocator that can be shared by decoder callback threads.
pub type PictureAllocator = Arc<dyn BufferAllocator>;

/// Immutable ownership of storage handed to a decoder. Clones keep the
/// allocation out of its pool until both decoding and publication end.
#[derive(Clone)]
pub struct BufferLease(Arc<Box<dyn DecodeBuffer>>);

impl From<Box<dyn DecodeBuffer>> for BufferLease {
    fn from(buffer: Box<dyn DecodeBuffer>) -> Self {
        Self(Arc::new(buffer))
    }
}

impl BufferLease {
    /// The slot in the allocator's shared-memory domain, if any.
    pub fn shared_slot(&self) -> Option<u32> {
        self.0.shared_slot()
    }
}

/// One displayed plane inside a decoder allocation's shared slot.
#[derive(Clone, Debug)]
pub struct BufferPlane {
    /// Byte offset relative to the slot's beginning.
    pub offset: u64,
    /// Bytes between consecutive displayed rows.
    pub stride: u64,
    /// Bytes through the final displayed row, excluding its padding.
    pub len: u64,
}

/// A displayed picture already resident in shared memory.
#[derive(Clone, Debug)]
pub struct SharedFrame {
    /// Slot identifier in the supplied allocator's domain.
    pub slot: u32,
    /// Displayed planes, excluding borders and padding after the last row.
    pub planes: Vec<BufferPlane>,
}

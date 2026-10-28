//! Shared memory for frames (`docs/design/decode.md`, "Shared memory"):
//! the only module of `coreav-worker` allowed `unsafe`, for one mapping
//! call through `memmap2` and one copy out of a mapping.
//!
//! The parent creates an anonymous memory file, sizes it and seals it
//! against shrinking and growing, all through `rustix`'s safe API, then maps
//! it read-only as a raw mapping (`memmap2::MmapRaw`, created without
//! `unsafe`). The worker opens it through `/proc/<parent>/fd/<n>` and maps
//! it read-write. Unmapping is `Drop`.
//!
//! Why the two `unsafe` sites are sound here:
//! - Truncation (both): a read or write past the end of a file that shrank
//!   while mapped faults. The parent seals the file with `F_SEAL_SHRINK`
//!   and `F_SEAL_GROW` (then `F_SEAL_SEAL`) before the worker can open it,
//!   and both sides map exactly the sealed length, so no process can shrink
//!   it under either mapping.
//! - Concurrent writes (the parent's copy): a compromised worker may write
//!   a slot while the parent copies it. The parent never creates a Rust
//!   reference to the mapping, whose contents another process changes: it
//!   copies each validated plane once, by raw pointer
//!   ([`ParentMemory::copy_out`]), into memory it owns. A racing write can
//!   only change the copied pixel values, which the worker controls anyway
//!   (every bit pattern is a valid `u8`); no byte of the mapping is read
//!   twice or steers the parent, since descriptors come from the pipe.
//! - The worker's mapping (`WorkerMemory::bytes_mut`) is a `&mut [u8]` over
//!   memory the parent only reads; the worker trusts nothing in it.

#![allow(unsafe_code)]

use std::fs::File;
use std::io;

use memmap2::{MmapMut, MmapOptions, MmapRaw};
use rustix::fs::{MemfdFlags, SealFlags, fcntl_add_seals, fcntl_get_seals, memfd_create};

/// The seals the parent sets and checks.
fn seals() -> SealFlags {
    SealFlags::SHRINK | SealFlags::GROW | SealFlags::SEAL
}

/// The parent's sealed memory file, mapped read-only, read only by copying.
pub struct ParentMemory {
    file: File,
    map: MmapRaw,
}

impl ParentMemory {
    /// Creates a sealed memory file of `len` bytes and maps it read-only.
    pub fn create(len: u64) -> io::Result<Self> {
        if len == 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "empty memory"));
        }
        let fd = memfd_create(
            "coreav-frames",
            MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING,
        )?;
        let file = File::from(fd);
        file.set_len(len)?;
        fcntl_add_seals(&file, seals())?;
        if !fcntl_get_seals(&file)?.contains(seals()) {
            return Err(io::Error::other("memory file is not sealed"));
        }
        let size = usize::try_from(len)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "memory too large"))?;
        let map = MmapOptions::new().len(size).map_raw_read_only(&file)?;
        Ok(Self { file, map })
    }

    /// The path under which the worker opens the file: this process's
    /// descriptor in `/proc`. The descriptor stays close-on-exec, so no
    /// child inherits it.
    pub fn path_for_worker(&self) -> String {
        use std::os::fd::AsRawFd;
        format!("/proc/{}/fd/{}", std::process::id(), self.file.as_raw_fd())
    }

    /// Its length in bytes.
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// Whether it is empty (never: [`ParentMemory::create`] refuses 0).
    pub fn is_empty(&self) -> bool {
        self.map.len() == 0
    }

    /// Copies `out.len()` bytes from offset `start` of the mapping into
    /// `out`; `None`, copying nothing, when the range is not inside it.
    pub fn copy_out(&self, start: usize, out: &mut [u8]) -> Option<()> {
        let end = start.checked_add(out.len())?;
        if end > self.map.len() {
            return None;
        }
        // SAFETY: `start..end` is inside the mapping (checked above), which
        // is valid for reads for as long as `self` lives: the file is sealed
        // against shrinking and mapped at exactly its length (module
        // documentation). `out` is memory this process owns, so it does not
        // overlap the mapping. No reference to the mapping is created; a
        // concurrent write by the worker changes only the copied values.
        unsafe {
            std::ptr::copy_nonoverlapping(
                self.map.as_ptr().add(start),
                out.as_mut_ptr(),
                out.len(),
            );
        }
        Some(())
    }
}

impl crate::descriptor::SlotMemory for ParentMemory {
    fn len(&self) -> usize {
        ParentMemory::len(self)
    }

    fn copy_out(&self, start: usize, out: &mut [u8]) -> Option<()> {
        ParentMemory::copy_out(self, start, out)
    }
}

/// The worker's read-write mapping of the parent's memory file.
pub struct WorkerMemory {
    map: MmapMut,
}

impl WorkerMemory {
    /// Opens the parent's memory file at `path` and maps its `len` bytes
    /// read-write, after checking its size and seals.
    pub fn open(path: &str, len: u64) -> io::Result<Self> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)?;
        if file.metadata()?.len() != len || !fcntl_get_seals(&file)?.contains(seals()) {
            return Err(io::Error::other("memory file is not the parent's"));
        }
        let size = usize::try_from(len)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "memory too large"))?;
        // SAFETY: the file is sealed against shrinking and growing (checked
        // above) and `size` bytes long, so the mapping never extends past
        // its end. The parent only reads it.
        let map = unsafe { MmapOptions::new().len(size).map_mut(&file)? };
        Ok(Self { map })
    }

    /// The mapping, writable.
    pub fn bytes_mut(&mut self) -> &mut [u8] {
        &mut self.map
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_writes_parent_reads() {
        let parent = ParentMemory::create(4096).expect("create");
        let mut worker = WorkerMemory::open(&parent.path_for_worker(), 4096).expect("open");
        worker
            .bytes_mut()
            .get_mut(100..104)
            .expect("range")
            .copy_from_slice(&[1, 2, 3, 4]);
        let mut copied = [0u8; 4];
        assert_eq!(parent.copy_out(100, &mut copied), Some(()));
        assert_eq!(copied, [1, 2, 3, 4]);
        assert_eq!(parent.copy_out(4093, &mut copied), None);
        assert_eq!(parent.copy_out(usize::MAX, &mut copied), None);
        assert_eq!(parent.copy_out(4092, &mut copied), Some(()));
        // Sealed: the size cannot change.
        assert!(parent.file.set_len(1).is_err());
        assert!(parent.file.set_len(8192).is_err());
        assert!(WorkerMemory::open(&parent.path_for_worker(), 8192).is_err());
    }
}

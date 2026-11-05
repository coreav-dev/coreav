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
use std::ops::Range;
use std::os::fd::{AsRawFd, BorrowedFd};
use std::sync::{Arc, Mutex};

use coreav_decode::{BufferAllocator, DecodeBuffer, Error, ErrorKind};

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

    /// Writes checked ranges directly to a pipe without borrowing the
    /// worker-writable bytes. The kernel copies them before returning.
    pub fn write_ranges(&self, pipe: BorrowedFd<'_>, ranges: &[Range<usize>]) -> io::Result<()> {
        if ranges
            .iter()
            .any(|r| r.start > r.end || r.end > self.map.len())
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "range outside shared memory",
            ));
        }
        let mut ranges = ranges.iter();
        loop {
            let mut vectors: Vec<libc::iovec> = ranges
                .by_ref()
                .take(1024)
                .filter(|r| !r.is_empty())
                .map(|r| libc::iovec {
                    iov_base: self.map.as_ptr().wrapping_add(r.start).cast_mut().cast(),
                    iov_len: r.len(),
                })
                .collect();
            if vectors.is_empty() {
                if ranges.len() == 0 {
                    return Ok(());
                }
                continue;
            }
            let mut first = 0;
            while first < vectors.len() {
                let remaining = vectors.get(first..).unwrap_or_default();
                let n = unsafe {
                    libc::writev(
                        pipe.as_raw_fd(),
                        remaining.as_ptr(),
                        remaining.len() as libc::c_int,
                    )
                };
                if n < 0 {
                    let error = io::Error::last_os_error();
                    if error.kind() == io::ErrorKind::Interrupted {
                        continue;
                    }
                    return Err(error);
                }
                if n == 0 {
                    return Err(io::ErrorKind::WriteZero.into());
                }
                let mut consumed = n as usize;
                while let Some(vector) = vectors.get_mut(first) {
                    if consumed < vector.iov_len {
                        vector.iov_base =
                            vector.iov_base.cast::<u8>().wrapping_add(consumed).cast();
                        vector.iov_len -= consumed;
                        break;
                    }
                    consumed -= vector.iov_len;
                    first += 1;
                    if consumed == 0 {
                        break;
                    }
                }
            }
        }
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
        Self::open_prefix(path, len, len)
    }

    /// Maps only the transfer slots, leaving decoder slots to their own
    /// exclusive mappings.
    pub fn open_prefix(path: &str, file_len: u64, len: u64) -> io::Result<Self> {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)?;
        if len > file_len
            || file.metadata()?.len() != file_len
            || !fcntl_get_seals(&file)?.contains(seals())
        {
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

#[derive(Clone)]
pub(crate) struct Pool(Arc<PoolInner>);

struct PoolInner {
    file: File,
    slot_size: usize,
    budget: usize,
    state: Mutex<PoolState>,
}

struct PoolState {
    unused: Vec<u32>,
    bytes: usize,
    free: Vec<Storage>,
}

enum Storage {
    Shared { slot: u32, map: MmapMut },
    Private(Vec<u8>),
}

impl Storage {
    fn len(&self) -> usize {
        match self {
            Self::Shared { map, .. } => map.len(),
            Self::Private(data) => data.capacity(),
        }
    }
}

struct Pooled {
    storage: Option<Storage>,
    pool: Pool,
}

impl DecodeBuffer for Pooled {
    fn bytes_mut(&mut self) -> &mut [u8] {
        match self.storage.as_mut() {
            Some(Storage::Shared { map, .. }) => map,
            Some(Storage::Private(data)) => data,
            None => &mut [],
        }
    }

    fn shared_slot(&self) -> Option<u32> {
        match &self.storage {
            Some(Storage::Shared { slot, .. }) => Some(*slot),
            _ => None,
        }
    }
}

impl Drop for Pooled {
    fn drop(&mut self) {
        if let Some(storage) = self.storage.take() {
            if let Ok(mut state) = self.pool.0.state.lock() {
                state.free.push(storage);
            }
        }
    }
}

impl Pool {
    pub(crate) fn open(
        path: &str,
        slots: u32,
        buffers: u32,
        slot_size: u64,
        budget: u64,
    ) -> io::Result<Self> {
        let count = slots
            .checked_add(buffers)
            .ok_or_else(|| io::Error::other("too many slots"))?;
        let file_len = u64::from(count)
            .checked_mul(slot_size)
            .ok_or_else(|| io::Error::other("memory size overflows"))?;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)?;
        if file.metadata()?.len() != file_len || !fcntl_get_seals(&file)?.contains(seals()) {
            return Err(io::Error::other("memory file is not the parent's"));
        }
        Ok(Self(Arc::new(PoolInner {
            file,
            slot_size: usize::try_from(slot_size)
                .map_err(|_| io::Error::other("slot too large"))?,
            budget: usize::try_from(budget).unwrap_or(usize::MAX),
            state: Mutex::new(PoolState {
                unused: (slots..count).rev().collect(),
                bytes: 0,
                free: Vec::new(),
            }),
        })))
    }
}

impl BufferAllocator for Pool {
    fn allocate(&self, len: usize) -> Result<Box<dyn DecodeBuffer>, Error> {
        let fail = || Error::new(ErrorKind::Limit, "out of memory for a decoder buffer");
        let len = len
            .checked_add(4095)
            .map(|n| n & !4095)
            .filter(|n| *n > 0)
            .ok_or_else(fail)?;
        let mut state = self.0.state.lock().map_err(|_| fail())?;
        let index = state
            .free
            .iter()
            .position(|s| s.len() >= len && matches!(s, Storage::Shared { .. }))
            .or_else(|| state.free.iter().position(|s| s.len() >= len));
        let storage = if let Some(index) = index {
            state.free.swap_remove(index)
        } else {
            while state
                .bytes
                .checked_add(len)
                .is_none_or(|n| n > self.0.budget)
            {
                let Some(storage) = state.free.pop() else {
                    return Err(fail());
                };
                state.bytes = state.bytes.saturating_sub(storage.len());
                if let Storage::Shared { slot, .. } = storage {
                    state.unused.push(slot);
                }
            }
            if len <= self.0.slot_size && state.unused.is_empty() {
                if let Some(index) = state
                    .free
                    .iter()
                    .position(|s| matches!(s, Storage::Shared { .. }))
                {
                    let storage = state.free.swap_remove(index);
                    state.bytes = state.bytes.saturating_sub(storage.len());
                    if let Storage::Shared { slot, .. } = storage {
                        state.unused.push(slot);
                    }
                }
            }
            let shared_slot = if len <= self.0.slot_size {
                state.unused.pop()
            } else {
                None
            };
            let storage = if let Some(slot) = shared_slot {
                let offset = u64::from(slot)
                    .checked_mul(self.0.slot_size as u64)
                    .ok_or_else(fail)?;
                let map = unsafe {
                    MmapOptions::new()
                        .offset(offset)
                        .len(len)
                        .map_mut(&self.0.file)
                }
                .map_err(|_| {
                    state.unused.push(slot);
                    fail()
                })?;
                Storage::Shared { slot, map }
            } else {
                let mut data = Vec::new();
                data.try_reserve_exact(len).map_err(|_| fail())?;
                data.resize(len, 0);
                Storage::Private(data)
            };
            state.bytes = state.bytes.checked_add(storage.len()).ok_or_else(fail)?;
            storage
        };
        drop(state);
        Ok(Box::new(Pooled {
            storage: Some(storage),
            pool: self.clone(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoder_and_parent_leases_both_end_before_pool_reuse() {
        let parent = ParentMemory::create(3 * 4096).expect("memory");
        let pool = Pool::open(&parent.path_for_worker(), 1, 2, 4096, 4 * 4096).expect("pool");
        let mut decoder = pool.allocate(128).expect("first");
        let first = decoder.shared_slot();
        decoder.bytes_mut().fill(37);
        let decoder = coreav_decode::BufferLease::from(decoder);
        let publication = decoder.clone();
        drop(decoder);
        let second = pool.allocate(128).expect("second");
        assert_ne!(second.shared_slot(), first);
        let private = pool.allocate(128).expect("private fallback");
        assert_eq!(private.shared_slot(), None);
        drop(publication);
        let mut reused = pool.allocate(128).expect("reuse");
        assert_eq!(reused.shared_slot(), first);
        assert!(reused.bytes_mut().iter().all(|byte| *byte == 37));
        assert!(pool.allocate(8 * 4096).is_err());
        drop((second, private, reused));
        assert!(pool.allocate(usize::MAX).is_err());
    }

    #[test]
    fn growing_a_released_buffer_keeps_its_shared_slot() {
        let parent = ParentMemory::create(2 * 8192).expect("memory");
        let pool = Pool::open(&parent.path_for_worker(), 1, 1, 8192, 8192).expect("pool");
        let first = pool.allocate(1).expect("small");
        let slot = first.shared_slot();
        drop(first);
        let grown = pool.allocate(8192).expect("grown");
        assert_eq!(grown.shared_slot(), slot);
    }

    #[test]
    fn raw_pipe_writes_pack_many_strided_ranges_and_reject_bad_ranges() {
        use std::io::Read;
        use std::os::fd::AsFd;
        use std::process::{Command, Stdio};
        let parent = ParentMemory::create(2 << 20).expect("memory");
        let mut worker = WorkerMemory::open(&parent.path_for_worker(), 2 << 20).expect("map");
        for (index, byte) in worker.bytes_mut().iter_mut().enumerate() {
            *byte = (index % 251) as u8;
        }
        let ranges: Vec<_> = (0..3000).map(|n| n * 600..n * 600 + 503).collect();
        let expected: Vec<_> = ranges
            .iter()
            .flat_map(|r| worker.bytes_mut().get(r.clone()).expect("range").to_vec())
            .collect();
        let mut child = Command::new("cat")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("cat");
        let input = child.stdin.take().expect("input");
        let mut output = child.stdout.take().expect("output");
        let reader = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            output.read_to_end(&mut bytes).expect("read");
            bytes
        });
        assert!(
            parent
                .write_ranges(input.as_fd(), std::slice::from_ref(&(0..usize::MAX)))
                .is_err()
        );
        parent.write_ranges(input.as_fd(), &ranges).expect("write");
        drop(input);
        assert!(child.wait().expect("wait").success());
        assert_eq!(reader.join().expect("reader"), expected);
    }

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

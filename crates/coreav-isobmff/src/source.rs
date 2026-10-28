//! Random-access byte sources.
//!
//! The low-level parsers in this crate work on byte slices. Probing and
//! packet iteration read through a [`Source`]; this trait moves to a
//! `coreav-io` crate once one exists.

use crate::Error;

/// Largest single read a [`BufferedSource`] issues to its inner source.
pub const MAX_READ: usize = 1 << 20;

/// Positioned reads from a byte source of known length.
pub trait Source {
    /// Total length in bytes.
    fn len(&mut self) -> Result<u64, Error>;

    /// Whether the source is empty.
    fn is_empty(&mut self) -> Result<bool, Error> {
        Ok(self.len()? == 0)
    }

    /// Fills `buf` with the bytes starting at `offset`, or fails with
    /// [`Error::Truncated`] if they are not all available.
    fn read_exact_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), Error>;

    /// Advises that reads within about `len` bytes from `offset`, moving
    /// mostly forward, follow: a writer's copy pass calls it before its
    /// first sample, with the extent of the samples it will copy (or, where
    /// the extent is not known before copying, their total size). A remote
    /// source may start fetching them, so that the reads find their bytes
    /// on the way. Purely advisory: reads stay correct whatever a source
    /// does with it. The default does nothing.
    fn will_read(&mut self, offset: u64, len: u64) {
        let _ = (offset, len);
    }

    /// Advises that the source will not be read for a while: a source may
    /// free what it holds for reads (a [`BufferedSource`] frees its window,
    /// filled again on the next read). Purely advisory: reads stay correct
    /// whatever a source does with it. The default does nothing.
    fn release(&mut self) {}

    /// Appends the `len` bytes starting at `offset` to `buf`, or fails with
    /// [`Error::Truncated`] if they are not all available (`buf` may then
    /// hold part of them). The caller bounds `len`; the default fails with
    /// [`Error::LimitExceeded`] where the memory cannot be reserved. The same bytes as [`Source::read_exact_at`],
    /// for a caller whose buffer is not filled yet: the default appends
    /// `len` zero bytes and reads into them; sources that can read into a
    /// vector's spare capacity override it (a file, bytes in memory, and a
    /// [`BufferedSource`] in front of them), so the buffer is never written
    /// twice.
    fn read_append(&mut self, offset: u64, len: usize, buf: &mut Vec<u8>) -> Result<(), Error> {
        let start = buf.len();
        let end = start.checked_add(len).ok_or(Error::Truncated { offset })?;
        buf.try_reserve_exact(len)
            .map_err(|_| Error::LimitExceeded {
                limit: crate::LimitKind::BoxBytes,
                offset,
            })?;
        buf.resize(end, 0);
        let target = buf.get_mut(start..).ok_or(Error::Truncated { offset })?;
        self.read_exact_at(offset, target)
    }

    /// Advice that the reads that follow touch only `regions`, each an
    /// offset and a length, in increasing order: a scan that reads the
    /// first bytes of many samples. A remote source may fetch the regions
    /// and skip the bytes between them. An empty list ends the advice: the
    /// reads that follow may go anywhere. Purely advisory, like
    /// [`will_read`](Self::will_read); the default does nothing.
    fn will_read_regions(&mut self, regions: &[(u64, u64)]) {
        let _ = regions;
    }

    /// Whether [`will_read_regions`](Self::will_read_regions) is of use:
    /// a remote source that can skip what lies between the regions. A
    /// caller lists regions only then. The default is `false`.
    fn wants_regions(&mut self) -> bool {
        false
    }
}

impl<S: Source + ?Sized> Source for &mut S {
    fn len(&mut self) -> Result<u64, Error> {
        (**self).len()
    }

    fn read_exact_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), Error> {
        (**self).read_exact_at(offset, buf)
    }

    fn will_read(&mut self, offset: u64, len: u64) {
        (**self).will_read(offset, len);
    }

    fn release(&mut self) {
        (**self).release();
    }

    fn read_append(&mut self, offset: u64, len: usize, buf: &mut Vec<u8>) -> Result<(), Error> {
        (**self).read_append(offset, len, buf)
    }

    fn will_read_regions(&mut self, regions: &[(u64, u64)]) {
        (**self).will_read_regions(regions);
    }

    fn wants_regions(&mut self) -> bool {
        (**self).wants_regions()
    }
}

/// A source over bytes already in memory.
#[derive(Clone, Copy, Debug)]
pub struct SliceSource<'a> {
    data: &'a [u8],
}

impl<'a> SliceSource<'a> {
    /// Wraps a byte slice.
    pub fn new(data: &'a [u8]) -> Self {
        Self { data }
    }
}

impl Source for SliceSource<'_> {
    fn len(&mut self) -> Result<u64, Error> {
        Ok(self.data.len() as u64)
    }

    fn read_exact_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), Error> {
        let start = usize::try_from(offset).map_err(|_| Error::Truncated { offset })?;
        let end = start
            .checked_add(buf.len())
            .ok_or(Error::Truncated { offset })?;
        let bytes = self
            .data
            .get(start..end)
            .ok_or(Error::Truncated { offset })?;
        buf.copy_from_slice(bytes);
        Ok(())
    }

    fn read_append(&mut self, offset: u64, len: usize, buf: &mut Vec<u8>) -> Result<(), Error> {
        let start = usize::try_from(offset).map_err(|_| Error::Truncated { offset })?;
        let end = start.checked_add(len).ok_or(Error::Truncated { offset })?;
        let bytes = self
            .data
            .get(start..end)
            .ok_or(Error::Truncated { offset })?;
        buf.extend_from_slice(bytes);
        Ok(())
    }
}

/// A source over a file, using positioned reads.
#[derive(Debug)]
pub struct FileSource {
    file: std::fs::File,
    length: u64,
}

impl FileSource {
    /// Opens a file for reading.
    pub fn open(path: impl AsRef<std::path::Path>) -> Result<Self, Error> {
        Self::from_file(std::fs::File::open(path)?)
    }

    /// A source over a file already open for reading, of its length now:
    /// for a file its owner created and keeps open, read back through the
    /// same handle rather than opened again by path.
    pub fn from_file(file: std::fs::File) -> Result<Self, Error> {
        let length = file.metadata()?.len();
        Ok(Self { file, length })
    }
}

impl Source for FileSource {
    fn len(&mut self) -> Result<u64, Error> {
        Ok(self.length)
    }

    fn read_exact_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), Error> {
        let end = offset
            .checked_add(buf.len() as u64)
            .ok_or(Error::Truncated { offset })?;
        if end > self.length {
            return Err(Error::Truncated { offset });
        }
        read_at(&self.file, offset, buf)
    }

    fn read_append(&mut self, offset: u64, len: usize, buf: &mut Vec<u8>) -> Result<(), Error> {
        use std::io::{Read, Seek, SeekFrom};
        let wanted = u64::try_from(len).map_err(|_| Error::Truncated { offset })?;
        let end = offset
            .checked_add(wanted)
            .ok_or(Error::Truncated { offset })?;
        if end > self.length {
            return Err(Error::Truncated { offset });
        }
        // `read_to_end` reads into the vector's spare capacity, which it
        // does not need to fill first. The file's cursor is not used by the
        // positioned reads.
        let start = buf.len();
        buf.try_reserve_exact(len)
            .map_err(|_| Error::LimitExceeded {
                limit: crate::LimitKind::BoxBytes,
                offset,
            })?;
        let mut file = &self.file;
        file.seek(SeekFrom::Start(offset))?;
        file.take(wanted).read_to_end(buf)?;
        if buf.len().saturating_sub(start) != len {
            return Err(Error::Truncated { offset });
        }
        Ok(())
    }
}

#[cfg(unix)]
fn read_at(file: &std::fs::File, offset: u64, buf: &mut [u8]) -> Result<(), Error> {
    use std::os::unix::fs::FileExt;
    file.read_exact_at(buf, offset).map_err(|error| {
        if error.kind() == std::io::ErrorKind::UnexpectedEof {
            Error::Truncated { offset }
        } else {
            Error::from(error)
        }
    })
}

#[cfg(not(unix))]
fn read_at(mut file: &std::fs::File, offset: u64, buf: &mut [u8]) -> Result<(), Error> {
    use std::io::{Read, Seek, SeekFrom};
    file.seek(SeekFrom::Start(offset))?;
    file.read_exact(buf).map_err(|error| {
        if error.kind() == std::io::ErrorKind::UnexpectedEof {
            Error::Truncated { offset }
        } else {
            Error::from(error)
        }
    })
}

/// A read window in front of another source.
///
/// Small reads are served from one window of at most [`MAX_READ`] bytes. A
/// fill starts at 4 KiB and doubles while reads move forward, so scattered
/// header reads stay small. A read of at least 64 KiB, and a read that ends
/// at the end of the source (where no read-ahead is possible), go straight
/// to the caller's buffer wherever the window does not hold them, so the
/// window never grows because of such reads. No read to the inner source is
/// larger than [`MAX_READ`]; larger requests are split.
///
/// A refill reads in place over bytes earlier fills left in the window's
/// vector, which keeps them until a fill longer than it or
/// [`Source::release`]; only its first `filled` bytes are the window. A
/// fill longer than the vector reads into a new vector's spare capacity
/// ([`Source::read_append`]), so the window is not zeroed before it is read
/// unless the inner source's `read_append` zeroes (`docs/design/
/// read-window.md`).
#[derive(Debug)]
pub struct BufferedSource<S> {
    inner: S,
    /// The bytes earlier fills read; the window is `window[..filled]`, the
    /// file from `start`.
    window: Vec<u8>,
    filled: usize,
    start: u64,
    next_fill: usize,
    largest_read: usize,
}

pub(crate) const FIRST_FILL: usize = 4 << 10;
/// Reads of at least this many bytes that start outside the window bypass
/// it.
const DIRECT_READ: usize = 64 << 10;

impl<S: Source> BufferedSource<S> {
    /// Wraps a source.
    pub fn new(inner: S) -> Self {
        Self {
            inner,
            window: Vec::new(),
            filled: 0,
            start: 0,
            next_fill: FIRST_FILL,
            largest_read: 0,
        }
    }

    /// The largest single read issued to the inner source so far.
    pub fn largest_read(&self) -> usize {
        self.largest_read
    }

    /// Returns the inner source.
    pub fn into_inner(self) -> S {
        self.inner
    }

    fn window_end(&self) -> u64 {
        self.start.saturating_add(self.filled as u64)
    }

    /// The window: the bytes of the file from `start`.
    fn held(&self) -> &[u8] {
        self.window.get(..self.filled).unwrap_or_default()
    }

    fn fill(&mut self, offset: u64, wanted: usize) -> Result<(), Error> {
        let total = self.inner.len()?;
        let sequential = offset == self.window_end() && self.filled > 0;
        self.next_fill = if sequential {
            self.next_fill.saturating_mul(2).min(MAX_READ)
        } else {
            FIRST_FILL
        };
        let available = total.saturating_sub(offset);
        let size = self.next_fill.max(wanted).min(MAX_READ);
        let size = usize::try_from(available).map_or(size, |a| size.min(a));
        // Nothing is held until the read succeeds.
        self.filled = 0;
        self.start = offset;
        self.largest_read = self.largest_read.max(size);
        match self.window.get_mut(..size) {
            // Over bytes an earlier fill read.
            Some(target) => self.inner.read_exact_at(offset, target)?,
            // Longer than the vector: into fresh spare capacity, the old
            // vector freed first.
            None => {
                self.window = Vec::new();
                self.window
                    .try_reserve_exact(size)
                    .map_err(|_| Error::LimitExceeded {
                        limit: crate::LimitKind::BoxBytes,
                        offset,
                    })?;
                self.inner.read_append(offset, size, &mut self.window)?;
            }
        }
        self.filled = size;
        Ok(())
    }
}

impl<S: Source> Source for BufferedSource<S> {
    fn len(&mut self) -> Result<u64, Error> {
        self.inner.len()
    }

    fn will_read(&mut self, offset: u64, len: u64) {
        self.inner.will_read(offset, len);
    }

    fn will_read_regions(&mut self, regions: &[(u64, u64)]) {
        self.inner.will_read_regions(regions);
    }

    fn wants_regions(&mut self) -> bool {
        self.inner.wants_regions()
    }

    fn release(&mut self) {
        self.window = Vec::new();
        self.filled = 0;
        self.next_fill = FIRST_FILL;
        self.inner.release();
    }

    fn read_exact_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), Error> {
        let mut done = 0usize;
        while done < buf.len() {
            let position = offset
                .checked_add(done as u64)
                .ok_or(Error::Truncated { offset })?;
            let left = buf.len().saturating_sub(done);
            if position < self.start || position >= self.window_end() {
                // Reads that run past the end take the window path, which
                // reports them as truncated at the first missing byte.
                let available = self.inner.len()?.saturating_sub(position);
                let to_end = available == left as u64;
                if (buf.len() >= DIRECT_READ || to_end) && available >= left.min(MAX_READ) as u64 {
                    // A large read goes straight to the caller's buffer
                    // where the window does not hold it, even its tail after
                    // a part the window holds: no second copy, and the
                    // window does not grow because of the caller's reads.
                    let step = left.min(MAX_READ);
                    let target = buf
                        .get_mut(done..done.saturating_add(step))
                        .ok_or(Error::Truncated { offset })?;
                    self.inner.read_exact_at(position, target)?;
                    self.largest_read = self.largest_read.max(step);
                    done = done.saturating_add(step);
                    continue;
                }
                self.fill(position, left)?;
                if self.filled == 0 {
                    return Err(Error::Truncated { offset: position });
                }
            }
            let skip = usize::try_from(position.saturating_sub(self.start))
                .map_err(|_| Error::Truncated { offset })?;
            let chunk = self.held().get(skip..).unwrap_or_default();
            let count = chunk.len().min(left);
            let source = chunk.get(..count).unwrap_or_default();
            let target = buf
                .get_mut(done..done.saturating_add(count))
                .ok_or(Error::Truncated { offset })?;
            target.copy_from_slice(source);
            if count == 0 {
                return Err(Error::Truncated { offset: position });
            }
            done = done.saturating_add(count);
        }
        Ok(())
    }

    /// As [`BufferedSource::read_exact_at`], from the same window and with
    /// the same reads to the inner source, appending to `buf`; a large read
    /// goes straight to the inner source's [`Source::read_append`].
    fn read_append(&mut self, offset: u64, len: usize, buf: &mut Vec<u8>) -> Result<(), Error> {
        let mut done = 0usize;
        while done < len {
            let position = offset
                .checked_add(done as u64)
                .ok_or(Error::Truncated { offset })?;
            let left = len.saturating_sub(done);
            if position < self.start || position >= self.window_end() {
                let available = self.inner.len()?.saturating_sub(position);
                let to_end = available == left as u64;
                if (len >= DIRECT_READ || to_end) && available >= left.min(MAX_READ) as u64 {
                    let step = left.min(MAX_READ);
                    self.inner.read_append(position, step, buf)?;
                    self.largest_read = self.largest_read.max(step);
                    done = done.saturating_add(step);
                    continue;
                }
                self.fill(position, left)?;
                if self.filled == 0 {
                    return Err(Error::Truncated { offset: position });
                }
            }
            let skip = usize::try_from(position.saturating_sub(self.start))
                .map_err(|_| Error::Truncated { offset })?;
            let chunk = self.held().get(skip..).unwrap_or_default();
            let count = chunk.len().min(left);
            buf.extend_from_slice(chunk.get(..count).unwrap_or_default());
            if count == 0 {
                return Err(Error::Truncated { offset: position });
            }
            done = done.saturating_add(count);
        }
        Ok(())
    }
}

/// Reads the `len` bytes at `offset`: into `stack` when they fit, as the
/// headers of a frame do (NAL unit lengths and headers, OBU and frame
/// headers), else (a parameter set, an SEI message, a chunk of an Annex B
/// frame) into `heap` with [`Source::read_append`], which a file, bytes in
/// memory and a [`BufferedSource`] fill without zeroing. No header read
/// allocates a buffer of its own: a scan of a file's frames reads tens of
/// thousands of headers (`docs/design/read-window.md`).
pub(crate) fn read_head<'a, S: Source>(
    source: &mut S,
    offset: u64,
    len: u64,
    stack: &'a mut [u8],
    heap: &'a mut Vec<u8>,
) -> Result<&'a [u8], Error> {
    let len = usize::try_from(len).map_err(|_| Error::Overflow { offset })?;
    if let Some(target) = stack.get_mut(..len) {
        source.read_exact_at(offset, target)?;
        return Ok(target);
    }
    heap.clear();
    source.read_append(offset, len, heap)?;
    Ok(heap)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(n: usize) -> Vec<u8> {
        (0..n).map(|i| (i % 251) as u8).collect()
    }

    #[test]
    fn appending_reads_the_same_bytes_as_reading_exactly() {
        let bytes = data(3 << 20);
        let path = std::env::temp_dir().join(format!("coreav-append-{}", std::process::id()));
        std::fs::write(&path, &bytes).unwrap();
        let sources = |kind: usize| -> Box<dyn Source> {
            match kind {
                0 => Box::new(SliceSource::new(&bytes)),
                1 => Box::new(FileSource::open(&path).unwrap()),
                2 => Box::new(BufferedSource::new(SliceSource::new(&bytes))),
                _ => Box::new(BufferedSource::new(FileSource::open(&path).unwrap())),
            }
        };
        // Sequences of (offset, length) on one source: small reads in and
        // out of a window, a read starting inside the window and running
        // past its end, large direct reads, reads to the end of the source
        // and past it.
        let sequences: [&[(u64, usize)]; 6] = [
            &[(10, 16), (20, 5000)],
            &[(10, 16), (2000, 10_000), (100, 50)],
            &[(4000, 70_000), (4100, 100)],
            &[(100_000, 64 << 10), (200_000, 1 << 20)],
            &[(300_000, (1 << 20) + 300_000)],
            &[((3 << 20) - 100, 100), ((3 << 20) - 100, 101), (3 << 20, 0)],
        ];
        for kind in 0..4 {
            for sequence in sequences {
                let (mut exact, mut append) = (sources(kind), sources(kind));
                for &(offset, len) in sequence {
                    let mut expected = vec![0u8; len];
                    let expected_result = exact.read_exact_at(offset, &mut expected);
                    // Something already in the buffer stays in front.
                    let mut appended = vec![7u8, 8];
                    let result = append.read_append(offset, len, &mut appended);
                    assert_eq!(result, expected_result, "{kind} {offset} {len}");
                    if result.is_ok() {
                        assert_eq!(&appended[..2], &[7, 8]);
                        assert_eq!(&appended[2..], &expected[..], "{kind} {offset} {len}");
                    }
                }
            }
        }
        // A buffered source issues the same reads to its inner source.
        for sequence in sequences {
            let mut exact = BufferedSource::new(SliceSource::new(&bytes));
            let mut append = BufferedSource::new(SliceSource::new(&bytes));
            for &(offset, len) in sequence {
                let _ = exact.read_exact_at(offset, &mut vec![0u8; len]);
                let _ = append.read_append(offset, len, &mut Vec::new());
                assert_eq!(append.largest_read(), exact.largest_read());
                assert_eq!((append.start, append.filled), (exact.start, exact.filled));
            }
        }
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn a_released_buffered_source_frees_its_window_and_reads_on() {
        let bytes = data(1 << 20);
        let mut source = BufferedSource::new(SliceSource::new(&bytes));
        let mut small = [0u8; 16];
        source.read_exact_at(10, &mut small).unwrap();
        assert!(source.window.capacity() > 0);
        source.release();
        assert_eq!(source.window.capacity(), 0);
        source.read_exact_at(20, &mut small).unwrap();
        assert_eq!(&small[..], &bytes[20..36]);
    }

    #[test]
    fn large_reads_bypass_the_window() {
        let bytes = data(3 << 20);
        let mut source = BufferedSource::new(SliceSource::new(&bytes));
        let mut small = [0u8; 16];
        source.read_exact_at(10, &mut small).unwrap();
        assert_eq!(&small[..], &bytes[10..26]);
        let window = source.window.capacity();
        // Starts inside the window, continues past it directly.
        let mut large = vec![0u8; 200 << 10];
        source.read_exact_at(20, &mut large).unwrap();
        assert_eq!(large, &bytes[20..20 + (200 << 10)]);
        // Larger than one inner read: split at MAX_READ.
        let mut huge = vec![0u8; (1 << 20) + (300 << 10)];
        source.read_exact_at(5000, &mut huge).unwrap();
        assert_eq!(huge, &bytes[5000..5000 + huge.len()]);
        assert_eq!(source.window.capacity(), window);
        assert_eq!(source.largest_read(), MAX_READ);
    }

    #[test]
    fn large_reads_that_start_in_the_window_do_not_grow_it() {
        let bytes = data(4 << 20);
        let mut source = BufferedSource::new(SliceSource::new(&bytes));
        let mut small = [0u8; 16];
        source.read_exact_at(0, &mut small).unwrap();
        let window = source.window.capacity();
        // Each 64 KiB read starts 100 bytes before the window's end.
        let mut at = (window - 100) as u64;
        let mut buf = vec![0u8; 64 << 10];
        for _ in 0..40 {
            source.read_exact_at(at, &mut buf).unwrap();
            assert_eq!(buf, &bytes[at as usize..at as usize + buf.len()]);
            source
                .read_exact_at(at + buf.len() as u64 - 50, &mut small)
                .unwrap();
            at += buf.len() as u64 - 100 + 50 + 16;
        }
        assert!(
            source.window.capacity() <= 8 << 10,
            "{}",
            source.window.capacity()
        );
    }

    #[test]
    fn reads_past_the_end_are_truncated_at_the_first_missing_byte() {
        let bytes = data(100 << 10);
        for at in [0u64, 1000, 50 << 10] {
            let mut source = BufferedSource::new(SliceSource::new(&bytes));
            let mut buf = vec![0u8; 128 << 10];
            let error = source.read_exact_at(at, &mut buf).unwrap_err();
            assert_eq!(error, Error::Truncated { offset: 100 << 10 });
        }
    }

    /// Bytes no two nearby offsets of which tend to agree, so that a byte
    /// served from the wrong offset shows.
    fn noise(n: usize) -> Vec<u8> {
        let mut state = 0x9e37_79b9_7f4a_7c15u64;
        (0..n)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                (state >> 32) as u8
            })
            .collect()
    }

    /// How an inner source was read.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Call {
        /// `read_exact_at`, and whether the buffer it was given held only
        /// zeroes.
        Exact {
            offset: u64,
            len: usize,
            zeroed: bool,
        },
        Append {
            offset: u64,
            len: usize,
        },
    }

    /// An inner source over `data` that claims to be `claimed` bytes long
    /// (a file cut short after it was opened, when longer than `data`),
    /// writes what it has of a read that runs past `data` and then fails,
    /// and records every read.
    struct Recording<'a> {
        data: &'a [u8],
        claimed: u64,
        calls: Vec<Call>,
    }

    impl<'a> Recording<'a> {
        fn new(data: &'a [u8]) -> Self {
            Self {
                data,
                claimed: data.len() as u64,
                calls: Vec::new(),
            }
        }
    }

    impl Source for Recording<'_> {
        fn len(&mut self) -> Result<u64, Error> {
            Ok(self.claimed)
        }

        fn read_exact_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), Error> {
            self.calls.push(Call::Exact {
                offset,
                len: buf.len(),
                zeroed: buf.iter().all(|b| *b == 0),
            });
            let start = (offset as usize).min(self.data.len());
            let have = &self.data[start..(start + buf.len()).min(self.data.len())];
            buf[..have.len()].copy_from_slice(have);
            if have.len() < buf.len() {
                return Err(Error::Truncated { offset });
            }
            Ok(())
        }

        fn read_append(&mut self, offset: u64, len: usize, buf: &mut Vec<u8>) -> Result<(), Error> {
            self.calls.push(Call::Append { offset, len });
            let start = (offset as usize).min(self.data.len());
            let have = &self.data[start..(start + len).min(self.data.len())];
            buf.extend_from_slice(have);
            if have.len() < len {
                return Err(Error::Truncated { offset });
            }
            Ok(())
        }
    }

    fn read(source: &mut impl Source, offset: u64, len: usize) -> Result<Vec<u8>, Error> {
        let mut buf = vec![0u8; len];
        source.read_exact_at(offset, &mut buf).map(|()| buf)
    }

    #[test]
    fn refills_read_over_earlier_bytes_and_grow_by_appending() {
        let bytes = noise(1 << 20);
        let mut source = BufferedSource::new(Recording::new(&bytes));
        // Forward reads that each start at the window's end double it:
        // 4, 8 and 16 KiB, each larger than any fill before.
        let mut at = 0u64;
        for _ in 0..3 {
            assert_eq!(
                read(&mut source, at, 16).unwrap(),
                &bytes[at as usize..][..16]
            );
            at = source.window_end();
        }
        // A jump forward: a 4 KiB fill, over bytes the first fills read.
        assert_eq!(
            read(&mut source, 500_000, 16).unwrap(),
            &bytes[500_000..][..16]
        );
        // Backwards: another 4 KiB fill in place.
        assert_eq!(read(&mut source, 100, 16).unwrap(), &bytes[100..116]);
        assert_eq!(
            source.inner.calls,
            [
                Call::Append {
                    offset: 0,
                    len: 4 << 10
                },
                Call::Append {
                    offset: 4 << 10,
                    len: 8 << 10
                },
                Call::Append {
                    offset: 12 << 10,
                    len: 16 << 10
                },
                Call::Exact {
                    offset: 500_000,
                    len: 4 << 10,
                    zeroed: false
                },
                Call::Exact {
                    offset: 100,
                    len: 4 << 10,
                    zeroed: false
                },
            ]
        );
        assert_eq!(source.window.len(), 16 << 10);
    }

    #[test]
    fn a_short_refill_serves_none_of_the_bytes_after_it() {
        let bytes = noise(1 << 20);
        let mut source = BufferedSource::new(SliceSource::new(&bytes));
        // Grow the window to 64 KiB at 200,000.
        let mut at = 200_000u64;
        while source.filled < 64 << 10 {
            read(&mut source, at, 16).unwrap();
            at = source.window_end();
        }
        let start = source.start;
        // Back before it: a 4 KiB fill over the first 4 KiB of the vector,
        // which still holds the rest of the 64 KiB from `start`.
        assert_eq!(read(&mut source, 1000, 16).unwrap(), &bytes[1000..1016]);
        assert_eq!((source.start, source.filled), (1000, 4 << 10));
        assert!(source.window.len() >= 64 << 10);
        // Every read across and past the fill's end is the file's bytes,
        // never what the vector held from `start`.
        for (offset, len) in [(1000 + 4090, 20), (1000 + 4096, 20), (5000, 30_000)] {
            assert_eq!(
                read(&mut source, offset, len).unwrap(),
                &bytes[offset as usize..][..len]
            );
        }
        assert_ne!(start, source.start);
    }

    #[test]
    fn a_fill_cut_by_the_end_holds_only_the_bytes_before_it() {
        let bytes = noise(300_000);
        let mut source = BufferedSource::new(SliceSource::new(&bytes));
        let mut at = 0u64;
        while source.filled < 128 << 10 {
            read(&mut source, at, 16).unwrap();
            at = source.window_end();
        }
        // 1,000 bytes before the end: the fill stops there, in a vector
        // that holds 128 KiB.
        let near = bytes.len() as u64 - 1000;
        assert_eq!(
            read(&mut source, near, 10).unwrap(),
            &bytes[near as usize..][..10]
        );
        assert_eq!(source.filled, 1000);
        assert_eq!(
            read(&mut source, near, 1001),
            Err(Error::Truncated {
                offset: bytes.len() as u64
            })
        );
        assert_eq!(
            read(&mut source, bytes.len() as u64 - 5, 10),
            Err(Error::Truncated {
                offset: bytes.len() as u64
            })
        );
        let mut appended = Vec::new();
        assert_eq!(
            source.read_append(near + 990, 20, &mut appended),
            Err(Error::Truncated {
                offset: bytes.len() as u64
            })
        );
        assert_eq!(appended, &bytes[near as usize + 990..]);
    }

    #[test]
    fn a_failed_fill_holds_nothing() {
        // The source claims 200,000 bytes and has 150,000: a fill that
        // reaches past them writes part and fails.
        let bytes = noise(200_000);
        let mut inner = Recording::new(&bytes[..150_000]);
        inner.claimed = 200_000;
        let mut source = BufferedSource::new(inner);
        // A large window first, so that a failed in-place fill writes over
        // bytes the vector held.
        let mut at = 0u64;
        while source.filled < 32 << 10 {
            read(&mut source, at, 16).unwrap();
            at = source.window_end();
        }
        for offset in [148_000u64, 149_990] {
            // The fill of 4 KiB runs past the 150,000 bytes, even where the
            // read itself would not.
            assert_eq!(
                read(&mut source, offset, 16),
                Err(Error::Truncated { offset })
            );
            assert_eq!(source.filled, 0);
            // The same read again fails again, rather than finding the
            // bytes the failed fill wrote.
            assert_eq!(
                read(&mut source, offset, 1),
                Err(Error::Truncated { offset })
            );
        }
        // Reads the source can serve go on.
        assert_eq!(
            read(&mut source, 100_000, 16).unwrap(),
            &bytes[100_000..100_016]
        );
        // A failed fill larger than any before (by appending) also holds
        // nothing.
        let mut source = BufferedSource::new(Recording {
            data: &bytes[..1000],
            claimed: 200_000,
            calls: Vec::new(),
        });
        assert_eq!(
            read(&mut source, 10, 16),
            Err(Error::Truncated { offset: 10 })
        );
        assert_eq!(source.filled, 0);
        assert_eq!(
            read(&mut source, 10, 16),
            Err(Error::Truncated { offset: 10 })
        );
    }

    #[test]
    fn an_in_place_fill_after_a_failed_growth_serves_only_its_bytes() {
        // The source claims 300,000 bytes and has 100,000. A 32 KiB fill
        // 10,000 bytes before the end appends part of its bytes and fails,
        // leaving a vector shorter than the fill; the 4 KiB fill that
        // follows reads in place over what that failed fill wrote.
        let bytes = noise(300_000);
        let mut source = BufferedSource::new(Recording {
            data: &bytes[..100_000],
            claimed: 300_000,
            calls: Vec::new(),
        });
        assert_eq!(
            read(&mut source, 90_000, 32 << 10),
            Err(Error::Truncated { offset: 90_000 })
        );
        assert_eq!(source.filled, 0);
        assert_eq!(source.window.len(), 10_000);
        // Back to the start: 4 KiB in place, then reads around its end.
        assert_eq!(read(&mut source, 0, 16).unwrap(), &bytes[..16]);
        assert_eq!((source.start, source.filled), (0, 4 << 10));
        assert_eq!(
            source.inner.calls.last(),
            Some(&Call::Exact {
                offset: 0,
                len: 4 << 10,
                zeroed: false
            })
        );
        for (offset, len) in [(4090u64, 20usize), (4096, 5000), (50, 9000)] {
            assert_eq!(
                read(&mut source, offset, len).unwrap(),
                &bytes[offset as usize..][..len]
            );
        }
    }

    #[test]
    fn random_reads_return_the_bytes_of_the_source() {
        let bytes = noise(3 << 20);
        let mut state = 1u64;
        let mut next = |bound: u64| {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (state >> 33) % bound
        };
        for round in 0..20 {
            let mut buffered = BufferedSource::new(SliceSource::new(&bytes));
            let mut plain = SliceSource::new(&bytes);
            let mut at = 0u64;
            for _ in 0..500 {
                // Mostly small reads moving forward, some jumps either way,
                // some large reads, some past the end.
                at = match next(10) {
                    0 => next(bytes.len() as u64 + 100),
                    1 => at.saturating_sub(next(200_000)),
                    _ => at + next(5000),
                };
                // Not empty: an empty read succeeds wherever it starts.
                let len = 1 + match next(20) {
                    0 => next(300 << 10) as usize,
                    1 => next(2 << 20) as usize,
                    _ => next(600) as usize,
                };
                // A buffered source names the first missing byte.
                let missing = Error::Truncated {
                    offset: at.max(bytes.len() as u64),
                };
                let (mut ours, mut theirs) = (vec![0u8; len], vec![0u8; len]);
                let result = buffered.read_exact_at(at, &mut ours);
                match plain.read_exact_at(at, &mut theirs) {
                    Ok(()) => assert_eq!((result, ours), (Ok(()), theirs), "{round} {at} {len}"),
                    Err(_) => assert_eq!(result, Err(missing.clone()), "{round} {at} {len}"),
                }
                let (mut ours, mut theirs) = (vec![1u8], vec![1u8]);
                let result = buffered.read_append(at, len, &mut ours);
                match plain.read_append(at, len, &mut theirs) {
                    Ok(()) => assert_eq!((result, ours), (Ok(()), theirs), "{round} {at} {len}"),
                    Err(_) => assert_eq!(result, Err(missing.clone()), "{round} {at} {len}"),
                }
            }
        }
    }
}

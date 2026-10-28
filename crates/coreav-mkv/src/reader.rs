//! Element reads through a [`Source`]: the I/O shell around the sans-IO
//! parsers.

use crate::ebml::{ElementHeader, MAX_HEADER_LEN, Parsed, parse_element_header};
use crate::error::LimitKind;
use crate::{Error, Source};
use core::cell::Cell;

/// Reads the element header at `at`; `end` bounds the enclosing element or
/// the file. A header that does not fit before `end` is truncated.
pub(crate) fn read_header<S: Source>(
    source: &mut S,
    at: u64,
    end: u64,
    max_size_len: u8,
) -> Result<ElementHeader, Error> {
    let room = end.checked_sub(at).ok_or(Error::Truncated { offset: at })?;
    let mut buf = [0u8; MAX_HEADER_LEN];
    let n = usize::try_from(room.min(MAX_HEADER_LEN as u64)).unwrap_or(MAX_HEADER_LEN);
    let slice = buf.get_mut(..n).ok_or(Error::Truncated { offset: at })?;
    source.read_exact_at(at, slice)?;
    match parse_element_header(slice, at, max_size_len)? {
        Parsed::Done(header) => Ok(header),
        Parsed::NeedMore(_) => Err(Error::Truncated { offset: at }),
    }
}

/// Reads the data of a known-sized element into memory, at most `max`
/// bytes (`limit` names the limit that `max` comes from).
pub(crate) fn read_data<S: Source>(
    source: &mut S,
    header: &ElementHeader,
    max: u64,
    limit: LimitKind,
) -> Result<Vec<u8>, Error> {
    let size = header.size.ok_or(Error::InvalidElement {
        offset: header.offset,
        id: header.id,
        reason: "unknown size not allowed for this element",
    })?;
    if size > max {
        return Err(Error::LimitExceeded {
            limit,
            offset: header.offset,
        });
    }
    let len = usize::try_from(size).map_err(|_| Error::LimitExceeded {
        limit,
        offset: header.offset,
    })?;
    let mut data = Vec::new();
    data.try_reserve_exact(len)
        .map_err(|_| Error::LimitExceeded {
            limit,
            offset: header.offset,
        })?;
    // Read into the reserved capacity, without zeroing it first.
    source.read_append(header.data_offset(), len, &mut data)?;
    Ok(data)
}

/// Counts element headers against `max_elements`. Shared by nested child
/// iterations, hence the cell.
#[derive(Debug)]
pub(crate) struct Counter {
    used: Cell<u32>,
    max: u32,
}

impl Counter {
    pub(crate) fn new(max: u32) -> Self {
        Self {
            used: Cell::new(0),
            max,
        }
    }

    pub(crate) fn count(&self, offset: u64) -> Result<(), Error> {
        let used = self.used.get();
        if used >= self.max {
            return Err(Error::LimitExceeded {
                limit: LimitKind::Elements,
                offset,
            });
        }
        self.used.set(used + 1);
        Ok(())
    }
}

/// The child elements of a master element held in memory. Children must
/// have a known size that fits inside the parent (RFC 8794 6.2 allows
/// unknown sizes only where the schema says so; none of the masters read
/// into memory allow it).
pub(crate) struct Children<'a, 'c> {
    data: &'a [u8],
    base: u64,
    max_size_len: u8,
    counter: &'c Counter,
}

impl<'a, 'c> Children<'a, 'c> {
    /// `data` is the parent's data, which starts at absolute offset `base`.
    pub(crate) fn new(data: &'a [u8], base: u64, max_size_len: u8, counter: &'c Counter) -> Self {
        Self {
            data,
            base,
            max_size_len,
            counter,
        }
    }
}

impl<'a> Iterator for Children<'a, '_> {
    type Item = Result<(ElementHeader, &'a [u8]), Error>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.data.is_empty() {
            return None;
        }
        let result = self.step();
        if result.is_err() {
            self.data = &[];
        }
        Some(result)
    }
}

impl<'a> Children<'a, '_> {
    fn step(&mut self) -> Result<(ElementHeader, &'a [u8]), Error> {
        let offset = self.base;
        self.counter.count(offset)?;
        let past_parent = |id| Error::InvalidElement {
            offset,
            id,
            reason: "element runs past its parent",
        };
        let header = match parse_element_header(self.data, offset, self.max_size_len)? {
            Parsed::Done(header) => header,
            Parsed::NeedMore(_) => return Err(past_parent(0)),
        };
        let size = header.size.ok_or(Error::InvalidElement {
            offset,
            id: header.id,
            reason: "unknown size not allowed for this element",
        })?;
        let start = usize::from(header.header_len);
        let end = usize::try_from(size)
            .ok()
            .and_then(|size| start.checked_add(size))
            .ok_or(past_parent(header.id))?;
        let data = self.data.get(start..end).ok_or(past_parent(header.id))?;
        self.data = self.data.get(end..).unwrap_or_default();
        self.base = header.end().ok_or(Error::Overflow { offset })?;
        Ok((header, data))
    }
}

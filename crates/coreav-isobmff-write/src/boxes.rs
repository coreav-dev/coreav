//! Box serialization (ISO/IEC 14496-12 4.2).
//!
//! A box is built as a small tree whose leaves hold serialized payloads, so
//! the size of every box is known before its header is written and the tree
//! can be streamed to a writer that cannot seek.

use crate::tables::Blocks;
use std::borrow::Cow;
use std::io::{self, Read, Write};

/// Largest box size a 32-bit size field holds.
const COMPACT_MAX: u64 = u32::MAX as u64;

/// The large part of a leaf's payload.
pub(crate) enum Body<'a> {
    /// Contiguous bytes.
    Bytes(Cow<'a, [u8]>),
    /// A table kept in blocks, written block by block.
    Blocks(&'a Blocks),
    /// This many zero bytes, never allocated: a table's size when only the
    /// box sizes are needed.
    Zeros(u64),
}

impl Body<'_> {
    fn len(&self) -> u64 {
        match self {
            Self::Bytes(bytes) => len64(bytes),
            Self::Blocks(blocks) => u64::try_from(blocks.len()).unwrap_or(u64::MAX),
            Self::Zeros(len) => *len,
        }
    }

    fn write<W: Write>(&self, out: &mut W) -> io::Result<()> {
        match self {
            Self::Bytes(bytes) => out.write_all(bytes),
            Self::Blocks(blocks) => blocks.slices().try_for_each(|slice| out.write_all(slice)),
            Self::Zeros(len) => io::copy(&mut io::repeat(0).take(*len), out).map(|_| ()),
        }
    }
}

/// One box.
pub(crate) enum Node<'a> {
    /// A box whose payload is `head` followed by `body`. The split lets a
    /// large table be prefixed by its counts without copying it.
    Leaf {
        kind: [u8; 4],
        head: Vec<u8>,
        body: Body<'a>,
    },
    /// A box that holds other boxes.
    Parent {
        kind: [u8; 4],
        children: Vec<Node<'a>>,
    },
}

impl<'a> Node<'a> {
    /// A box with a payload.
    pub(crate) fn leaf(kind: &[u8; 4], payload: Vec<u8>) -> Self {
        Self::Leaf {
            kind: *kind,
            head: payload,
            body: Body::Bytes(Cow::Borrowed(&[])),
        }
    }

    /// A full box (version and flags, 14496-12 4.2.2) with a payload.
    pub(crate) fn full(kind: &[u8; 4], version: u8, flags: u32, payload: &[u8]) -> Self {
        let mut head = full_header(version, flags);
        head.extend_from_slice(payload);
        Self::leaf(kind, head)
    }

    /// A full box whose payload is `head` (after version and flags) followed
    /// by `body`.
    pub(crate) fn full_split(
        kind: &[u8; 4],
        version: u8,
        flags: u32,
        head: &[u8],
        body: Body<'a>,
    ) -> Self {
        let mut prefix = full_header(version, flags);
        prefix.extend_from_slice(head);
        Self::Leaf {
            kind: *kind,
            head: prefix,
            body,
        }
    }

    /// A container box.
    pub(crate) fn parent(kind: &[u8; 4], children: Vec<Node<'a>>) -> Self {
        Self::Parent {
            kind: *kind,
            children,
        }
    }

    /// Bytes of the payload.
    fn content_len(&self) -> u64 {
        match self {
            Self::Leaf { head, body, .. } => len64(head).saturating_add(body.len()),
            Self::Parent { children, .. } => children
                .iter()
                .fold(0u64, |sum, child| sum.saturating_add(child.len())),
        }
    }

    /// Bytes of the whole box, header included.
    pub(crate) fn len(&self) -> u64 {
        let content = self.content_len();
        let compact = content.saturating_add(8);
        if compact <= COMPACT_MAX {
            compact
        } else {
            content.saturating_add(16)
        }
    }

    /// Writes the box.
    pub(crate) fn write<W: Write>(&self, out: &mut W) -> io::Result<()> {
        let (kind, content) = match self {
            Self::Leaf { kind, .. } | Self::Parent { kind, .. } => (kind, self.content_len()),
        };
        write_header(out, kind, content)?;
        match self {
            Self::Leaf { head, body, .. } => {
                out.write_all(head)?;
                body.write(out)
            }
            Self::Parent { children, .. } => {
                for child in children {
                    child.write(out)?;
                }
                Ok(())
            }
        }
    }
}

/// Length of a header for `content` payload bytes.
pub(crate) fn header_len(content: u64) -> u64 {
    if content.saturating_add(8) <= COMPACT_MAX {
        8
    } else {
        16
    }
}

/// Writes a box header for `content` payload bytes, with a 64-bit size when
/// a 32-bit one does not fit (14496-12 4.2.2, `size == 1`).
pub(crate) fn write_header<W: Write>(out: &mut W, kind: &[u8; 4], content: u64) -> io::Result<()> {
    let compact = content.saturating_add(8);
    if let Ok(size) = u32::try_from(compact) {
        out.write_all(&size.to_be_bytes())?;
        out.write_all(kind)
    } else {
        out.write_all(&1u32.to_be_bytes())?;
        out.write_all(kind)?;
        out.write_all(&content.saturating_add(16).to_be_bytes())
    }
}

fn full_header(version: u8, flags: u32) -> Vec<u8> {
    let mut head = Vec::with_capacity(16);
    head.push(version);
    head.extend_from_slice(flags.to_be_bytes().get(1..).unwrap_or(&[0, 0, 0]));
    head
}

fn len64(bytes: &[u8]) -> u64 {
    u64::try_from(bytes.len()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_include_headers() {
        let tree = Node::parent(
            b"moov",
            vec![
                Node::full(b"mvhd", 0, 0, &[1, 2, 3]),
                Node::full_split(b"stsz", 0, 0, &[9], Body::Bytes(Cow::Owned(vec![7; 5]))),
            ],
        );
        let mut out = Vec::new();
        tree.write(&mut out).unwrap();
        assert_eq!(tree.len(), out.len() as u64);
        assert_eq!(&out[..8], &[0, 0, 0, 41, b'm', b'o', b'o', b'v']);
        assert_eq!(
            &out[8..20],
            &[0, 0, 0, 15, b'm', b'v', b'h', b'd', 0, 0, 0, 0]
        );
        assert_eq!(
            &out[23..],
            &[
                0, 0, 0, 18, b's', b't', b's', b'z', 0, 0, 0, 0, 9, 7, 7, 7, 7, 7
            ]
        );
    }

    #[test]
    fn large_boxes_use_a_64_bit_size() {
        let mut out = Vec::new();
        write_header(&mut out, b"mdat", u64::from(u32::MAX)).unwrap();
        assert_eq!(out.len() as u64, header_len(u64::from(u32::MAX)));
        assert_eq!(&out[..8], &[0, 0, 0, 1, b'm', b'd', b'a', b't']);
        assert_eq!(
            u64::from_be_bytes(out[8..].try_into().unwrap()),
            u64::from(u32::MAX) + 16
        );
        let mut small = Vec::new();
        write_header(&mut small, b"mdat", u64::from(u32::MAX) - 8).unwrap();
        assert_eq!(small, [255, 255, 255, 255, b'm', b'd', b'a', b't']);
        assert_eq!(header_len(u64::from(u32::MAX) - 8), 8);
    }
}

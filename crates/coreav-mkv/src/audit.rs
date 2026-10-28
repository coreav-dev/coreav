//! The structure audit of the block scan (`docs/design/policy-matroska.md`,
//! decision 8): a walk over every element header of the Segment, up to the
//! end of the file, that fails on anything after which a reader might find
//! frames the packet listing skips.
//!
//! The audit is a cursor that the packet listing drives
//! ([`crate::Packets`], when the scan asks for it): before the listing reads
//! an element, the audit walks up to and past the same element, so both
//! read the same bytes at about the same time and a file is read once
//! rather than once per walk (#135). The two walks stay independent: each
//! has its own work count, and the audit's verdict does not depend on the
//! listing's. When the listing stops early (an error, or the end of the
//! Segment), the audit walks on alone to the end of the file, and its
//! error, wherever in the file, takes precedence over the listing's, as
//! when it ran first.

use crate::ebml::{self, ElementHeader};
use crate::ids;
use crate::reader::read_header;
use crate::schema::{self, Kind};
use crate::segment::Segment;
use crate::{Error, LimitKind, Limits, Source};

/// Work counted against `max_block_work` by the structure audit, and the
/// sizes it allows.
struct Work {
    used: u64,
    max: u64,
    /// Most octets of a string or binary other than a frame or an attached
    /// file: `max_element_bytes`.
    max_element: u64,
    /// Most octets of a block or a block addition: `max_packet_bytes`.
    max_frame: u64,
}

/// Most octets of an attached file (`FileData`) the audit accepts: large
/// fonts fit, and it stays well below the sizes at which demuxers refuse a
/// binary element and resynchronize.
const MAX_ATTACHED_FILE: u64 = 64 << 20;

/// `FileData` and `BlockAdditional`.
const FILE_DATA: u32 = 0x465C;
const BLOCK_ADDITIONAL: u32 = 0xA5;

/// Master elements nested deeper than this in the Segment fail the audit;
/// the Matroska schema nests at most eight levels below a Top-Level
/// Element.
const MAX_AUDIT_DEPTH: u32 = 12;

impl Work {
    fn charge(&mut self, offset: u64) -> Result<(), Error> {
        if self.used >= self.max {
            return Err(Error::LimitExceeded {
                limit: LimitKind::BlockWork,
                offset,
            });
        }
        self.used += 1;
        Ok(())
    }
}

/// Where the walk ends, from the file's length.
#[derive(Clone, Copy)]
struct Bounds {
    len: u64,
    /// The Segment's declared end, if its size is known.
    declared_end: Option<u64>,
    /// Where the walk stops: the end of the file, or the declared end if it
    /// is past it (a truncated file).
    end: u64,
    /// Where the Segment's children must end: the declared end or the end
    /// of the file, whichever comes first.
    limit: u64,
}

/// The Cluster the audit is in.
#[derive(Clone, Copy)]
struct Cluster {
    /// Its end, `None` for an unknown size.
    end: Option<u64>,
    /// Where its children stop: its end, or the Segment's limit.
    stop: u64,
}

/// The BlockGroup the audit is in.
#[derive(Clone, Copy)]
struct Group {
    end: u64,
    /// Blocks met so far.
    blocks: u32,
}

/// The audit's cursor: [`Audit::through`] walks it forward.
pub(crate) struct Audit {
    work: Work,
    bounds: Option<Bounds>,
    infos: u32,
    tracks: u32,
    at: u64,
    cluster: Option<Cluster>,
    group: Option<Group>,
    /// The walk reached the end, or failed.
    done: bool,
}

impl Audit {
    pub(crate) fn new(segment: &Segment, limits: &Limits) -> Self {
        Self {
            work: Work {
                used: 0,
                max: limits.max_block_work,
                max_element: limits.max_element_bytes,
                max_frame: limits.max_packet_bytes,
            },
            bounds: None,
            infos: 0,
            tracks: 0,
            at: segment.data_offset,
            cluster: None,
            group: None,
            done: false,
        }
    }

    /// Walks the audit until it is past the element that starts at `at`
    /// (or, with `u64::MAX`, to the end of the file). After an error, the
    /// walk is over and later calls do nothing.
    pub(crate) fn through<S: Source>(
        &mut self,
        segment: &Segment,
        source: &mut S,
        at: u64,
    ) -> Result<(), Error> {
        while !self.done && self.at <= at {
            if let Err(error) = self.step(segment, source) {
                self.done = true;
                return Err(error);
            }
        }
        Ok(())
    }

    /// Audits one element: a child of the BlockGroup or the Cluster the
    /// audit is in, or a child of the Segment (for a Cluster or a
    /// BlockGroup, its header). Each step moves the cursor forward or out
    /// of a BlockGroup or a Cluster.
    fn step<S: Source>(&mut self, segment: &Segment, source: &mut S) -> Result<(), Error> {
        let bounds = match self.bounds {
            Some(bounds) => bounds,
            None => {
                let len = source.len()?;
                let declared_end = segment
                    .size
                    .map(|size| segment.data_offset.saturating_add(size));
                let bounds = Bounds {
                    len,
                    declared_end,
                    end: declared_end.map_or(len, |declared| declared.max(len)),
                    limit: declared_end.unwrap_or(len).min(len),
                };
                self.bounds = Some(bounds);
                bounds
            }
        };
        if let Some(group) = self.group {
            return self.group_child(segment, source, group);
        }
        if let Some(cluster) = self.cluster {
            return self.cluster_child(segment, source, cluster);
        }
        if self.at >= bounds.end {
            self.done = true;
            return Ok(());
        }
        self.work.charge(self.at)?;
        let h = read_header(source, self.at, bounds.len, segment.max_size_len)?;
        if bounds
            .declared_end
            .is_some_and(|declared| self.at >= declared)
        {
            return Err(unverifiable(&h, "data after the Segment's declared end"));
        }
        let limit = bounds.limit;
        match h.id {
            ids::CLUSTER => {
                let end = match h.end() {
                    Some(end) if end > limit => {
                        return Err(unverifiable(
                            &h,
                            "a Cluster runs past the Segment or the file",
                        ));
                    }
                    end => end,
                };
                self.cluster = Some(Cluster {
                    end,
                    stop: end.unwrap_or(limit),
                });
                self.at = h.data_offset();
            }
            ids::INFO
            | ids::TRACKS
            | ids::SEEK_HEAD
            | ids::CUES
            | ids::ATTACHMENTS
            | ids::CHAPTERS
            | ids::TAGS
            | ids::VOID
            | ids::CRC32 => {
                let count = match h.id {
                    ids::INFO => Some(&mut self.infos),
                    ids::TRACKS => Some(&mut self.tracks),
                    _ => None,
                };
                if let Some(count) = count {
                    *count += 1;
                    if *count > 1 {
                        return Err(unverifiable(&h, "a second Info or Tracks element"));
                    }
                }
                let end = known_end(&h, limit)?;
                audit_element(segment, source, &h, end, 0, &mut self.work)?;
                self.at = end;
            }
            ids::EBML => return Err(unverifiable(&h, "an EBML header inside the Segment")),
            _ => return Err(unverifiable(&h, "an unknown element in the Segment")),
        }
        Ok(())
    }

    /// Audits one child of a Cluster, or leaves the Cluster where it ends
    /// (for an unknown size, at the first element that cannot be its
    /// child).
    fn cluster_child<S: Source>(
        &mut self,
        segment: &Segment,
        source: &mut S,
        cluster: Cluster,
    ) -> Result<(), Error> {
        let stop = cluster.stop;
        if self.at >= stop {
            self.cluster = None;
            self.at = stop;
            return Ok(());
        }
        self.work.charge(self.at)?;
        let h = read_header(source, self.at, stop, segment.max_size_len)?;
        if cluster.end.is_none() && (h.id == ids::EBML || ids::TOP_LEVEL.contains(&h.id)) {
            // An element that cannot be a child ends an unknown-sized
            // Cluster (RFC 8794 6.2).
            self.cluster = None;
            return Ok(());
        }
        let child_end = known_end(&h, stop)?;
        match h.id {
            ids::SIMPLE_BLOCK => audit_block(segment, source, &h, &self.work)?,
            ids::BLOCK_GROUP => {
                // Its children are audited one step each, as the listing
                // reads them.
                self.group = Some(Group {
                    end: child_end,
                    blocks: 0,
                });
                self.at = h.data_offset();
                return Ok(());
            }
            _ => audit_element(segment, source, &h, child_end, 1, &mut self.work)?,
        }
        self.at = child_end;
        Ok(())
    }

    /// Audits one child of a BlockGroup, or leaves the BlockGroup where it
    /// ends.
    fn group_child<S: Source>(
        &mut self,
        segment: &Segment,
        source: &mut S,
        group: Group,
    ) -> Result<(), Error> {
        if self.at >= group.end {
            self.group = None;
            self.at = group.end;
            return Ok(());
        }
        self.work.charge(self.at)?;
        let g = read_header(source, self.at, group.end, segment.max_size_len)?;
        let g_end = known_end(&g, group.end)?;
        if g.id == ids::BLOCK {
            let blocks = group.blocks + 1;
            if blocks > 1 {
                return Err(unverifiable(&g, "a BlockGroup with a second Block"));
            }
            self.group = Some(Group { blocks, ..group });
            audit_block(segment, source, &g, &self.work)?;
        } else {
            audit_element(segment, source, &g, g_end, 2, &mut self.work)?;
        }
        self.at = g_end;
        Ok(())
    }
}

/// An element the audit cannot account for: the listing might miss frames
/// a reader finds.
fn unverifiable(h: &ElementHeader, reason: &'static str) -> Error {
    Error::InvalidElement {
        offset: h.offset,
        id: h.id,
        reason,
    }
}

/// Checks one element's length against its kind in the Matroska schema
/// and, for a master element, every element below it: a length the type
/// does not allow (an integer of 9 octets, a 4-octet `SeekID` of 5) is
/// where demuxers resynchronize, finding elements the listing would not
/// (decision 8). Block payloads are not read.
fn audit_element<S: Source>(
    segment: &Segment,
    source: &mut S,
    h: &ElementHeader,
    end: u64,
    depth: u32,
    work: &mut Work,
) -> Result<(), Error> {
    let Some(kind) = schema::kind(h.id) else {
        return Ok(());
    };
    let len = end.saturating_sub(h.data_offset());
    if !kind.allows(len) {
        return Err(unverifiable(
            h,
            "an element whose length its type does not allow",
        ));
    }
    // RFC 8794 bounds no string or binary, but demuxers refuse long ones
    // and resynchronize inside them; the audit bounds them well below.
    let most = match (kind, h.id) {
        (Kind::Binary, FILE_DATA) => Some(MAX_ATTACHED_FILE),
        (Kind::Binary, BLOCK_ADDITIONAL | ids::SIMPLE_BLOCK | ids::BLOCK) => Some(work.max_frame),
        (Kind::Text | Kind::Binary, _) => Some(work.max_element),
        _ => None,
    };
    if most.is_some_and(|most| len > most) {
        return Err(unverifiable(
            h,
            "a string or binary longer than the audit allows",
        ));
    }
    if kind != Kind::Master {
        return Ok(());
    }
    if depth >= MAX_AUDIT_DEPTH {
        return Err(unverifiable(h, "elements nested too deep"));
    }
    let mut at = h.data_offset();
    while at < end {
        work.charge(at)?;
        let child = read_header(source, at, end, segment.max_size_len)?;
        let child_end = known_end(&child, end)?;
        audit_element(segment, source, &child, child_end, depth + 1, work)?;
        at = child_end;
    }
    Ok(())
}

/// Checks that a SimpleBlock or Block names a defined track and is no
/// longer than `max_packet_bytes`.
fn audit_block<S: Source>(
    segment: &Segment,
    source: &mut S,
    h: &ElementHeader,
    work: &Work,
) -> Result<(), Error> {
    if h.size.is_some_and(|size| size > work.max_frame) {
        return Err(unverifiable(
            h,
            "a string or binary longer than the audit allows",
        ));
    }
    let mut head = [0u8; 8];
    let size = h.size.unwrap_or(0).min(8);
    let head = head
        .get_mut(..usize::try_from(size).unwrap_or(0))
        .unwrap_or_default();
    source.read_exact_at(h.data_offset(), head)?;
    let number = ebml::read_vint(head).map(|(value, _, _)| value);
    let defined = number.is_some_and(|number| {
        segment.tracks.iter().any(|t| t.number == number)
            || segment.dropped.iter().any(|t| t.number == number)
    });
    if defined {
        Ok(())
    } else {
        Err(unverifiable(h, "a block of an undefined track"))
    }
}

/// Where an element of known size ends, if it ends within `limit`.
fn known_end(h: &ElementHeader, limit: u64) -> Result<u64, Error> {
    match h.end() {
        Some(end) if end <= limit => Ok(end),
        Some(_) => Err(Error::Truncated { offset: h.offset }),
        None => Err(unverifiable(h, "an element of unknown size")),
    }
}

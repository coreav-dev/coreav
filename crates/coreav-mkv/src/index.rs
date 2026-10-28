//! The seek index: `Cues` when the segment has them (RFC 9559 22), a
//! cluster scan otherwise.

use crate::ebml::{self, ElementHeader};
use crate::error::{LimitKind, Warning, warn};
use crate::ids;
use crate::reader::{Children, Counter, read_data, read_header};
use crate::segment::Segment;
use crate::{Error, Limits, Source};

/// Where an index came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndexSource {
    /// The segment's `Cues` element.
    Cues,
    /// A scan of every Cluster's header and `Timestamp`.
    ClusterScan,
}

/// One seek point: a time and the Cluster to start reading from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SeekPoint {
    /// Time in Segment Ticks: `CueTime`, or the Cluster `Timestamp`.
    pub time: u64,
    /// `CueTrack`; `None` for a cluster scan, whose points apply to every
    /// track.
    pub track: Option<u64>,
    /// Absolute offset of the Cluster.
    pub cluster: u64,
    /// `CueRelativePosition`: offset of the block inside the Cluster's data.
    pub relative: Option<u64>,
}

/// A seek index, in storage order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Index {
    /// Where the points come from.
    pub source: IndexSource,
    /// The points.
    pub points: Vec<SeekPoint>,
    /// Problems met while building the index.
    pub warnings: Vec<Warning>,
}

impl Index {
    /// The Cluster to read from to reach `time` (Segment Ticks) on `track`
    /// (`None`: any track): the latest point at or before `time`, else the
    /// first point.
    pub fn cluster_for(&self, track: Option<u64>, time: u64) -> Option<u64> {
        let matching = || {
            self.points
                .iter()
                .filter(move |p| track.is_none() || p.track.is_none() || p.track == track)
        };
        matching()
            .filter(|p| p.time <= time)
            .max_by_key(|p| (p.time, p.cluster))
            .or_else(|| matching().min_by_key(|p| (p.time, p.cluster)))
            .map(|p| p.cluster)
    }
}

const POINT_BYTES: u64 = core::mem::size_of::<SeekPoint>() as u64;

impl Segment {
    /// Builds the seek index: the `Cues` when present and readable, else a
    /// cluster scan (with a warning when `Cues` exist but cannot be read).
    pub fn index<S: Source>(&self, mut source: S, limits: &Limits) -> Result<Index, Error> {
        let mut warnings = Vec::new();
        match self.cue_points(&mut source, limits, &mut warnings) {
            Ok(Some(points)) if !points.is_empty() => {
                return Ok(Index {
                    source: IndexSource::Cues,
                    points,
                    warnings,
                });
            }
            Ok(Some(_)) => {}
            Ok(None) => warn(&mut warnings, Warning::NoCues),
            Err(Error::Io(kind)) => return Err(Error::Io(kind)),
            // Unreadable Cues: the clusters still give an index.
            Err(_) => warn(&mut warnings, Warning::InvalidCuePoint),
        }
        let points = self.scan_clusters(&mut source, limits)?;
        Ok(Index {
            source: IndexSource::ClusterScan,
            points,
            warnings,
        })
    }

    /// Reads the `Cues`, one `CuePoint` at a time; `None` when the segment
    /// has none. Cue points that point outside the segment are dropped with
    /// a warning.
    fn cue_points<S: Source>(
        &self,
        source: &mut S,
        limits: &Limits,
        warnings: &mut Vec<Warning>,
    ) -> Result<Option<Vec<SeekPoint>>, Error> {
        let Some(at) = self.cues else {
            return Ok(None);
        };
        let counter = Counter::new(limits.max_elements);
        counter.count(at)?;
        let cues = read_header(source, at, self.end, self.max_size_len)?;
        let end = cues
            .end()
            .filter(|e| *e <= self.end)
            .ok_or(Error::Truncated { offset: at })?;
        let mut points = Vec::new();
        let mut child = cues.data_offset();
        while child < end {
            counter.count(child)?;
            let h = read_header(source, child, end, self.max_size_len)?;
            let next = h.end().filter(|e| *e <= end).ok_or(Error::InvalidElement {
                offset: child,
                id: h.id,
                reason: "element runs past its parent",
            })?;
            if h.id == ids::CUE_POINT {
                let data = read_data(
                    source,
                    &h,
                    limits.max_element_bytes,
                    LimitKind::ElementBytes,
                )?;
                self.cue_point(&data, &h, &counter, &mut points, limits, warnings)?;
            }
            child = next;
        }
        Ok(Some(points))
    }

    fn cue_point(
        &self,
        data: &[u8],
        h: &ElementHeader,
        counter: &Counter,
        points: &mut Vec<SeekPoint>,
        limits: &Limits,
        warnings: &mut Vec<Warning>,
    ) -> Result<(), Error> {
        let mut time = None;
        let mut positions = Vec::new();
        for child in Children::new(data, h.data_offset(), self.max_size_len, counter) {
            let (c, d) = child?;
            match c.id {
                ids::CUE_TIME => time = Some(ebml::uint(d, &c)?),
                ids::CUE_TRACK_POSITIONS => positions.push((c, d)),
                _ => {}
            }
        }
        let Some(time) = time else {
            warn(warnings, Warning::InvalidCuePoint);
            return Ok(());
        };
        for (c, d) in positions {
            let (mut track, mut cluster, mut relative) = (None, None, None);
            for child in Children::new(d, c.data_offset(), self.max_size_len, counter) {
                let (e, d) = child?;
                match e.id {
                    ids::CUE_TRACK => track = Some(ebml::uint(d, &e)?),
                    ids::CUE_CLUSTER_POSITION => cluster = Some(ebml::uint(d, &e)?),
                    ids::CUE_RELATIVE_POSITION => relative = Some(ebml::uint(d, &e)?),
                    _ => {}
                }
            }
            let cluster = cluster
                .and_then(|p| self.data_offset.checked_add(p))
                .filter(|p| *p < self.end);
            let (Some(track), Some(cluster)) = (track, cluster) else {
                warn(warnings, Warning::InvalidCuePoint);
                continue;
            };
            let used = (points.len() as u64 + 1).saturating_mul(POINT_BYTES);
            if used > limits.max_index_bytes {
                return Err(Error::LimitExceeded {
                    limit: LimitKind::IndexBytes,
                    offset: h.offset,
                });
            }
            points.push(SeekPoint {
                time,
                track: Some(track),
                cluster,
                relative,
            });
        }
        Ok(())
    }

    /// Visits every Cluster from the first: its offset and `Timestamp`.
    /// Reads Cluster headers and the elements before each `Timestamp`;
    /// blocks are skipped by size, except inside an unknown-sized Cluster,
    /// whose children must be walked to find its end.
    fn scan_clusters<S: Source>(
        &self,
        source: &mut S,
        limits: &Limits,
    ) -> Result<Vec<SeekPoint>, Error> {
        let len = source.len()?;
        let end = self.end.min(len);
        let mut points = Vec::new();
        let mut work = 0u64;
        let mut count = |offset| {
            work += 1;
            if work > limits.max_block_work {
                Err(Error::LimitExceeded {
                    limit: LimitKind::BlockWork,
                    offset,
                })
            } else {
                Ok(())
            }
        };
        let Some(mut at) = self.first_cluster else {
            return Ok(points);
        };
        while at < end {
            count(at)?;
            let h = match read_header(source, at, end, self.max_size_len) {
                Ok(h) => h,
                // The index covers the complete part of a truncated file.
                Err(Error::Truncated { .. }) => break,
                Err(error) => return Err(error),
            };
            if h.id == ids::EBML {
                break;
            }
            if h.id != ids::CLUSTER {
                match h.end() {
                    Some(next) => {
                        at = next;
                        continue;
                    }
                    None => {
                        return Err(Error::InvalidElement {
                            offset: at,
                            id: h.id,
                            reason: "unknown size not allowed for this element",
                        });
                    }
                }
            }
            let cluster_end = h.end().map_or(end, |e| e.min(end));
            let mut child = h.data_offset();
            let mut timestamp = None;
            let mut next_cluster = h.end();
            while child < cluster_end {
                count(child)?;
                let c = match read_header(source, child, cluster_end, self.max_size_len) {
                    Ok(c) => c,
                    Err(Error::Truncated { .. }) => break,
                    Err(error) => return Err(error),
                };
                if h.size.is_none() && (c.id == ids::EBML || ids::TOP_LEVEL.contains(&c.id)) {
                    next_cluster = Some(child);
                    break;
                }
                let Some(c_end) = c.end() else {
                    return Err(Error::InvalidElement {
                        offset: child,
                        id: c.id,
                        reason: "unknown size not allowed for this element",
                    });
                };
                if c.id == ids::TIMESTAMP && timestamp.is_none() {
                    let data = read_data(source, &c, 8, LimitKind::ElementBytes)?;
                    timestamp = Some(ebml::uint(&data, &c)?);
                    if h.size.is_some() {
                        break;
                    }
                }
                child = c_end;
            }
            if let Some(time) = timestamp {
                let used = (points.len() as u64 + 1).saturating_mul(POINT_BYTES);
                if used > limits.max_index_bytes {
                    return Err(Error::LimitExceeded {
                        limit: LimitKind::IndexBytes,
                        offset: at,
                    });
                }
                points.push(SeekPoint {
                    time,
                    track: None,
                    cluster: at,
                    relative: None,
                });
            }
            match next_cluster {
                Some(next) if next > at => at = next,
                _ if h.size.is_none() => at = cluster_end,
                _ => break,
            }
        }
        Ok(points)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cluster_lookup() {
        let point = |time, cluster| SeekPoint {
            time,
            track: Some(1),
            cluster,
            relative: None,
        };
        let index = Index {
            source: IndexSource::Cues,
            points: vec![point(0, 100), point(1000, 200), point(2000, 300)],
            warnings: Vec::new(),
        };
        assert_eq!(index.cluster_for(Some(1), 1500), Some(200));
        assert_eq!(index.cluster_for(Some(1), 2000), Some(300));
        assert_eq!(index.cluster_for(None, 5), Some(100));
        assert_eq!(index.cluster_for(Some(2), 1500), None);
    }
}

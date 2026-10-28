//! Where a stream copy may start a video track (`docs/design/cut-points.md`):
//! at a sync sample the coded data agrees with. A sync sample table
//! (`stss`) can be absent, in which case ISO/IEC 14496-12 8.6.2 makes every
//! sample a sync sample, or it can list frames that are not random access
//! points; a cut there starts on a frame that does not decode.

use crate::hls::nal::Units;
use coreav_isobmff::access::AccessFormat;
use coreav_isobmff::{Codec, Error, Movie, Packet, Packets, Source, Track};

/// The cut rule of one video track.
#[derive(Clone, Copy)]
pub(crate) struct CutPoints {
    /// The H.264 bitstream view (`coreav_isobmff::access`), when readable.
    h264: Option<AccessFormat>,
    /// HEVC NAL units, when readable.
    hevc: Option<Units>,
    /// An AV1 track whose samples are in the file.
    av1: bool,
    /// `max_num_ref_frames` of the H.264 sequence parameter set.
    max_num_ref_frames: Option<u32>,
    /// The movie box samples' sync flags come from a sync sample table.
    sync_table: bool,
    /// Samples in the movie box; later ones are fragment samples, whose
    /// flags are their own.
    movie_samples: u64,
}

impl CutPoints {
    /// The rule for `track` of `movie`.
    pub(crate) fn of(movie: &Movie, track: &Track) -> Self {
        Self {
            h264: movie.access_format(track.index),
            hevc: Units::of(track).filter(|_| track.description.codec == Codec::Hevc),
            av1: track.description.codec == Codec::Av1
                && track.tables_valid
                && !track.external_data,
            max_num_ref_frames: movie.max_num_ref_frames(track.index),
            sync_table: track.sync_table(),
            movie_samples: u64::from(track.sample_count),
        }
    }

    /// Whether the rule reads the coded data of this track at all.
    pub(crate) fn reads(&self) -> bool {
        self.h264.is_some() || self.hevc.is_some() || self.av1
    }

    /// Whether `packet`, a packet of this track, may start a cut, read
    /// through `packets` (a listing of the track):
    ///
    /// - it is a sync sample, as the container states; and
    /// - for H.264, the frame starts decoding by itself
    ///   ([`coreav_isobmff::access::FrameAccess::starts_decoding`]),
    ///   whether or not a sync sample table states it; a frame whose NAL
    ///   units cannot be read keeps the container's word when a table (or
    ///   a fragment's flags) states it, and is no cut when nothing does;
    /// - for HEVC, it starts with an IRAP picture;
    /// - for AV1, it is a sync sample as the AV1 ISOBMFF binding defines one
    ///   ([`coreav_isobmff::access::read_av1_sync`]): a sequence header,
    ///   then a key frame shown at once; a sample whose OBUs cannot be read
    ///   keeps the container's word only when stated.
    ///
    /// Other codecs, and tracks whose bitstream cannot be read, keep the
    /// container's flag.
    pub(crate) fn allows<S: Source>(
        &self,
        packets: &mut Packets<'_, S>,
        packet: &Packet,
    ) -> Result<bool, Error> {
        if !packet.keyframe {
            return Ok(false);
        }
        self.allows_sync(packets, packet.sample, packet.offset, packet.size)
    }

    /// [`CutPoints::allows`] for the sync sample `sample` of `size` bytes at
    /// `offset`.
    pub(crate) fn allows_sync<S: Source>(
        &self,
        packets: &mut Packets<'_, S>,
        sample: u64,
        offset: u64,
        size: u32,
    ) -> Result<bool, Error> {
        let stated = self.sync_table || sample >= self.movie_samples;
        if let Some(format) = self.h264 {
            return Ok(packets
                .starts_decoding_at(format, offset, size, self.max_num_ref_frames)?
                .unwrap_or(stated));
        }
        if let Some(units) = self.hevc {
            return units.starts_sequence(packets.source_mut(), offset, size);
        }
        if self.av1 {
            return Ok(packets.av1_sync_at(offset, size)?.unwrap_or(stated));
        }
        Ok(true)
    }
}

/// The latest index in `lowest..end` that `accepts`, or `None`. Indices are
/// checked from `end` back in groups of 1, 2, 4 and so on, each group in
/// increasing order, so that the reads of a group, in file order, move
/// forward: a remote source serves a group from one response, and the
/// requests grow with the logarithm of the distance back
/// (docs/design/cut-points.md, "Cost").
pub(crate) fn latest_accepted(
    lowest: usize,
    end: usize,
    accepts: &mut dyn FnMut(usize) -> Result<bool, Error>,
) -> Result<Option<usize>, Error> {
    let mut end = end;
    let mut group = 1usize;
    while end > lowest {
        let begin = end.saturating_sub(group).max(lowest);
        let mut found = None;
        for index in begin..end {
            if accepts(index)? {
                found = Some(index);
            }
        }
        if found.is_some() {
            return Ok(found);
        }
        end = begin;
        group = group.saturating_mul(2);
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::latest_accepted;

    #[test]
    fn groups_double_back_from_the_end_in_increasing_order() {
        let mut seen = Vec::new();
        let found = latest_accepted(0, 10, &mut |i| {
            seen.push(i);
            Ok(i == 2)
        });
        assert_eq!(found.unwrap(), Some(2));
        // 9; 7, 8; 3 to 6; then 0 to 2 (the group stops at `lowest`).
        assert_eq!(seen, [9, 7, 8, 3, 4, 5, 6, 0, 1, 2]);
        // The latest in a group wins; none below `lowest` is checked.
        let mut seen = Vec::new();
        let found = latest_accepted(4, 10, &mut |i| {
            seen.push(i);
            Ok(i % 2 == 0 && i < 8)
        });
        assert_eq!(found.unwrap(), Some(6));
        assert_eq!(seen, [9, 7, 8, 4, 5, 6]);
        assert_eq!(latest_accepted(3, 3, &mut |_| Ok(true)).unwrap(), None);
    }
}

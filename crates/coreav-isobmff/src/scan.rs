//! Scanning a video track's samples for the parameter sets and frame headers
//! that set its picture size (`docs/design/frame-sizes.md`), with the
//! readers of [`crate::frames`].

use crate::frames::{self, FrameFormat, FrameSizes};
use crate::size::Size;
use crate::source::FIRST_FILL;
use crate::{Budget, Codec, Error, Limits, Movie, Source};

impl Movie {
    /// [`Movie::scan_frame_sizes`] of every track, in track order, under one
    /// work budget for the whole scan (`docs/design/frame-sizes.md`,
    /// decision 3): `max_sample_work` bounds the work of all tracks
    /// together, so a file of many video tracks cannot multiply it. A track
    /// whose scan fails on the file's structure or a limit gets `None`
    /// (unknown), as does a track whose codec has no frame size; the tracks
    /// after one that spent the budget are unknown too.
    ///
    /// # Errors
    ///
    /// [`Error::Io`] when `source` fails to read; [`Error::LimitExceeded`]
    /// (`Tracks`) when the list of sizes cannot be allocated.
    pub fn scan_all_frame_sizes<S: Source>(
        &self,
        mut source: S,
        limits: &Limits,
    ) -> Result<Vec<Option<Size>>, Error> {
        let mut budget = Budget::new(limits);
        let mut sizes = Vec::new();
        sizes
            .try_reserve_exact(self.tracks.len())
            .map_err(|_| Error::LimitExceeded {
                limit: crate::LimitKind::Tracks,
                offset: 0,
            })?;
        for track in &self.tracks {
            let size = match self.scan_frame_sizes(track.index, &mut source, limits, &mut budget) {
                Ok(size) => size,
                Err(error @ Error::Io(_)) => return Err(error),
                Err(_) => None,
            };
            sizes.push(size);
        }
        Ok(sizes)
    }

    /// The bound, in each dimension, of the picture sizes a video track's
    /// codec configuration and samples define: every sequence parameter set
    /// (H.264, HEVC) and sequence header (AV1) of the record and of every
    /// sample, every coded VP9 frame size, every ProRes frame header.
    ///
    /// Reads the headers of every sample (not their coded data) through
    /// `source`, bounded by `limits` and `budget` like a packet listing.
    /// `Ok(None)` when the track's codec has no such size, its samples are
    /// not available, none of them states a size, or a sample cannot be
    /// walked (a length or size past its end, a header that does not
    /// parse): a size an unread sample might exceed is not a bound.
    pub fn scan_frame_sizes<S: Source>(
        &self,
        index: usize,
        source: S,
        limits: &Limits,
        budget: &mut Budget,
    ) -> Result<Option<Size>, Error> {
        let track = self.tracks.get(index).ok_or(Error::NoSuchTrack)?;
        let description = &track.description;
        let config = description.codec_config.as_ref().map(|c| c.data.as_slice());
        let format = match (description.codec, config) {
            (Codec::H264, Some(record)) => FrameFormat::from_record(b"avcC", record),
            (Codec::Hevc, Some(record)) => FrameFormat::from_record(b"hvcC", record),
            (Codec::Av1, _) => Some(FrameFormat::Av1),
            (Codec::Vp9, _) => Some(FrameFormat::Vp9),
            (Codec::ProRes, _) => Some(FrameFormat::ProRes),
            _ => None,
        };
        let Some(format) = format else {
            return Ok(None);
        };
        if !track.tables_valid || track.external_data {
            return Ok(None);
        }
        // A record this crate cannot read (a parameter set that does not
        // parse, a structure past its end, a version other than 1): a
        // decoder applies its parameter sets to every sample without one,
        // or frames the samples differently, so no bound.
        if description.parameter_record_unreadable {
            return Ok(None);
        }
        let mut source = source;
        let advised = format.header_only()
            && !self.has_fragments()
            && self.advise_headers(index, &mut source, limits, budget);
        let mut sizes = FrameSizes::new(description.parameter_set_size);
        let scanned = self.scan_samples(index, format, &mut source, limits, budget, &mut sizes);
        if advised {
            // The next track's scan, or whatever reads next, goes anywhere.
            source.will_read_regions(&[]);
        }
        scanned?;
        Ok(if sizes.is_unverifiable() {
            None
        } else {
            sizes.bound()
        })
    }

    /// Reads the header of every sample of the track into `sizes`, stopping
    /// early once they are unverifiable.
    fn scan_samples<S: Source>(
        &self,
        index: usize,
        format: FrameFormat,
        source: S,
        limits: &Limits,
        budget: &mut Budget,
        sizes: &mut FrameSizes,
    ) -> Result<(), Error> {
        let mut packets = self.packets(index, source, limits, budget)?;
        while let Some(packet) = packets.next() {
            let packet = packet?;
            let (source, budget) = packets.parts_mut();
            frames::read_frame(
                format,
                source,
                packet.offset,
                u64::from(packet.size),
                sizes,
                &mut || budget.charge(1),
            )?;
            if sizes.is_unverifiable() {
                return Ok(());
            }
        }
        Ok(())
    }

    /// Tells `source` which bytes the scan of a header-only format reads:
    /// [`FIRST_FILL`] bytes from the start of each sample (the window a
    /// header read fills, `BufferedSource`, also past a smaller sample). A
    /// remote source can then skip the samples' coded data, which is most
    /// of the file for ProRes. Only for a source that can use it, and for at
    /// most [`MAX_ADVISED_SAMPLES`] samples over all the tracks of `budget`,
    /// so that the list stays small (1 MiB) and listing for advice stays
    /// bounded whatever the sample tables claim, however many tracks there
    /// are; a track past that is read through, as without advice. Listing
    /// the samples reads only the sample tables, held in memory, under a
    /// budget of its own, so advice never changes what the scan may read.
    /// Whether advice was given.
    fn advise_headers<S: Source>(
        &self,
        index: usize,
        source: &mut S,
        limits: &Limits,
        budget: &mut Budget,
    ) -> bool {
        if !source.wants_regions() {
            return false;
        }
        let allowed = MAX_ADVISED_SAMPLES.saturating_sub(budget.advised);
        if allowed == 0 {
            return false;
        }
        let head = FIRST_FILL as u64;
        let mut listing = Budget::new(limits);
        let Ok(packets) = self.packets(index, &mut *source, limits, &mut listing) else {
            budget.advised = MAX_ADVISED_SAMPLES;
            return false;
        };
        let mut regions: Vec<(u64, u64)> = Vec::new();
        for packet in packets {
            // Every sample listed counts, advised or not: a track that does
            // not fit uses up what is left.
            budget.advised = budget.advised.saturating_add(1);
            let Ok(packet) = packet else {
                return false;
            };
            if regions.len() >= allowed || regions.try_reserve(1).is_err() {
                budget.advised = MAX_ADVISED_SAMPLES;
                return false;
            }
            // The window a header read fills, even past a smaller sample.
            regions.push((packet.offset, head));
        }
        if regions.is_empty() {
            return false;
        }
        regions.sort_unstable();
        source.will_read_regions(&regions);
        true
    }
}

/// The most samples whose header regions [`Movie::scan_frame_sizes`]
/// advises, over all the tracks of one request: 65,536 regions of 16
/// bytes.
const MAX_ADVISED_SAMPLES: usize = 1 << 16;

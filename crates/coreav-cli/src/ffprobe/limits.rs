//! The Matroska and transport stream limits that follow a caller's
//! [`coreav_isobmff::Limits`], so that one set of bounds governs every
//! container [`super::probe_limited`] reads.
//!
//! A field with a counterpart of the same default is carried as it is
//! (`max_total_boxes` bounds Matroska's element headers, `max_box_bytes` an
//! element, `max_table_bytes` the cue index, the inflated-size budget and
//! the bytes a transport stream holds at once, `max_sample_work` the block
//! work, `max_tracks` the tracks and streams). A field whose default
//! differs is the container's own at the ISO default, lowered when the
//! caller goes below the ISO default and raised when it goes above (see
//! [`carried`]), so the defaults are the readers' own and no limit grows
//! unless the caller raised it. The transport stream's probe distances and
//! its program count have no counterpart and keep their defaults.

use coreav_isobmff::Limits;

/// `own` where the caller left `value` at `base`, the lower of the two
/// below it and the higher above it.
fn carried(value: u64, base: u64, own: u64) -> u64 {
    match value.cmp(&base) {
        std::cmp::Ordering::Less => value.min(own),
        std::cmp::Ordering::Equal => own,
        std::cmp::Ordering::Greater => value.max(own),
    }
}

/// The Matroska limits `limits` implies.
pub fn matroska(limits: &Limits) -> coreav_mkv::Limits {
    coreav_mkv::Limits {
        max_elements: limits.max_total_boxes,
        max_element_bytes: limits.max_box_bytes,
        max_retained_bytes: limits.max_retained_bytes,
        max_index_bytes: limits.max_table_bytes,
        max_packet_bytes: limits.max_packet_bytes,
        max_block_work: limits.max_sample_work,
        max_tracks: limits.max_tracks,
        max_inflated_bytes: limits.max_table_bytes,
    }
}

/// The transport stream limits `limits` implies.
pub fn mpegts(limits: &Limits) -> coreav_mpegts::Limits {
    let base = Limits::default();
    let own = coreav_mpegts::Limits::default();
    coreav_mpegts::Limits {
        max_streams: limits.max_tracks,
        max_packet_bytes: carried(
            limits.max_packet_bytes,
            base.max_packet_bytes,
            own.max_packet_bytes,
        ),
        max_buffered_bytes: limits.max_table_bytes,
        max_listing_packets: carried(
            limits.max_sample_work,
            base.max_sample_work,
            own.max_listing_packets,
        ),
        ..own
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_carry_to_each_readers_defaults() {
        let limits = Limits::default();
        assert_eq!(matroska(&limits), coreav_mkv::Limits::default());
        assert_eq!(mpegts(&limits), coreav_mpegts::Limits::default());
    }

    #[test]
    fn a_limit_is_lowered_or_raised_from_the_default_and_never_by_the_default() {
        let lowered = Limits {
            max_packet_bytes: 1000,
            max_sample_work: 10,
            max_table_bytes: 4096,
            ..Limits::default()
        };
        let ts = mpegts(&lowered);
        assert_eq!(ts.max_packet_bytes, 1000);
        assert_eq!(ts.max_listing_packets, 10);
        assert_eq!(ts.max_buffered_bytes, 4096);
        let mkv = matroska(&lowered);
        assert_eq!(mkv.max_packet_bytes, 1000);
        assert_eq!(mkv.max_block_work, 10);
        assert_eq!(mkv.max_index_bytes, 4096);
        assert_eq!(mkv.max_inflated_bytes, 4096);

        let edge = Limits {
            max_table_bytes: 64 << 20,
            ..Limits::default()
        };
        assert_eq!(matroska(&edge).max_index_bytes, 64 << 20);
        assert_eq!(matroska(&edge).max_inflated_bytes, 64 << 20);
        assert_eq!(mpegts(&edge).max_buffered_bytes, 64 << 20);
        assert_eq!(
            mpegts(&edge).max_packet_bytes,
            coreav_mpegts::Limits::default().max_packet_bytes
        );

        let raised = Limits {
            max_packet_bytes: 256 << 20,
            max_sample_work: 1 << 40,
            ..Limits::default()
        };
        assert_eq!(mpegts(&raised).max_packet_bytes, 256 << 20);
        assert_eq!(mpegts(&raised).max_listing_packets, 1 << 40);
    }
}

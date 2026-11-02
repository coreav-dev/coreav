//! Codec-neutral raw-frame and encoded-packet boundary regressions.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::num::NonZeroU64;

use coreav_decode::ErrorKind;
use coreav_frame::{Picture, PixelFormat};
use coreav_transcode::contract::{PacketLimits, Packets, Submissions, VideoLayout};

fn picture() -> Picture {
    Picture::zeroed(2, 2, PixelFormat::YUV420P).unwrap()
}

fn submissions(max: u64) -> Submissions {
    Submissions::new(
        VideoLayout::new(2, 2, PixelFormat::YUV420P, 6).unwrap(),
        max,
    )
}

fn packets() -> Packets {
    Packets::new(PacketLimits {
        max_packets: 3,
        max_packet_bytes: 4,
        max_output_bytes: 8,
    })
}

#[test]
fn submissions_check_layout_order_and_count_without_retaining_pixels() {
    let mut sent = submissions(2);
    let first = sent.submit(picture(), -20).unwrap();
    assert_eq!(first.id().index(), 0);
    assert_eq!(first.pts(), -20);
    assert_eq!(first.picture().packed_len(), 6);
    assert_eq!(first.into_picture().width(), 2);
    assert_eq!(
        sent.submit(picture(), -20).unwrap_err().kind,
        ErrorKind::InvalidData
    );
    let wrong = Picture::zeroed(4, 2, PixelFormat::YUV420P).unwrap();
    assert_eq!(
        sent.submit(wrong, 0).unwrap_err().kind,
        ErrorKind::InvalidData
    );
    assert_eq!(sent.submit(picture(), 0).unwrap().id().index(), 1);
    assert_eq!(
        sent.submit(picture(), 20).unwrap_err().kind,
        ErrorKind::Limit
    );
}

#[test]
fn raw_layout_checks_zero_invalid_formats_and_frame_budget() {
    assert!(VideoLayout::new(0, 2, PixelFormat::YUV420P, 6).is_err());
    assert!(VideoLayout::new(2, 0, PixelFormat::YUV420P, 6).is_err());
    assert!(VideoLayout::new(2, 2, PixelFormat::Gbr { depth: 0 }, 6).is_err());
    assert_eq!(
        VideoLayout::new(2, 2, PixelFormat::YUV420P, 5)
            .unwrap_err()
            .kind,
        ErrorKind::Limit
    );
    assert!(VideoLayout::new(u32::MAX, u32::MAX, PixelFormat::YUV420P, u64::MAX).is_err());
}

#[test]
fn reordered_samples_get_their_submitted_times_and_owned_bytes() {
    let mut sent = submissions(3);
    for time in [0, 10, 25] {
        sent.submit(picture(), time).unwrap();
    }
    let mut packets = packets();
    let first = packets.accept(vec![1, 2], 0, true, &sent).unwrap();
    assert_eq!(first.data(), &[1, 2]);
    assert_eq!(first.meta().bytes(), 2);
    assert!(first.meta().sync());
    assert_eq!(first.meta().frame().index(), 0);
    packets.accept(vec![3], 2, false, &sent).unwrap();
    packets.accept(vec![4], 1, false, &sent).unwrap();
    let samples = packets.finish(&sent, NonZeroU64::new(15).unwrap()).unwrap();
    assert_eq!(
        samples.iter().map(|(_, time)| time.pts).collect::<Vec<_>>(),
        [0, 25, 10]
    );
    assert_eq!(
        samples.iter().map(|(_, time)| time.dts).collect::<Vec<_>>(),
        [-15, -5, 10]
    );
    assert_eq!(samples.last().unwrap().1.duration, 15);
}

#[test]
fn invalid_packets_do_not_consume_the_byte_or_packet_budget() {
    let mut sent = submissions(3);
    sent.submit(picture(), 0).unwrap();
    let mut packets = packets();
    assert_eq!(
        packets.accept(vec![1], 1, false, &sent).unwrap_err().kind,
        ErrorKind::InvalidData
    );
    assert_eq!(
        packets.accept(vec![], 0, false, &sent).unwrap_err().kind,
        ErrorKind::InvalidData
    );
    assert_eq!(
        packets
            .accept(vec![1; 5], 0, false, &sent)
            .unwrap_err()
            .kind,
        ErrorKind::Limit
    );
    packets.accept(vec![1; 4], 0, true, &sent).unwrap();
    assert_eq!(
        packets.accept(vec![1], 0, false, &sent).unwrap_err().kind,
        ErrorKind::InvalidData
    );
    packets.finish(&sent, NonZeroU64::new(1).unwrap()).unwrap();
}

#[test]
fn drain_requires_a_complete_permutation_and_respects_both_output_limits() {
    let duration = NonZeroU64::new(1).unwrap();
    let mut sent = submissions(3);
    for time in [0, 1, 2] {
        sent.submit(picture(), time).unwrap();
    }
    let mut duplicate = packets();
    for _ in 0..3 {
        duplicate.accept(vec![1], 0, false, &sent).unwrap();
    }
    assert_eq!(
        duplicate.accept(vec![1], 1, false, &sent).unwrap_err().kind,
        ErrorKind::Limit
    );
    assert_eq!(
        duplicate.finish(&sent, duration).unwrap_err().kind,
        ErrorKind::InvalidData
    );
    let mut partial = packets();
    partial.accept(vec![1], 0, true, &sent).unwrap();
    assert_eq!(
        partial.finish(&sent, duration).unwrap_err().kind,
        ErrorKind::InvalidData
    );
    let mut too_large = packets();
    too_large.accept(vec![1; 4], 0, true, &sent).unwrap();
    too_large.accept(vec![1; 4], 1, false, &sent).unwrap();
    assert_eq!(
        too_large.accept(vec![1], 2, false, &sent).unwrap_err().kind,
        ErrorKind::Limit
    );
    assert!(packets().finish(&submissions(0), duration).is_err());
}

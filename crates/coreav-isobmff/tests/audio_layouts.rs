//! Sound sample entries whose stored fields need care: QuickTime
//! uncompressed audio with a constant sample size of 1, and AAC, AC-3 and
//! E-AC-3 channel counts from their configuration.
#![allow(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]

mod common;

use common::{alac_cookie, av_movie_with, bx, cat, dfla, full, lossless_entry, u32s};
use coreav_isobmff::{
    Allowlist, Budget, Codec, Limits, Movie, SampleDescription, SliceSource, TrackKind, probe,
};

/// A QuickTime sound sample entry: version, channels, sample size, rate
/// 48 kHz, and for version 1 the four extra fields.
fn sound_entry(kind: &[u8; 4], version: u16, channels: u16, bits: u16, v1: [u32; 4]) -> Vec<u8> {
    let mut body = vec![0u8; 6];
    body.extend_from_slice(&1u16.to_be_bytes());
    body.extend_from_slice(&version.to_be_bytes());
    body.extend_from_slice(&[0u8; 6]);
    body.extend_from_slice(&channels.to_be_bytes());
    body.extend_from_slice(&bits.to_be_bytes());
    body.extend_from_slice(&[0u8; 4]);
    body.extend_from_slice(&(48_000u32 << 16).to_be_bytes());
    if version == 1 {
        body.extend_from_slice(&u32s(&v1));
    }
    bx(kind, &body)
}

/// A movie with one audio track: `frames` samples of 1 tick at 48 kHz in
/// one chunk of `data_bytes` bytes, and `stsz` stored as a constant size
/// with a count.
fn audio_movie(entry: Vec<u8>, frames: u32, stsz: (u32, u32), data_bytes: usize) -> Vec<u8> {
    let ftyp = bx(b"ftyp", b"qt  \0\0\x02\0qt  ");
    let mdat = bx(b"mdat", &vec![0x5A; data_bytes]);
    let offset = u32::try_from(ftyp.len() + 8).unwrap();
    let mut mvhd = u32s(&[0, 0, 1000, frames / 48]);
    mvhd.extend_from_slice(&[0u8; 80]);
    let mut tkhd = u32s(&[0, 0, 1, 0, frames / 48]);
    tkhd.extend_from_slice(&[0u8; 16]);
    tkhd.extend_from_slice(&u32s(&[0x10000, 0, 0, 0, 0x10000, 0, 0, 0, 0x4000_0000]));
    tkhd.extend_from_slice(&u32s(&[0, 0]));
    let mut mdhd = u32s(&[0, 0, 48_000, frames]);
    mdhd.extend_from_slice(&[0x55, 0xc4, 0, 0]);
    let mut hdlr = u32s(&[0]);
    hdlr.extend_from_slice(b"soun");
    hdlr.extend_from_slice(&[0u8; 13]);
    let stbl = bx(
        b"stbl",
        &cat(&[
            full(b"stsd", 0, 0, &[u32s(&[1]), entry].concat()),
            full(b"stts", 0, 0, &u32s(&[1, frames, 1])),
            full(b"stsc", 0, 0, &u32s(&[1, 1, frames, 1])),
            full(b"stsz", 0, 0, &u32s(&[stsz.0, stsz.1])),
            full(b"stco", 0, 0, &u32s(&[1, offset])),
        ]),
    );
    let dref = full(
        b"dref",
        0,
        0,
        &[u32s(&[1]), full(b"url ", 0, 1, &[])].concat(),
    );
    let minf = bx(
        b"minf",
        &cat(&[full(b"smhd", 0, 0, &[0u8; 4]), bx(b"dinf", &dref), stbl]),
    );
    let mdia = bx(
        b"mdia",
        &cat(&[full(b"mdhd", 0, 0, &mdhd), full(b"hdlr", 0, 0, &hdlr), minf]),
    );
    let trak = bx(b"trak", &cat(&[full(b"tkhd", 0, 3, &tkhd), mdia]));
    let moov = bx(b"moov", &cat(&[full(b"mvhd", 0, 0, &mvhd), trak]));
    cat(&[ftyp, mdat, moov])
}

fn probed(data: &[u8]) -> Movie {
    probe(
        SliceSource::new(data),
        &Limits::default(),
        &Allowlist::default(),
    )
    .unwrap()
}

/// (count, every packet's size, offsets advance by the size, last end).
fn packets(data: &[u8], movie: &Movie) -> (usize, Vec<u64>, u64) {
    let limits = Limits::default();
    let mut budget = Budget::new(&limits);
    let mut sizes = Vec::new();
    let mut end = 0;
    for packet in movie
        .packets(0, SliceSource::new(data), &limits, &mut budget)
        .unwrap()
    {
        let packet = packet.unwrap();
        if !sizes.is_empty() {
            assert_eq!(packet.offset, end, "packets are contiguous");
        }
        end = packet.offset + u64::from(packet.size);
        sizes.push(u64::from(packet.size));
    }
    sizes.dedup();
    (movie.tracks[0].sample_count as usize, sizes, end)
}

#[test]
fn version_0_stereo_with_size_1_counts_frames() {
    // 8-bit unsigned stereo (`raw `): 2-byte frames, stsz says 1 per frame.
    let data = audio_movie(sound_entry(b"raw ", 0, 2, 8, [0; 4]), 4800, (1, 4800), 9600);
    let movie = probed(&data);
    let track = &movie.tracks[0];
    assert!(track.tables_valid);
    assert_eq!(track.sample_bytes, Some(9600));
    assert_eq!(track.constant_sample_size(), Some(2));
    let (count, sizes, end) = packets(&data, &movie);
    assert_eq!((count, sizes), (4800, vec![2]));
    assert_eq!(
        end,
        u64::try_from(data.len()).unwrap() - u64::try_from(movie_tail(&data)).unwrap()
    );
}

/// Bytes after `mdat` (the movie box), so that `end` is checked against the
/// end of the media data.
fn movie_tail(data: &[u8]) -> usize {
    let at = data.windows(4).position(|w| w == b"moov").unwrap() - 4;
    data.len() - at
}

#[test]
fn version_1_with_size_1_counts_bytes() {
    // 16-bit little-endian stereo (`sowt`), bytesPerFrame 4: stsz counts
    // the bytes, stts and stsc the frames.
    let entry = sound_entry(b"sowt", 1, 2, 16, [1, 2, 4, 2]);
    let data = audio_movie(entry, 4800, (1, 19_200), 19_200);
    let movie = probed(&data);
    let track = &movie.tracks[0];
    assert!(track.tables_valid, "{:?}", track.warnings);
    assert_eq!(track.sample_count, 4800);
    assert_eq!(track.sample_bytes, Some(19_200));
    let (_, sizes, _) = packets(&data, &movie);
    assert_eq!(sizes, vec![4]);
}

#[test]
fn eight_bit_mono_keeps_its_one_byte_samples() {
    // `sowt` with a sample size of 8 and one channel: a frame is 1 byte, so
    // the stored size is the real one.
    let data = audio_movie(sound_entry(b"sowt", 0, 1, 8, [0; 4]), 4800, (1, 4800), 4800);
    let movie = probed(&data);
    let track = &movie.tracks[0];
    assert_eq!(track.description.pcm.unwrap().bits, 8);
    assert_eq!(track.sample_bytes, Some(4800));
    let (count, sizes, _) = packets(&data, &movie);
    assert_eq!((count, sizes), (4800, vec![1]));
}

#[test]
fn other_counts_and_real_sizes_are_left_as_stored() {
    // A count that is neither the frames nor the bytes: invalid as before.
    let data = audio_movie(
        sound_entry(b"twos", 0, 2, 16, [0; 4]),
        4800,
        (1, 5000),
        20_000,
    );
    let track = &probed(&data).tracks[0];
    assert!(!track.tables_valid);
    // A real constant size is never changed.
    let data = audio_movie(
        sound_entry(b"twos", 0, 2, 16, [0; 4]),
        4800,
        (4, 4800),
        19_200,
    );
    let track = &probed(&data).tracks[0];
    assert!(track.tables_valid);
    assert_eq!(track.sample_bytes, Some(19_200));
}

#[test]
fn fixed_width_codes_and_iso_entries() {
    // `in24` version 1 with bytesPerFrame 0: the frame is 2 channels × 3
    // bytes, whatever the template sample size (16) says.
    let entry = sound_entry(b"in24", 1, 2, 16, [1, 0, 0, 0]);
    let data = audio_movie(entry, 4800, (1, 4800), 28_800);
    let track = &probed(&data).tracks[0];
    assert_eq!(track.sample_bytes, Some(28_800));
    // ISO `ipcm` with a constant size of 1 is left as stored: only the
    // QuickTime codes follow the QuickTime convention.
    let data = audio_movie(
        sound_entry(b"ipcm", 0, 2, 16, [0; 4]),
        4800,
        (1, 4800),
        4800,
    );
    let track = &probed(&data).tracks[0];
    assert_eq!(track.sample_bytes, Some(4800));
}

/// An `mp4a` entry with `channels` in the sample entry and an `esds` whose
/// `AudioSpecificConfig` is `config`.
fn aac_with(channels: u16, config: &[u8]) -> Vec<u8> {
    let specific_len = u8::try_from(config.len()).unwrap();
    let decoder_len = 13 + 2 + specific_len;
    let mut esds = vec![3, 3 + 2 + decoder_len, 0, 0, 0, 4, decoder_len, 0x40, 0x15];
    esds.extend_from_slice(&[0; 11]);
    esds.extend_from_slice(&[5, specific_len]);
    esds.extend_from_slice(config);
    let mut body = vec![0u8; 6];
    body.extend_from_slice(&1u16.to_be_bytes());
    body.extend_from_slice(&[0u8; 8]);
    body.extend_from_slice(&channels.to_be_bytes());
    body.extend_from_slice(&16u16.to_be_bytes());
    body.extend_from_slice(&[0u8; 4]);
    body.extend_from_slice(&(48_000u32 << 16).to_be_bytes());
    body.extend_from_slice(&full(b"esds", 0, 0, &esds));
    bx(b"mp4a", &body)
}

/// The channels probe gives an `ac-3` or `ec-3` entry that says 2 and
/// whose `dac3` or `dec3` is `config`.
fn ac3_channels(kind: &[u8; 4], config_kind: &[u8; 4], config: &[u8]) -> Option<u32> {
    let mut body = vec![0u8; 6];
    body.extend_from_slice(&1u16.to_be_bytes());
    body.extend_from_slice(&[0u8; 8]);
    body.extend_from_slice(&2u16.to_be_bytes());
    body.extend_from_slice(&16u16.to_be_bytes());
    body.extend_from_slice(&[0u8; 4]);
    body.extend_from_slice(&(48_000u32 << 16).to_be_bytes());
    body.extend_from_slice(&bx(config_kind, config));
    let movie = probed(&av_movie_with(bx(kind, &body), false));
    let audio = movie
        .tracks
        .iter()
        .find(|t| t.kind == TrackKind::Audio)
        .unwrap();
    audio.description.channels
}

#[test]
fn ac3_and_eac3_channels_come_from_the_configuration() {
    // dac3: fscod, bsid, bsmod, acmod, lfeon, bit_rate_code, reserved.
    let dac3 = |acmod, lfeon| {
        pack(&[
            (0, 2),
            (8, 5),
            (0, 3),
            (acmod, 3),
            (lfeon, 1),
            (10, 5),
            (0, 5),
        ])
    };
    assert_eq!(ac3_channels(b"ac-3", b"dac3", &dac3(7, 1)), Some(6));
    assert_eq!(ac3_channels(b"ac-3", b"dac3", &dac3(1, 0)), Some(1));
    // acmod 0 is two independent mono channels.
    assert_eq!(ac3_channels(b"ac-3", b"dac3", &dac3(0, 0)), Some(2));
    // dec3: data_rate, num_ind_sub, then the substream's fscod, bsid,
    // reserved, asvc, bsmod, acmod, lfeon, reserved, num_dep_sub, reserved.
    let dec3 = |acmod, lfeon| {
        pack(&[
            (384, 13),
            (0, 3),
            (0, 2),
            (16, 5),
            (0, 1),
            (0, 1),
            (0, 3),
            (acmod, 3),
            (lfeon, 1),
            (0, 3),
            (0, 4),
            (0, 1),
        ])
    };
    assert_eq!(ac3_channels(b"ec-3", b"dec3", &dec3(7, 1)), Some(6));
    assert_eq!(ac3_channels(b"ec-3", b"dec3", &dec3(6, 0)), Some(4));
    // A truncated box: the entry's count.
    assert_eq!(ac3_channels(b"ac-3", b"dac3", &[0x10]), Some(2));
}

/// Packs `(value, width)` fields most significant bit first.
fn pack(fields: &[(u32, u32)]) -> Vec<u8> {
    let mut bits = Vec::new();
    for &(value, width) in fields {
        for i in (0..width).rev() {
            bits.push(value >> i & 1 == 1);
        }
    }
    bits.chunks(8)
        .map(|c| {
            c.iter()
                .enumerate()
                .fold(0u8, |b, (i, bit)| b | (u8::from(*bit) << (7 - i)))
        })
        .collect()
}

fn aac_description(entry_channels: u16, config: &[u8]) -> SampleDescription {
    let movie = probed(&av_movie_with(aac_with(entry_channels, config), false));
    let audio = movie
        .tracks
        .iter()
        .find(|t| t.kind == TrackKind::Audio)
        .unwrap();
    assert_eq!(audio.description.codec, Codec::Aac);
    audio.description.clone()
}

fn aac_channels(entry_channels: u16, config: &[u8]) -> Option<u32> {
    aac_description(entry_channels, config).channels
}

/// Whether probe flags that parametric stereo may be signalled only in
/// the bitstream.
fn aac_implicit_ps(entry_channels: u16, config: &[u8]) -> bool {
    aac_description(entry_channels, config).implicit_ps_possible
}

#[test]
fn aac_channels_come_from_the_configuration() {
    // LC, 48 kHz (index 3), channelConfiguration 6 (5.1) under an entry of 2.
    let surround = pack(&[(2, 5), (3, 4), (6, 4), (0, 3)]);
    assert_eq!(aac_channels(2, &surround), Some(6));
    // channelConfiguration 7 is 7.1: 8 channels.
    assert_eq!(
        aac_channels(2, &pack(&[(2, 5), (3, 4), (7, 4), (0, 3)])),
        Some(8)
    );
    // Mono under a mono entry.
    assert_eq!(
        aac_channels(1, &pack(&[(2, 5), (3, 4), (1, 4), (0, 3)])),
        Some(1)
    );
    // channelConfiguration 0 (program config element): the entry's count.
    assert_eq!(
        aac_channels(2, &pack(&[(2, 5), (3, 4), (0, 4), (0, 3)])),
        Some(2)
    );
}

#[test]
fn aac_parametric_stereo_counts_as_stereo() {
    // Explicit HE-AAC v2 (object type 29): mono core, stereo output, even
    // under a mono entry.
    let explicit = pack(&[(29, 5), (6, 4), (1, 4), (3, 4), (2, 5)]);
    assert_eq!(aac_channels(1, &explicit), Some(2));
    // An LC mono configuration under a stereo entry may carry parametric
    // stereo signalled only in the bitstream: the entry's 2 stays.
    let lc_mono = pack(&[(2, 5), (6, 4), (1, 4), (0, 3)]);
    assert_eq!(aac_channels(2, &lc_mono), Some(2));
    assert!(aac_implicit_ps(2, &lc_mono));
    // Under a mono entry it is mono.
    assert_eq!(aac_channels(1, &lc_mono), Some(1));
    assert!(!aac_implicit_ps(1, &lc_mono));
    // Explicit parametric stereo, a stereo configuration and an explicit
    // no-SBR signal (sync extension 0x2b7, SBR object type 5, flag 0) leave
    // nothing implicit.
    assert!(!aac_implicit_ps(1, &explicit));
    assert!(!aac_implicit_ps(
        2,
        &pack(&[(2, 5), (6, 4), (2, 4), (0, 3)])
    ));
    let no_sbr = pack(&[(2, 5), (6, 4), (1, 4), (0, 3), (0x2b7, 11), (5, 5), (0, 1)]);
    assert_eq!(aac_channels(2, &no_sbr), Some(1));
    assert!(!aac_implicit_ps(2, &no_sbr));
    // SBR without parametric stereo (0x548, psPresentFlag 0): mono.
    let no_ps = pack(&[
        (2, 5),
        (6, 4),
        (1, 4),
        (0, 3),
        (0x2b7, 11),
        (5, 5),
        (1, 1),
        (3, 4),
        (0x548, 11),
        (0, 1),
    ]);
    assert_eq!(aac_channels(2, &no_ps), Some(1));
    assert!(!aac_implicit_ps(2, &no_ps));
    // Explicit hierarchical SBR (object type 5) over an LC mono core can
    // still carry implicit parametric stereo.
    let hierarchical = pack(&[(5, 5), (6, 4), (1, 4), (3, 4), (2, 5), (0, 3)]);
    assert_eq!(aac_channels(2, &hierarchical), Some(2));
    assert!(aac_implicit_ps(2, &hierarchical));
}

/// The sample entry's rate is kept as stored, for the writers that copy it
/// into a version 0 sound entry; the configuration's output rate is apart:
/// the core rate, or the explicit SBR extension's.
#[test]
fn the_configured_rate_is_apart_from_the_stored_one() {
    // LC at 96 kHz (index 0), stereo; the entry says 48 kHz.
    let lc96 = aac_description(2, &pack(&[(2, 5), (0, 4), (2, 4), (0, 3)]));
    assert_eq!(lc96.sample_rate, Some(48_000));
    assert_eq!(lc96.configured_sample_rate, Some(96_000));
    // LC 24 kHz with explicit SBR at 48 kHz in the backward compatible
    // extension.
    let sbr = aac_description(
        2,
        &pack(&[
            (2, 5),
            (6, 4),
            (2, 4),
            (0, 3),
            (0x2b7, 11),
            (5, 5),
            (1, 1),
            (3, 4),
        ]),
    );
    assert_eq!(sbr.configured_sample_rate, Some(48_000));
    // A reserved index: no configured rate.
    let reserved = aac_description(2, &pack(&[(2, 5), (13, 4), (2, 4), (0, 3)]));
    assert_eq!(reserved.configured_sample_rate, None);
}

fn lossless_description(entry: Vec<u8>) -> (SampleDescription, Vec<coreav_isobmff::Warning>) {
    let movie = probe(
        SliceSource::new(&av_movie_with(entry, false)),
        &Limits::default(),
        &Allowlist::listing(),
    )
    .unwrap();
    let audio = movie
        .tracks
        .iter()
        .find(|t| t.kind == TrackKind::Audio)
        .unwrap();
    (audio.description.clone(), audio.warnings.clone())
}

/// FLAC's and ALAC's rate and bit depth come from their configuration: the
/// sample entry's 16.16 rate cannot hold 65,536 Hz or more, and a writer
/// stores something else there for higher rates.
#[test]
fn lossless_rates_and_depths_come_from_the_configuration() {
    for (rate, bits) in [
        (44_100, 16),
        (48_000, 24),
        (88_200, 16),
        (96_000, 24),
        (192_000, 16),
        (655_350, 32),
        (655_351, 24),
        (1_048_575, 24),
    ] {
        let (flac, warnings) = lossless_description(lossless_entry(b"fLaC", dfla(rate, bits)));
        assert_eq!(flac.codec, Codec::Flac);
        assert_eq!(flac.sample_rate, Some(48_000));
        assert_eq!(flac.configured_sample_rate, Some(rate));
        assert_eq!(flac.configured_bits, Some(u16::try_from(bits).unwrap()));
        assert!(warnings.is_empty(), "{warnings:?}");
        let depth = u8::try_from(bits.min(24)).unwrap();
        let (alac, warnings) =
            lossless_description(lossless_entry(b"alac", alac_cookie(rate, depth)));
        assert_eq!(alac.sample_rate, Some(48_000));
        assert_eq!(alac.configured_sample_rate, Some(rate));
        assert_eq!(alac.configured_bits, Some(u16::from(depth)));
        assert!(warnings.is_empty(), "{warnings:?}");
    }
}

/// A configuration that cannot be read leaves the rate to the sample entry
/// and the bit depth unknown, with a warning that names the box.
#[test]
fn a_malformed_lossless_configuration_is_refused() {
    let malformed = |kind: &[u8; 4]| {
        coreav_isobmff::Warning::MalformedCodecConfig(coreav_isobmff::FourCc(*kind))
    };
    let mut version_1 = dfla(96_000, 24);
    version_1[8] = 1;
    let mut not_streaminfo = dfla(96_000, 24);
    not_streaminfo[12] = 0x84;
    let mut short_block = dfla(96_000, 24);
    short_block[15] = 33;
    let mut truncated = dfla(96_000, 24);
    truncated.truncate(30);
    truncated[3] = 30;
    let mut tiny_blocks = dfla(96_000, 24);
    tiny_blocks[16..20].copy_from_slice(&[0, 15, 0, 15]);
    let mut min_above_max = dfla(96_000, 24);
    min_above_max[16..20].copy_from_slice(&[0x10, 0, 0x0F, 0xFF]);
    for config in [
        version_1,
        not_streaminfo,
        short_block,
        truncated,
        tiny_blocks,
        min_above_max,
        dfla(0, 16),
        dfla(96_000, 3),
    ] {
        let (flac, warnings) = lossless_description(lossless_entry(b"fLaC", config));
        assert_eq!(flac.sample_rate, Some(48_000));
        assert_eq!(flac.configured_sample_rate, None);
        assert_eq!(flac.configured_bits, None);
        assert_eq!(warnings, vec![malformed(b"dfLa")]);
    }
    let mut version_1 = alac_cookie(96_000, 24);
    version_1[8] = 1;
    let mut truncated = alac_cookie(96_000, 24);
    truncated.truncate(35);
    truncated[3] = 35;
    let mut compatible_1 = alac_cookie(96_000, 24);
    compatible_1[16] = 1;
    for config in [
        version_1,
        truncated,
        compatible_1,
        alac_cookie(96_000, 0),
        alac_cookie(96_000, 33),
        alac_cookie(0, 16),
    ] {
        let (alac, warnings) = lossless_description(lossless_entry(b"alac", config));
        assert_eq!(alac.sample_rate, Some(48_000));
        assert_eq!(alac.configured_sample_rate, None);
        assert_eq!(alac.configured_bits, None);
        assert_eq!(warnings, vec![malformed(b"alac")]);
    }
    for (kind, config) in [(b"fLaC", b"dfLa"), (b"alac", b"alac")] {
        let (description, warnings) = lossless_description(common::audio_entry(kind));
        assert_eq!(description.configured_sample_rate, None);
        assert_eq!(
            warnings,
            vec![malformed(config)],
            "{kind:?} without {config:?}"
        );
    }
}

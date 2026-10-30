//! The pool's memory bound (`docs/design/audio-decode.md`, "Threads"):
//! packets of 15 MiB that each decode to one sample, all pointing at the
//! same bytes of a small MP4 file, must not pile up in memory when several
//! threads decode. Run in release by CI (pinned, like the demuxers' hostile
//! tests); a debug build copies the packets too slowly. Linux only: the
//! peak memory comes from `/proc/self/status`.
#![cfg(target_os = "linux")]
#![allow(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic
)]

use coreav_audio::{Error, Limits, Options, OutputFormat, Sink, decode};
use coreav_isobmff::SliceSource;

struct Count(u64);

impl Sink for Count {
    fn start(&mut self, _: &OutputFormat) -> Result<(), Error> {
        Ok(())
    }
    fn write(&mut self, samples: &[u8]) -> Result<(), Error> {
        self.0 += samples.len() as u64;
        Ok(())
    }
}

fn bx(kind: &[u8], payload: &[u8]) -> Vec<u8> {
    let mut out = ((8 + payload.len()) as u32).to_be_bytes().to_vec();
    out.extend_from_slice(kind);
    out.extend_from_slice(payload);
    out
}

fn full(kind: &[u8], flags: u32, payload: &[u8]) -> Vec<u8> {
    let mut p = flags.to_be_bytes().to_vec();
    p.extend_from_slice(payload);
    bx(kind, &p)
}

fn crc8(d: &[u8]) -> u8 {
    let mut c = 0u8;
    for &b in d {
        c ^= b;
        for _ in 0..8 {
            c = if c & 0x80 != 0 { (c << 1) ^ 7 } else { c << 1 };
        }
    }
    c
}

fn crc16(d: &[u8]) -> u16 {
    let mut c = 0u16;
    for &b in d {
        c ^= u16::from(b) << 8;
        for _ in 0..8 {
            c = if c & 0x8000 != 0 {
                (c << 1) ^ 0x8005
            } else {
                c << 1
            };
        }
    }
    c
}

/// An MP4 file of FLAC samples of the given sizes, all at one offset: a
/// one-sample frame, then padding up to the largest.
fn file(sizes: &[u32]) -> Vec<u8> {
    let count = sizes.len() as u32;
    let size = sizes.iter().copied().max().unwrap_or(12);
    let mut frame = vec![0xFF, 0xF8, 0x60, 0x08, 0x00, 0x00];
    frame.push(crc8(&frame));
    frame.extend_from_slice(&[0x00, 0x12, 0x34]);
    let crc = crc16(&frame);
    frame.extend_from_slice(&crc.to_be_bytes());
    let mut packet = frame;
    packet.resize(size as usize, 0);
    let mut si = vec![0, 16, 0x10, 0, 0, 0, 0, 0, 0, 0];
    si.extend_from_slice(&((44_100u64 << 44) | (15 << 36)).to_be_bytes());
    si.extend_from_slice(&[0; 16]);
    let mut dfla = vec![0x80, 0, 0, 34];
    dfla.extend_from_slice(&si);
    let dfla = full(b"dfLa", 0, &dfla);
    let mut entry = vec![0; 6];
    entry.extend_from_slice(&1u16.to_be_bytes());
    entry.extend_from_slice(&[0; 8]);
    for v in [1u16, 16, 0, 0] {
        entry.extend_from_slice(&v.to_be_bytes());
    }
    entry.extend_from_slice(&(44_100u32 << 16).to_be_bytes());
    entry.extend_from_slice(&dfla);
    let mut stsd = 1u32.to_be_bytes().to_vec();
    stsd.extend(bx(b"fLaC", &entry));
    let words = |v: &[u32]| v.iter().flat_map(|x| x.to_be_bytes()).collect::<Vec<u8>>();
    let ftyp = bx(b"ftyp", b"isom\x00\x00\x02\x00isomiso2mp41");
    let moov = |offset: u32| {
        let mut stco = count.to_be_bytes().to_vec();
        for _ in 0..count {
            stco.extend_from_slice(&offset.to_be_bytes());
        }
        let mut stbl = full(b"stsd", 0, &stsd);
        stbl.extend(full(b"stts", 0, &words(&[1, count, 1])));
        stbl.extend(full(b"stsc", 0, &words(&[1, 1, 1, 1])));
        let mut stsz = words(&[0, count]);
        stsz.extend(words(sizes));
        stbl.extend(full(b"stsz", 0, &stsz));
        stbl.extend(full(b"stco", 0, &stco));
        let mut minf = full(b"smhd", 0, &[0; 4]);
        let mut dref = 1u32.to_be_bytes().to_vec();
        dref.extend(full(b"url ", 1, b""));
        minf.extend(bx(b"dinf", &full(b"dref", 0, &dref)));
        minf.extend(bx(b"stbl", &stbl));
        let mut hdlr = vec![0; 4];
        hdlr.extend_from_slice(b"soun");
        hdlr.extend_from_slice(&[0; 12]);
        hdlr.extend_from_slice(b"snd\0");
        let mut mdia = full(
            b"mdhd",
            0,
            &[words(&[0, 0, 44_100, count]), vec![0x55, 0xC4, 0, 0]].concat(),
        );
        mdia.extend(full(b"hdlr", 0, &hdlr));
        mdia.extend(bx(b"minf", &minf));
        let matrix = words(&[0x10000, 0, 0, 0, 0x10000, 0, 0, 0, 0x4000_0000]);
        let mut tkhd = words(&[0, 0, 1, 0, 0]);
        tkhd.extend_from_slice(&[0; 8]);
        tkhd.extend_from_slice(&[0, 0, 0, 0, 0x01, 0x00, 0, 0]);
        tkhd.extend_from_slice(&matrix);
        tkhd.extend_from_slice(&[0; 8]);
        let mut trak = full(b"tkhd", 3, &tkhd);
        trak.extend(bx(b"mdia", &mdia));
        let mut mvhd = words(&[0, 0, 1000, 0, 0x10000]);
        mvhd.extend_from_slice(&[0x01, 0x00]);
        mvhd.extend_from_slice(&[0; 10]);
        mvhd.extend_from_slice(&matrix);
        mvhd.extend_from_slice(&[0; 24]);
        mvhd.extend_from_slice(&2u32.to_be_bytes());
        let mut moov = full(b"mvhd", 0, &mvhd);
        moov.extend(bx(b"trak", &trak));
        bx(b"moov", &moov)
    };
    let head = ftyp.len() + moov(0).len() + 8;
    let mut out = ftyp;
    out.extend(moov(head as u32));
    out.extend_from_slice(&((8 + packet.len()) as u32).to_be_bytes());
    out.extend_from_slice(b"mdat");
    out.extend(packet);
    out
}

/// Peak resident memory of this process, in KiB.
fn peak_kib() -> u64 {
    let status = std::fs::read_to_string("/proc/self/status").unwrap();
    let line = status.lines().find(|l| l.starts_with("VmHWM:")).unwrap();
    line.split_whitespace().nth(1).unwrap().parse().unwrap()
}

#[test]
#[ignore = "release only, run by CI"]
fn large_packets_do_not_pile_up_in_the_pool() {
    run(&vec![15 << 20; 400], 400);
}

/// 15 MiB packets among runs of tiny ones: reused buffers must not keep
/// the large packets' memory outside the bound.
#[test]
#[ignore = "release only, run by CI"]
fn reused_buffers_do_not_keep_large_packets() {
    let mut sizes = Vec::new();
    let mut seed = 1u32;
    for _ in 0..300 {
        seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        sizes.extend(std::iter::repeat_n(12, (seed >> 16) as usize % 256));
        sizes.push(15 << 20);
    }
    let count = sizes.len() as u64;
    run(&sizes, count);
}

fn run(sizes: &[u32], samples: u64) {
    let data = file(sizes);
    let before = peak_kib();
    let mut sink = Count(0);
    let options = Options {
        threads: 4,
        ..Options::default()
    };
    let summary = decode(
        SliceSource::new(&data),
        &options,
        &Limits::default(),
        &mut sink,
    )
    .unwrap();
    assert_eq!(summary.samples, samples);
    let growth = peak_kib().saturating_sub(before);
    // 64 MiB in flight, a 15 MiB packet per thread being read or decoded,
    // a job, and allocator slack; without the byte bound it was 6 GB.
    assert!(growth < 256 << 10, "peak grew by {growth} KiB");
}

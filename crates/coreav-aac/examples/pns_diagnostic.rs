//! Bounded, opt-in packet spectra and untrimmed PCM for PNS qualification.

use coreav_aac::{Config, Decoder, Limits};
use coreav_isobmff::{self as mp4, Source};
use std::error::Error;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::Path;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn create(path: &Path) -> Result<BufWriter<File>> {
    Ok(BufWriter::new(
        OpenOptions::new().write(true).create_new(true).open(path)?,
    ))
}

fn adts(out: &mut impl Write, data: &[u8], rate: u8, channels: u8) -> Result<()> {
    let size = data
        .len()
        .checked_add(7)
        .filter(|&size| size <= 8191)
        .ok_or("ADTS packet limit")?;
    let header = [
        0xff,
        0xf1,
        0x40 | (rate << 2) | (channels >> 2),
        (channels << 6) | ((size >> 11) as u8),
        (size >> 3) as u8,
        ((size << 5) as u8) | 0x1f,
        0xfc,
    ];
    out.write_all(&header)?;
    out.write_all(data)?;
    Ok(())
}

#[derive(Default)]
struct Bits {
    data: Vec<u8>,
    used: usize,
}

impl Bits {
    fn put(&mut self, value: u32, width: u32) {
        for shift in (0..width).rev() {
            if self.used % 8 == 0 {
                self.data.push(0);
            }
            if let Some(byte) = self.data.last_mut() {
                *byte |= (((value >> shift) & 1) as u8) << (7 - self.used % 8);
            }
            self.used += 1;
        }
    }
}

fn zero_packet(sequences: &[u8]) -> Vec<u8> {
    let mut bits = Bits::default();
    bits.put(u32::from(sequences.len() == 2), 3);
    bits.put(0, 4);
    if sequences.len() == 2 {
        bits.put(0, 1);
    }
    for &sequence in sequences {
        bits.put(100, 8);
        bits.put(0, 1);
        bits.put(u32::from(sequence), 2);
        bits.put(0, 1);
        if sequence == 2 {
            bits.put(0, 4);
            bits.put(0, 7);
        } else {
            bits.put(0, 6);
            bits.put(0, 1);
        }
        bits.put(0, 3);
    }
    bits.put(7, 3);
    bits.data
}

fn legal(previous: u8, next: u8) -> bool {
    matches!((previous, next), (0 | 3, 0 | 1) | (1, 2) | (2, 2 | 3))
}

fn pcm(out: &mut impl Write, samples: &[f32], frame: usize) -> Result<()> {
    if samples.iter().any(|sample| !sample.is_finite()) {
        return Err("nonfinite native samples".into());
    }
    for index in 0..frame {
        for channel in samples.chunks_exact(frame) {
            out.write_all(&channel.get(index).ok_or("PCM extent")?.to_le_bytes())?;
        }
    }
    Ok(())
}

fn run(input: &Path, directory: &Path) -> Result<()> {
    let limits = mp4::Limits {
        max_packet_bytes: 16 << 20,
        ..mp4::Limits::default()
    };
    let movie = mp4::probe(
        mp4::FileSource::open(input)?,
        &limits,
        &mp4::Allowlist::new(&[mp4::Codec::Aac]),
    )?;
    if movie.has_fragments() {
        return Err("fragmented MP4 is outside qualification scope".into());
    }
    let track = movie
        .tracks
        .iter()
        .find(|track| track.kind == mp4::TrackKind::Audio)
        .ok_or("no audio track")?;
    let asc = track
        .description
        .codec_config
        .as_ref()
        .and_then(|config| mp4::esds_decoder_specific_info(&config.data))
        .and_then(|(_, config)| config)
        .ok_or("missing AAC configuration")?;
    let config = Config::parse(asc, &Limits { max_channels: 2 })?;
    if config.frame_length() != 1024 || !matches!(config.channel_configuration(), 1 | 2) {
        return Err("diagnostic requires explicit 1024-sample mono/stereo LC".into());
    }
    let rate = [
        96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350,
    ]
    .iter()
    .position(|&rate| rate == config.sample_rate())
    .ok_or("ADTS sample rate")? as u8;
    let channels = config.channels();
    let mut decoder = Decoder::new(&config);
    let mut samples = vec![0.0f32; 1024 * channels];
    let mut sequence = vec![0; channels];
    let mut history_valid = true;
    let (mut tns, mut stereo, mut coupling, mut all_ms) = (false, false, false, false);
    let mut metadata = create(&directory.join("metadata.json"))?;
    let mut spectra = create(&directory.join("spectra.f32"))?;
    let mut native = create(&directory.join("native.f32"))?;
    let mut transport = create(&directory.join("packets.aac"))?;
    write!(
        metadata,
        "{{\"schema\":\"coreav.aac-pns-diagnostic/1\",\"rate\":{},\"channels\":{},\"frame_length\":1024,\"frames\":[",
        config.sample_rate(),
        channels
    )?;
    let mut budget = mp4::Budget::new(&limits);
    let mut packets = movie.packets(
        track.index,
        mp4::FileSource::open(input)?,
        &limits,
        &mut budget,
    )?;
    let mut data = Vec::new();
    let (mut count, mut bytes) = (0u64, 0u64);
    while let Some(packet) = packets.next() {
        let packet = packet?;
        count += 1;
        bytes = bytes
            .checked_add(u64::from(packet.size))
            .ok_or("encoded data overflow")?;
        if count > 16_384 || bytes > 64 << 20 || packet.size > 16 << 20 {
            return Err("diagnostic packet budget".into());
        }
        let size = usize::try_from(packet.size)?;
        data.try_reserve(size.saturating_sub(data.len()))?;
        data.resize(size, 0);
        packets
            .source_mut()
            .read_exact_at(packet.offset, &mut data)?;
        adts(&mut transport, &data, rate, channels as u8)?;
        let frame = decoder.decode(&data, &mut samples)?;
        tns |= frame.pns_tns_bands > 0;
        stereo |= frame.pns_stereo_bands > 0;
        coupling |= frame.coupling_channels > 0;
        all_ms |= frame.intensity_all_ms_bands > 0;
        for (previous, next) in sequence.iter_mut().zip(decoder.window_sequences()) {
            history_valid &= legal(*previous, next);
            *previous = next;
        }
        pcm(&mut native, &samples, 1024)?;
        if count > 1 {
            write!(metadata, ",")?;
        }
        write!(metadata, "{{\"channels\":[")?;
        for (index, channel) in decoder.diagnostic().iter().enumerate() {
            if index > 0 {
                write!(metadata, ",")?;
            }
            write!(
                metadata,
                "{{\"sequence\":{},\"shape\":{},\"noise\":[",
                channel.sequence, channel.shape
            )?;
            for (index, band) in channel.noise.iter().enumerate() {
                if index > 0 {
                    write!(metadata, ",")?;
                }
                write!(
                    metadata,
                    "{{\"start\":{},\"end\":{},\"sf\":{}}}",
                    band.start, band.end, band.sf
                )?;
            }
            write!(metadata, "]}}")?;
            for value in &channel.spectrum {
                spectra.write_all(&value.to_le_bytes())?;
            }
        }
        write!(metadata, "]}}")?;
    }
    if count == 0 || count != u64::from(track.sample_count) {
        return Err("incomplete packet listing".into());
    }
    let padding = if sequence.contains(&1) { 2 } else { 1 };
    write!(metadata, "],\"padding\":[")?;
    for index in 0..padding {
        for current in &mut sequence {
            *current = match *current {
                1 => 2,
                2 => 3,
                _ => 0,
            };
        }
        let data = zero_packet(&sequence);
        adts(&mut transport, &data, rate, channels as u8)?;
        decoder.decode(&data, &mut samples)?;
        pcm(&mut native, &samples, 1024)?;
        if index > 0 {
            write!(metadata, ",")?;
        }
        write!(metadata, "[")?;
        for (index, channel) in decoder.diagnostic().iter().enumerate() {
            if channel.spectrum.iter().any(|&value| value != 0.0) || !channel.noise.is_empty() {
                return Err("padding spectrum is not zero".into());
            }
            if index > 0 {
                write!(metadata, ",")?;
            }
            write!(
                metadata,
                "{{\"sequence\":{},\"shape\":{}}}",
                channel.sequence, channel.shape
            )?;
        }
        write!(metadata, "]")?;
    }
    write!(
        metadata,
        "],\"packets\":{count},\"padding_packets\":{padding},\"unsafe\":["
    )?;
    let mut comma = "";
    for (present, label) in [
        (tns, "tns_pns"),
        (stereo, "stereo_pns"),
        (coupling, "coupling"),
        (all_ms, "all_ms_intensity"),
        (!history_valid, "window_history"),
    ] {
        if present {
            write!(metadata, "{comma}\"{label}\"")?;
            comma = ",";
        }
    }
    writeln!(metadata, "]}}")?;
    metadata.flush()?;
    spectra.flush()?;
    native.flush()?;
    transport.flush()?;
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let [input, directory] = args.as_slice() else {
        return Err("expected INPUT OUTPUT_DIRECTORY".into());
    };
    run(Path::new(input), Path::new(directory))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn closing_packets_have_zero_spectra_and_all_window_sequences() {
        for (asc, channels) in [([0x12, 0x08], 1), ([0x12, 0x10], 2)] {
            let config = Config::parse(&asc, &Limits { max_channels: 2 }).unwrap();
            let mut decoder = Decoder::new(&config);
            let mut samples = vec![0.0; 1024 * channels];
            for sequence in [0, 1, 2, 2, 3, 0] {
                let packet = zero_packet(&vec![sequence; channels]);
                decoder.decode(&packet, &mut samples).unwrap();
                assert!(samples.iter().all(|&sample| sample == 0.0));
                assert!(
                    decoder
                        .diagnostic()
                        .iter()
                        .all(|channel| channel.sequence == sequence
                            && channel.noise.is_empty()
                            && channel.spectrum.iter().all(|&value| value == 0.0))
                );
            }
        }
    }
}

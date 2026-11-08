use super::{Shared, overflow};
use crate::ExtractError;
use coreav_audio::flac::{FrameHeader, crc16_update};
use coreav_isobmff::{Error, Packet, Source};
use std::cell::RefCell;
use std::io::{self, Write};

pub(super) struct Plan {
    pub(super) minimum: u16,
    pub(super) maximum: u16,
    pub(super) samples: u64,
}

pub(super) struct Survey {
    samples: u64,
    frames: u64,
    previous: Option<FrameHeader>,
    nominal: Option<u32>,
    minimum: u32,
    maximum: u32,
    rewrite: bool,
}

impl Survey {
    pub(super) fn new() -> Self {
        Self {
            samples: 0,
            frames: 0,
            previous: None,
            nominal: None,
            minimum: u32::MAX,
            maximum: 0,
            rewrite: false,
        }
    }

    pub(super) fn push(&mut self, head: FrameHeader) -> Option<()> {
        if let Some(previous) = self.previous {
            self.minimum = self.minimum.min(previous.block_size);
            self.rewrite |= previous.variable != head.variable
                || (!previous.variable && self.nominal != Some(previous.block_size));
        }
        self.rewrite |= head.number
            != if head.variable {
                self.samples
            } else {
                self.frames
            };
        self.nominal.get_or_insert(head.block_size);
        self.maximum = self.maximum.max(head.block_size);
        self.samples = self.samples.checked_add(u64::from(head.block_size))?;
        self.frames = self.frames.checked_add(1)?;
        self.previous = Some(head);
        Some(())
    }

    pub(super) fn finish(mut self) -> Option<Plan> {
        self.rewrite |= self.previous.is_some_and(|head| {
            !head.variable && self.nominal.is_some_and(|block| head.block_size > block)
        });
        if !self.rewrite || self.minimum < 16 || self.samples >= 1 << 36 || self.maximum == 0 {
            return None;
        }
        if self.frames == 1 {
            self.minimum = 16;
            self.maximum = self.maximum.max(16);
        }
        if self.minimum == self.maximum {
            self.minimum = 16;
            self.maximum = self.maximum.max(17);
        }
        Some(Plan {
            minimum: u16::try_from(self.minimum).ok()?,
            maximum: u16::try_from(self.maximum).ok()?,
            samples: self.samples,
        })
    }
}

pub(super) fn header<S: Source>(
    source: &RefCell<S>,
    packet: &Packet,
) -> Result<Option<FrameHeader>, Error> {
    let mut bytes = [0; 16];
    let bytes = bytes
        .get_mut(..(packet.size as usize).min(16))
        .ok_or_else(overflow)?;
    Shared(source).read_exact_at(packet.offset, bytes)?;
    Ok(FrameHeader::parse(bytes).filter(|head| head.bytes().len() + 2 <= packet.size as usize))
}

pub(super) struct Frame<'a, W> {
    out: &'a mut W,
    header: FrameHeader,
    skipped: usize,
    body: usize,
    footer: [u8; 2],
    footer_len: usize,
    original_crc: u16,
    rewritten_crc: u16,
}

impl<'a, W: Write> Frame<'a, W> {
    pub(super) fn new(
        out: &'a mut W,
        header: FrameHeader,
        samples: u64,
        size: u32,
    ) -> Result<Self, ExtractError> {
        let mut bytes = [0; 16];
        let len = header
            .renumber(samples, &mut bytes)
            .ok_or(ExtractError::SourceChanged)?;
        let bytes = bytes.get(..len).ok_or(ExtractError::SourceChanged)?;
        let body = (size as usize)
            .checked_sub(header.bytes().len() + 2)
            .ok_or(ExtractError::SourceChanged)?;
        out.write_all(bytes)?;
        Ok(Self {
            out,
            header,
            skipped: 0,
            body,
            footer: [0; 2],
            footer_len: 0,
            original_crc: 0,
            rewritten_crc: crc16_update(0, bytes),
        })
    }

    pub(super) fn finish(self) -> Result<(), ExtractError> {
        if self.skipped != self.header.bytes().len() || self.body != 0 || self.footer_len != 2 {
            return Err(ExtractError::SourceChanged);
        }
        let stored = u16::from_be_bytes(self.footer);
        self.out
            .write_all(&(stored ^ self.original_crc ^ self.rewritten_crc).to_be_bytes())?;
        Ok(())
    }
}

impl<W: Write> Write for Frame<'_, W> {
    fn write(&mut self, mut bytes: &[u8]) -> io::Result<usize> {
        let len = bytes.len();
        let invalid = || io::Error::from(io::ErrorKind::InvalidData);
        let skip = bytes
            .len()
            .min(self.header.bytes().len().saturating_sub(self.skipped));
        let (prefix, rest) = bytes.split_at_checked(skip).ok_or_else(invalid)?;
        if self.header.bytes().get(self.skipped..self.skipped + skip) != Some(prefix) {
            return Err(invalid());
        }
        self.original_crc = crc16_update(self.original_crc, prefix);
        self.skipped += skip;
        bytes = rest;
        let take = bytes.len().min(self.body);
        let (body, rest) = bytes.split_at_checked(take).ok_or_else(invalid)?;
        self.original_crc = crc16_update(self.original_crc, body);
        self.rewritten_crc = crc16_update(self.rewritten_crc, body);
        self.out.write_all(body)?;
        self.body -= take;
        let end = self
            .footer_len
            .checked_add(rest.len())
            .ok_or_else(invalid)?;
        self.footer
            .get_mut(self.footer_len..end)
            .ok_or_else(invalid)?
            .copy_from_slice(rest);
        self.footer_len = end;
        Ok(len)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.out.flush()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic)]
mod tests {
    use super::*;
    use coreav_audio::flac::{FlacDecoder, StreamInfo, encode::FlacSink};
    use coreav_audio::{Limits as AudioLimits, OutputFormat, Pcm, Plane, SampleFormat, Sink};

    fn encoded(count: usize, seed: u32, format: SampleFormat) -> (StreamInfo, Vec<u8>, Vec<i32>) {
        let values: Vec<i32> = (0..count * 2)
            .map(|index| {
                let value = (index as u32).wrapping_mul(7919).wrapping_add(seed * 131) as i32;
                if format == SampleFormat::S16 {
                    (value as i16) as i32
                } else {
                    (value << 8) >> 8
                }
            })
            .collect();
        let pcm: Vec<u8> = values
            .iter()
            .flat_map(|value| value.to_le_bytes()[..format.bytes()].to_vec())
            .collect();
        let mut bytes = Vec::new();
        let mut sink = FlacSink::new(&mut bytes, AudioLimits::default());
        sink.start(&OutputFormat {
            channels: 2,
            sample_rate: 48000,
            format,
            channel_mask: 3,
        })
        .unwrap();
        sink.write(&pcm).unwrap();
        let header = sink.finish().unwrap();
        let info = StreamInfo::parse(&header[8..42]).unwrap();
        (info, bytes[header.len()..].to_vec(), values)
    }

    fn interleaved(pcm: &Pcm) -> Vec<i32> {
        let planes: Vec<&[i32]> = (0..pcm.channels())
            .map(|channel| match pcm.plane(channel).unwrap() {
                Plane::Int { samples, .. } => samples,
                _ => panic!("integer PCM expected"),
            })
            .collect();
        (0..pcm.frames())
            .flat_map(|index| planes.iter().map(move |plane| plane[index]))
            .collect()
    }

    #[test]
    fn rewritten_concatenations_decode_to_every_original_sample() {
        for format in [SampleFormat::S16, SampleFormat::S24] {
            for count in [16, 17, 127, 128, 256, 4096, 4112, 8191, 8192] {
                let mut samples = 0;
                let mut expected = Vec::new();
                let mut actual = Vec::new();
                for seed in 0..3 {
                    let (info, bytes, values) = encoded(count, seed, format);
                    expected.extend(values);
                    let mut original = FlacDecoder::new(info, &AudioLimits::default()).unwrap();
                    let mut decoder = FlacDecoder::new(info, &AudioLimits::default()).unwrap();
                    let mut pcm = Pcm::new();
                    let mut rest = bytes.as_slice();
                    while !rest.is_empty() {
                        let frame = original.decode(rest, &mut pcm).unwrap();
                        assert!(frame.crc_ok);
                        let packet = &rest[..frame.bytes];
                        let head = FrameHeader::parse(packet).unwrap();
                        let mut out = Vec::new();
                        let mut writer =
                            Frame::new(&mut out, head, samples, packet.len() as u32).unwrap();
                        for chunk in packet.chunks((seed as usize + 1) * 7) {
                            writer.write_all(chunk).unwrap();
                        }
                        writer.finish().unwrap();
                        let rewritten = decoder.decode(&out, &mut pcm).unwrap();
                        assert!(rewritten.variable && rewritten.crc_ok);
                        assert_eq!(rewritten.bytes, out.len());
                        assert_eq!(rewritten.number, samples);
                        assert_eq!(
                            out.len() as u32,
                            head.rewritten_size(packet.len() as u32, samples).unwrap()
                        );
                        actual.extend(interleaved(&pcm));
                        samples += u64::from(frame.block_size);
                        rest = &rest[frame.bytes..];
                    }
                }
                assert_eq!(actual, expected, "{count} samples per input");
            }
        }
    }

    #[test]
    fn every_write_split_preserves_crc_error_syndromes() {
        let (info, mut packet, _) = encoded(32, 1, SampleFormat::S16);
        for corrupt in [false, true] {
            if corrupt {
                let len = packet.len();
                packet[len - 1] ^= 1;
            }
            let head = FrameHeader::parse(&packet).unwrap();
            for split in 0..=packet.len() {
                let mut out = Vec::new();
                let mut writer = Frame::new(&mut out, head, 1 << 31, packet.len() as u32).unwrap();
                writer.write_all(&packet[..split]).unwrap();
                writer.write_all(&packet[split..]).unwrap();
                writer.finish().unwrap();
                let mut decoder = FlacDecoder::new(info, &AudioLimits::default()).unwrap();
                assert_eq!(
                    decoder.decode(&out, &mut Pcm::new()).unwrap().crc_ok,
                    !corrupt
                );
                assert_eq!(crc16_update(0, &packet), crc16_update(0, &out));
            }
        }
    }

    #[test]
    fn changed_headers_and_incomplete_packets_fail_closed() {
        let (_, packet, _) = encoded(32, 0, SampleFormat::S16);
        let head = FrameHeader::parse(&packet).unwrap();
        let mut out = Vec::new();
        let mut writer = Frame::new(&mut out, head, 0, packet.len() as u32).unwrap();
        assert!(writer.write_all(&[0]).is_err());
        let mut writer = Frame::new(&mut out, head, 0, packet.len() as u32).unwrap();
        writer.write_all(&packet[..packet.len() - 1]).unwrap();
        assert!(writer.finish().is_err());
    }

    #[test]
    fn continuous_streams_keep_headers_and_discontinuities_rewrite_all_frames() {
        let (_, packet, _) = encoded(32, 0, SampleFormat::S16);
        let base = FrameHeader::parse(&packet).unwrap();
        for variable in [false, true] {
            for blocks in [[4096, 4096, 768], [16, 16, 16]] {
                let mut survey = Survey::new();
                let mut samples = 0;
                for (frame, block) in blocks.into_iter().enumerate() {
                    let mut head = base;
                    head.variable = variable;
                    head.block_size = block;
                    head.number = if variable { samples } else { frame as u64 };
                    samples += u64::from(block);
                    survey.push(head).unwrap();
                }
                assert!(survey.finish().is_none());
            }
        }
        for blocks in [[4096, 768, 4096, 768], [16, 16, 16, 16]] {
            let mut survey = Survey::new();
            for (frame, block) in blocks.into_iter().enumerate() {
                let mut head = base;
                head.block_size = block;
                head.number = (frame % 2) as u64;
                survey.push(head).unwrap();
            }
            let plan = survey.finish().unwrap();
            assert!(plan.minimum < plan.maximum);
            assert!(plan.minimum >= 16);
            assert_eq!(plan.samples, blocks.into_iter().map(u64::from).sum::<u64>());
        }
        let mut survey = Survey::new();
        for (frame, block) in [4096, 768, 4096].into_iter().enumerate() {
            let mut head = base;
            head.number = frame as u64;
            head.block_size = block;
            survey.push(head).unwrap();
        }
        assert_eq!(survey.finish().unwrap().minimum, 768);
    }

    #[test]
    fn a_single_repaired_frame_has_legal_representable_bounds() {
        let (_, packet, _) = encoded(32, 0, SampleFormat::S16);
        let base = FrameHeader::parse(&packet).unwrap();
        assert!(Survey::new().finish().is_none());
        for block in [1, 15, 16, 128, 65535] {
            let mut head = base;
            head.block_size = block;
            head.number = 1;
            let mut survey = Survey::new();
            survey.push(head).unwrap();
            let plan = survey.finish().unwrap();
            assert!(plan.minimum >= 16 && plan.minimum < plan.maximum);
            assert!(u32::from(plan.maximum) >= block);
            assert_eq!(plan.samples, u64::from(block));
        }
    }
}

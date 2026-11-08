//! Opt-in scalar Main/Main10 HEVC at the shared packet decode boundary.

#![forbid(unsafe_code)]
#![cfg(feature = "native-hevc")]

mod configuration;
mod output;

use std::time::Instant;

use coreav_decode::{
    Backend, BackendKind, Codec, Decoder, DecoderInfo, Error, ErrorKind, Isolation, Limits, Packet,
    StreamConfig, StreamTraits, Support,
};
use coreav_frame::{Chroma, Frame};
use coreav_hevc::{AnnexB, DecodeLimits, LengthPrefixed, PictureDecoder};

use configuration::{Configuration, Framing};
use output::Output;

const MAX_NALS: usize = 4096;
const DECODER_NAME: &str = "coreav hevc, scalar Main/Main10";

/// Scalar native eight-bit Main and eight- or ten-bit Main10 decoding.
#[derive(Clone, Copy, Debug, Default)]
pub struct NativeHevc {
    options: NativeOptions,
}

/// Reconstruction options for the opt-in native decoder.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NativeOptions {
    /// Reconstruction workers; `None` selects available host parallelism.
    /// Counts are bounded to one through sixteen and reduced to fit memory.
    pub threads: Option<usize>,
}

impl NativeHevc {
    /// Creates the opt-in backend without activating any decoder session.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a backend with caller-selected reconstruction options.
    pub fn with_options(options: NativeOptions) -> Self {
        Self { options }
    }
}

impl Backend for NativeHevc {
    fn kind(&self) -> BackendKind {
        BackendKind::Native
    }

    fn supports(&self, stream: &StreamConfig, traits: &StreamTraits) -> Support {
        match scope(stream, traits).and_then(|()| Configuration::parse(stream).map(|_| ())) {
            Ok(()) => Support::Yes,
            Err(error) => Support::No(error.message),
        }
    }

    fn open(&self, stream: &StreamConfig, limits: &Limits) -> Result<Box<dyn Decoder>, Error> {
        scope(stream, &stream.traits())?;
        let configuration = Configuration::parse(stream)?;
        if stream.width.is_some_and(|width| width > limits.max_width)
            || stream
                .height
                .is_some_and(|height| height > limits.max_height)
        {
            return Err(limit("declared HEVC dimensions exceed the frame limit"));
        }
        if let (Some(width), Some(height)) = (stream.width, stream.height) {
            limits.check_frame(width, height, 0)?;
        }
        let defaults = coreav_hevc::Limits::default();
        let deadline = limits.deadline();
        let threads = self
            .options
            .threads
            .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, usize::from));
        let mut decoder = PictureDecoder::new(DecodeLimits {
            headers: coreav_hevc::Limits {
                max_width: limits.max_width.min(defaults.max_width),
                max_height: limits.max_height.min(defaults.max_height),
                max_pixels: limits.max_pixels.min(defaults.max_pixels),
                max_reference_frames: limits
                    .max_reference_frames
                    .min(defaults.max_reference_frames),
                max_nal_bytes: usize::try_from(limits.max_packet_bytes)
                    .unwrap_or(usize::MAX)
                    .min(defaults.max_nal_bytes),
                ..defaults
            },
            max_memory_bytes: usize::try_from(limits.max_memory_bytes).unwrap_or(usize::MAX),
            max_frame_bytes: limits.max_frame_bytes,
            deadline: Some(deadline),
            ..DecodeLimits::default()
        })
        .with_threads(threads);
        if (core::mem::size_of::<NativeDecoder>() as u64).saturating_add(DECODER_NAME.len() as u64)
            > limits.max_memory_bytes
        {
            return Err(limit("HEVC owner exceeds the memory budget"));
        }
        let mut name = String::new();
        name.try_reserve_exact(DECODER_NAME.len())
            .map_err(|_| limit("HEVC decoder name allocation"))?;
        name.push_str(DECODER_NAME);
        let output = Output::new(limits.clone(), deadline, stream.color, name.capacity())?;
        decoder
            .set_external_memory(output.retained_bytes()?)
            .map_err(codec_error)?;
        configuration.feed(|nal| {
            check_deadline(deadline)?;
            decoder
                .read_nal(nal, None, |_| {
                    Err(coreav_hevc::Error::InvalidData("configuration_output"))
                })
                .map_err(codec_error)
        })?;
        output.check()?;
        Ok(Box::new(NativeDecoder {
            decoder: Some(decoder),
            info: DecoderInfo {
                backend: BackendKind::Native,
                isolation: Isolation::InProcess,
                decoder: name,
            },
            framing: configuration.framing,
            output,
            state: State::Active,
            next_origin: 0,
        }))
    }
}

fn scope(stream: &StreamConfig, traits: &StreamTraits) -> Result<(), Error> {
    if stream.codec != Codec::Hevc {
        return Err(Error::new(
            ErrorKind::Unsupported,
            "this backend decodes HEVC only",
        ));
    }
    if traits
        .bit_depth
        .is_some_and(|depth| !matches!(depth, 8 | 10))
        || traits.chroma.is_some_and(|chroma| chroma != Chroma::Cs420)
    {
        return Err(Error::new(
            ErrorKind::Unsupported,
            "native HEVC requires eight- or ten-bit 4:2:0",
        ));
    }
    Ok(())
}

#[derive(PartialEq, Eq)]
enum State {
    Active,
    Draining,
    Ended,
    Failed(Error),
}

struct NativeDecoder {
    decoder: Option<PictureDecoder>,
    info: DecoderInfo,
    framing: Framing,
    output: Output,
    state: State,
    next_origin: u64,
}

struct RowOutput<'a> {
    output: &'a mut Output,
    error: Option<(usize, Error)>,
}

impl RowOutput<'_> {
    fn capture<T>(&mut self, result: Result<T, Error>, offset: usize) -> coreav_hevc::Result<T> {
        result.map_err(|error| {
            if self
                .error
                .as_ref()
                .is_none_or(|(earlier, _)| *earlier >= offset)
            {
                self.error = Some((offset, error));
            }
            coreav_hevc::Error::Limit("boundary_output")
        })
    }

    fn finish(self, error: coreav_hevc::Error) -> Error {
        if error == coreav_hevc::Error::Limit("boundary_output") {
            self.error
                .map_or_else(|| codec_error(error), |(_, error)| error)
        } else {
            codec_error(error)
        }
    }
}

impl coreav_hevc::native_rows::RowSink for RowOutput<'_> {
    fn prepare(
        &mut self,
        header: coreav_hevc::native_rows::OutputHeader,
        memory: &mut coreav_hevc::OutputMemory<'_>,
    ) -> coreav_hevc::Result<bool> {
        let offset = self.output.ready.len();
        let result = self.output.prepare(header, memory);
        self.capture(result, offset)
    }

    fn fill(
        &mut self,
        offset: usize,
        picture: coreav_hevc::DecodedPicture<'_>,
    ) -> coreav_hevc::Result<()> {
        let result = self.output.fill(offset, picture);
        self.capture(result, offset)
    }

    fn truncate(&mut self, keep: usize) {
        self.output.truncate(keep);
    }

    fn cost(
        &self,
        debt: usize,
        frame_bytes: usize,
    ) -> coreav_hevc::Result<coreav_hevc::native_rows::SinkCost> {
        self.output.cost(debt, frame_bytes)
    }
}

impl NativeDecoder {
    fn terminal_result<T>(&mut self, result: Result<T, Error>) -> Result<T, Error> {
        let Err(error) = result else {
            return result;
        };
        if let State::Failed(error) = &self.state {
            return Err(error.clone());
        }
        let pending = match &mut self.decoder {
            Some(decoder) => {
                let mut sink = RowOutput {
                    output: &mut self.output,
                    error: None,
                };
                decoder
                    .settle_rows(&mut sink)
                    .map_err(|error| sink.finish(error))
            }
            None => Ok(()),
        };
        let error = pending.err().unwrap_or(error);
        self.decoder = None;
        self.state = State::Failed(error.clone());
        Err(error)
    }

    fn send(&mut self, packet: &Packet<'_>) -> Result<(), Error> {
        if self.state != State::Active {
            return Err(invalid("the HEVC decode request is closed"));
        }
        self.output.check()?;
        self.output.limits.check_packet(packet)?;
        let origin = self
            .next_origin
            .checked_mul(2)
            .and_then(|n| n.checked_add(u64::from(packet.discard)))
            .ok_or(limit("HEVC packet identity overflow"))?;
        self.next_origin = self
            .next_origin
            .checked_add(1)
            .ok_or(limit("HEVC packet identity overflow"))?;
        let decoder = self
            .decoder
            .as_mut()
            .ok_or(invalid("the HEVC decoder is closed"))?;
        let mut count = 0usize;
        let mut feed = |nal: coreav_hevc::Result<&[u8]>| -> Result<(), Error> {
            count = count
                .checked_add(1)
                .filter(|n| *n <= MAX_NALS)
                .ok_or(limit("too many HEVC NALs in one packet"))?;
            self.output.check()?;
            let nal = coreav_hevc::packet_nal(
                nal.map_err(codec_error)?,
                usize::try_from(self.output.limits.max_packet_bytes)
                    .unwrap_or(usize::MAX)
                    .min(coreav_hevc::Limits::default().max_nal_bytes),
            )
            .map_err(codec_error)?
            .0;
            let mut sink = RowOutput {
                output: &mut self.output,
                error: None,
            };
            decoder
                .read_nal_rows(nal, packet.pts, Some(origin), &mut sink)
                .map_err(|error| sink.finish(error))
        };
        match self.framing {
            Framing::AnnexB => {
                for nal in AnnexB::new(packet.data) {
                    feed(nal)?;
                }
            }
            Framing::Length(width) => {
                for nal in LengthPrefixed::new(packet.data, width).map_err(codec_error)? {
                    feed(nal)?;
                }
            }
        }
        if count == 0 {
            return Err(invalid("empty HEVC access unit"));
        }
        self.output.check()
    }

    fn eof(&mut self) -> Result<(), Error> {
        match self.state {
            State::Failed(ref error) => return Err(error.clone()),
            State::Ended => return Ok(()),
            State::Draining => return self.output.check(),
            State::Active => {}
        }
        self.output.check()?;
        let decoder = self
            .decoder
            .as_mut()
            .ok_or(invalid("the HEVC decoder is closed"))?;
        let mut sink = RowOutput {
            output: &mut self.output,
            error: None,
        };
        decoder
            .finish_rows(&mut sink)
            .map_err(|error| sink.finish(error))?;
        self.output.check()?;
        if !decoder.has_native_rows() {
            self.decoder = None;
        }
        self.state = if self.output.is_empty() {
            State::Ended
        } else {
            State::Draining
        };
        Ok(())
    }

    fn receive(&mut self) -> Result<Option<Frame>, Error> {
        match self.state {
            State::Failed(ref error) => {
                return self.output.pop().map(Some).ok_or_else(|| error.clone());
            }
            State::Ended => return Ok(None),
            State::Active | State::Draining => {}
        }
        self.output.check()?;
        let decision = self
            .decoder
            .as_mut()
            .map(PictureDecoder::receive_rows)
            .transpose()
            .map_err(codec_error)?
            .flatten();
        let frame = match decision {
            Some(decision) => {
                if let Some(offset) = decision.consume {
                    self.output.consume(offset)?;
                }
                if decision.pop {
                    let frame = self
                        .output
                        .pop_rows()
                        .ok_or(invalid("missing validated HEVC output"))?;
                    self.decoder
                        .as_mut()
                        .ok_or(invalid("the HEVC decoder is closed"))?
                        .release_rows_memory(self.output.retained_bytes()?)
                        .map_err(codec_error)?;
                    Some(frame)
                } else {
                    None
                }
            }
            None => {
                let frame = self.output.pop();
                if let Some(decoder) = &mut self.decoder {
                    decoder
                        .set_external_memory(self.output.retained_bytes()?)
                        .map_err(codec_error)?;
                }
                frame
            }
        };
        if self.state == State::Draining && self.output.is_empty() {
            self.decoder = None;
            self.state = State::Ended;
        }
        Ok(frame)
    }
}

impl Decoder for NativeDecoder {
    fn send_packet(&mut self, packet: &Packet<'_>) -> Result<(), Error> {
        let result = self.send(packet);
        self.terminal_result(result)
    }

    fn send_eof(&mut self) -> Result<(), Error> {
        let result = self.eof();
        self.terminal_result(result)
    }

    fn receive_frame(&mut self) -> Result<Option<Frame>, Error> {
        let result = self.receive();
        self.terminal_result(result)
    }

    fn info(&self) -> &DecoderInfo {
        &self.info
    }

    fn settle_native_hevc_refusal(&mut self, refusal: Error) -> Error {
        match self.terminal_result::<core::convert::Infallible>(Err(refusal)) {
            Err(error) => error,
            Ok(never) => match never {},
        }
    }

    fn take_native_hevc_terminal_output(
        &mut self,
    ) -> Option<coreav_decode::native_hevc_private::TerminalOutput> {
        if !matches!(self.state, State::Failed(_)) {
            return None;
        }
        let State::Failed(error) = std::mem::replace(&mut self.state, State::Ended) else {
            return None;
        };
        Some(coreav_decode::native_hevc_private::TerminalOutput {
            frames: std::mem::take(&mut self.output.ready),
            error,
        })
    }
}

fn invalid(message: &'static str) -> Error {
    Error::new(ErrorKind::InvalidData, message)
}
fn limit(message: &'static str) -> Error {
    Error::new(ErrorKind::Limit, message)
}

fn check_deadline(deadline: Instant) -> Result<(), Error> {
    if Instant::now() >= deadline {
        return Err(Error::new(
            ErrorKind::Timeout,
            "the HEVC decode deadline passed",
        ));
    }
    Ok(())
}

fn codec_error(error: coreav_hevc::Error) -> Error {
    let kind = match error {
        coreav_hevc::Error::Internal(_) => ErrorKind::Backend,
        coreav_hevc::Error::Limit("decode_deadline") => ErrorKind::Timeout,
        coreav_hevc::Error::Limit(_) => ErrorKind::Limit,
        coreav_hevc::Error::Unsupported(_) => ErrorKind::Unsupported,
        coreav_hevc::Error::InvalidData(_) => ErrorKind::InvalidData,
    };
    Error::new(kind, error.to_string())
}

#[cfg(test)]
mod tests;

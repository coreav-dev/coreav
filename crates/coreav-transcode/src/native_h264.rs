//! Bounded native H.264 access units for the existing transcode spool and muxers.

use coreav_frame::{ChromaLocation, ColorInfo, PixelFormat, Pixels};
use coreav_h264_encoder::{Colour, Deblocking, Gop, Limits, Plane, Settings};

use crate::Error;
use crate::encoder::{EncoderSettings, Sink};

pub use coreav_h264_encoder::RateMode;

/// Explicit preference for the opt-in native encoder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeH264 {
    /// Integer CRF or fixed QP, from 10 through 51.
    pub rate: RateMode,
}

pub(crate) fn color(color: ColorInfo) -> ColorInfo {
    ColorInfo {
        primaries: Some(color.primaries.unwrap_or(2)),
        transfer: Some(color.transfer.unwrap_or(2)),
        matrix: Some(color.matrix.unwrap_or(2)),
        full_range: Some(color.full_range.unwrap_or(false)),
        chroma_location: Some(ChromaLocation::Left),
    }
}

pub(crate) struct Encoder<S> {
    encoder: coreav_h264_encoder::Encoder,
    sink: S,
    frames: u64,
    size: (u32, u32),
}

impl<S: Sink> Encoder<S> {
    pub(crate) fn new(
        settings: &EncoderSettings,
        request: NativeH264,
        max_bytes: u64,
        sink: S,
    ) -> Result<Self, Error> {
        if settings.format != PixelFormat::YUV420P
            || settings.sample_aspect.is_some_and(|(n, d)| n != d)
        {
            return Err(Error::NativeIneligible(
                "native H.264 needs square 8-bit 4:2:0 pixels",
            ));
        }
        let c = settings.color;
        let settings_native = Settings {
            width: settings.width as usize,
            height: settings.height as usize,
            rate_mode: request.rate,
            colour: Colour {
                full_range: c.full_range.unwrap_or(false),
                primaries: c.primaries.unwrap_or(2),
                transfer: c.transfer.unwrap_or(2),
                matrix: c.matrix.unwrap_or(2),
                chroma_location: 0,
            },
            deblocking: Deblocking::Enabled,
            gop: Gop::Fixed { idr_interval: 250 },
        };
        let limits = Limits {
            frames: u64::MAX,
            total_bytes: max_bytes,
            ..Limits::default()
        };
        let encoder = match settings.threads.filter(|n| *n > 0) {
            Some(n) => {
                coreav_h264_encoder::Encoder::with_threads(settings_native, limits, n as usize)
            }
            None => coreav_h264_encoder::Encoder::new(settings_native, limits),
        }
        .map_err(ineligible)?;
        Ok(Self {
            encoder,
            sink,
            frames: 0,
            size: (settings.width, settings.height),
        })
    }

    pub(crate) fn send_frame(&mut self, pixels: Pixels) -> Result<(), Error> {
        let picture = pixels
            .into_cpu()
            .map_err(|_| Error::NativeIneligible("native H.264 needs CPU planes"))?;
        if picture.format() != PixelFormat::YUV420P
            || (picture.width(), picture.height()) != self.size
        {
            return Err(Error::NativeIneligible(
                "native H.264 needs fixed-size 4:2:0 pictures",
            ));
        }
        let view = |index: usize, width, height| {
            let plane = picture
                .planes()
                .get(index)
                .ok_or(Error::NativeIneligible("native H.264 needs three planes"))?;
            Plane::new(plane.data(), plane.stride(), width, height).map_err(ineligible)
        };
        let (width, height) = (picture.width() as usize, picture.height() as usize);
        let access = self
            .encoder
            .encode(coreav_h264_encoder::Picture {
                y: view(0, width, height)?,
                cb: view(1, width / 2, height / 2)?,
                cr: view(2, width / 2, height / 2)?,
            })
            .map_err(ineligible)?;
        let idr = access
            .nals
            .iter()
            .any(|nal| nal.first().is_some_and(|b| b & 31 == 5));
        for nal in &access.nals {
            if !idr && nal.first().is_some_and(|b| matches!(b & 31, 7 | 8)) {
                continue;
            }
            self.sink.write(&[0, 0, 0, 1]).map_err(Error::Encode)?;
            self.sink.write(nal).map_err(Error::Encode)?;
        }
        self.frames += 1;
        Ok(())
    }

    pub(crate) fn frames_sent(&self) -> u64 {
        self.frames
    }

    pub(crate) fn finish(self) -> S {
        self.sink
    }
}

fn ineligible(_: coreav_h264_encoder::Error) -> Error {
    Error::NativeIneligible("the picture exceeds native H.264 settings or coding limits")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::encoder::FrameRate;

    #[derive(Default)]
    struct Bytes(Vec<u8>);
    impl Sink for Bytes {
        fn write(&mut self, data: &[u8]) -> Result<(), coreav_decode::Error> {
            self.0.extend_from_slice(data);
            Ok(())
        }
    }

    fn picture() -> coreav_frame::Picture {
        coreav_frame::Picture::new(
            18,
            34,
            PixelFormat::YUV420P,
            [(24, 34), (16, 17), (16, 17)]
                .into_iter()
                .map(|(stride, rows)| coreav_frame::Plane::new(vec![128; stride * rows], stride))
                .collect(),
        )
        .unwrap()
    }

    fn settings(threads: u32) -> EncoderSettings {
        EncoderSettings {
            threads: Some(threads),
            color: color(ColorInfo::default()),
            ..EncoderSettings::new(18, 34, PixelFormat::YUV420P, FrameRate { num: 30, den: 1 })
        }
    }

    #[test]
    fn padded_planes_and_every_quality_mode_are_thread_independent() {
        for rate in [
            RateMode::Crf(10),
            RateMode::Crf(23),
            RateMode::Crf(51),
            RateMode::FixedQp(10),
            RateMode::FixedQp(51),
        ] {
            let mut expected = None;
            for threads in [0, 1, 2, 8, 128] {
                let mut encoder = Encoder::new(
                    &settings(threads),
                    NativeH264 { rate },
                    1_000_000,
                    Bytes::default(),
                )
                .unwrap();
                for _ in 0..3 {
                    encoder.send_frame(Pixels::Cpu(picture())).unwrap();
                }
                assert_eq!(encoder.frames_sent(), 3);
                let bytes = encoder.finish().0;
                assert!(!bytes.is_empty());
                if let Some(expected) = &expected {
                    assert_eq!(&bytes, expected);
                } else {
                    expected = Some(bytes);
                }
            }
        }
    }

    #[test]
    fn library_limits_are_retryable_without_counting_a_failed_picture() {
        let request = NativeH264 {
            rate: RateMode::Crf(23),
        };
        let mut encoder = Encoder::new(&settings(1), request, 1, Bytes::default()).unwrap();
        assert!(matches!(
            encoder.send_frame(Pixels::Cpu(picture())),
            Err(Error::NativeIneligible(_))
        ));
        assert_eq!(encoder.frames_sent(), 0);
        assert!(encoder.finish().0.is_empty());
        for (width, height) in [(14, 16), (16, 14), (1922, 16), (16, 1082), (17, 16)] {
            let settings = EncoderSettings {
                width,
                height,
                ..settings(1)
            };
            assert!(matches!(
                Encoder::new(&settings, request, 1_000_000, Bytes::default()),
                Err(Error::NativeIneligible(_))
            ));
        }
        let settings = EncoderSettings {
            sample_aspect: Some((4, 3)),
            ..settings(1)
        };
        assert!(matches!(
            Encoder::new(&settings, request, 1_000_000, Bytes::default()),
            Err(Error::NativeIneligible(_))
        ));
    }
}

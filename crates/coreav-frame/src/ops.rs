//! Frame operations on CPU pictures: crop, rotate, resize, conversion to
//! RGB and the thumbnail path that resizes and converts in one pass.

use crate::color::{Levels, Matrix, row_to_rgb8};
use crate::picture::zeros;
use crate::resample::{Channel, Edge, Order, Source, Taps, resample, taps};
use crate::{ChromaLocation, ColorInfo, Filter, FrameError, Picture, PixelFormat, Plane};

/// A rectangle of a picture, in pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    /// Left edge.
    pub x: u32,
    /// Top edge.
    pub y: u32,
    /// Width.
    pub width: u32,
    /// Height.
    pub height: u32,
}

impl Picture {
    /// Keeps `rect`. Its left and top edges must fall on chroma samples
    /// (even for 4:2:0), so that the chroma planes crop with the luma.
    pub fn crop(&self, rect: Rect) -> Result<Picture, FrameError> {
        let right = rect.x.checked_add(rect.width);
        let bottom = rect.y.checked_add(rect.height);
        if rect.width == 0
            || rect.height == 0
            || right.is_none_or(|r| r > self.width())
            || bottom.is_none_or(|b| b > self.height())
        {
            return Err(FrameError::InvalidGeometry("crop outside the picture"));
        }
        let format = self.format();
        let (sx, sy) = subsampling(format);
        if rect.x % (1 << sx) != 0 || rect.y % (1 << sy) != 0 {
            return Err(FrameError::Unsupported("crop edge between chroma samples"));
        }
        let mut out = Picture::zeroed(rect.width, rect.height, format)?;
        for plane in 0..format.plane_count() {
            let (px, py) = if plane == 0 { (0, 0) } else { (sx, sy) };
            let element = format.element_bytes(plane);
            let x0 = usize::try_from(rect.x >> px)
                .ok()
                .and_then(|x| x.checked_mul(element))
                .ok_or(FrameError::InvalidGeometry("crop offset overflows"))?;
            let y0 = usize::try_from(rect.y >> py)
                .map_err(|_| FrameError::InvalidGeometry("crop offset overflows"))?;
            let (rows, row_bytes) = out
                .plane_rows(plane)
                .ok_or(FrameError::InvalidGeometry("no such plane"))?;
            for y in 0..rows {
                let src = self
                    .row(plane, y0 + y)
                    .and_then(|r| r.get(x0..x0 + row_bytes))
                    .ok_or(FrameError::InvalidGeometry("crop outside the plane"))?;
                if let Some(dst) = out.row_mut(plane, y) {
                    dst.copy_from_slice(src);
                }
            }
        }
        Ok(out)
    }

    /// Turns the picture clockwise by `quarter_turns` × 90°. A quarter or
    /// three-quarter turn of a 4:2:2 picture would give 4:4:0, which the
    /// frame model has no format for; convert it first.
    pub fn rotate(&self, quarter_turns: u8) -> Result<Picture, FrameError> {
        let turns = quarter_turns % 4;
        let format = self.format();
        if turns % 2 == 1 && format.chroma() == Some(crate::Chroma::Cs422) {
            return Err(FrameError::Unsupported(
                "a quarter turn of 4:2:2 has no pixel format",
            ));
        }
        let (w, h) = if turns % 2 == 1 {
            (self.height(), self.width())
        } else {
            (self.width(), self.height())
        };
        let mut out = Picture::zeroed(w, h, format)?;
        for plane in 0..format.plane_count() {
            let element = format.element_bytes(plane);
            let (sw, sh) = format
                .plane_size(plane, self.width(), self.height())
                .ok_or(FrameError::InvalidGeometry("no such plane"))?;
            let (sw, sh) = (to_usize(sw)?, to_usize(sh)?);
            let (dw, dh) = format
                .plane_size(plane, w, h)
                .ok_or(FrameError::InvalidGeometry("no such plane"))?;
            let (dw, dh) = (to_usize(dw)?, to_usize(dh)?);
            for dy in 0..dh {
                let Some(dst) = out.row_mut(plane, dy) else {
                    continue;
                };
                for (dx, d) in dst.chunks_exact_mut(element).take(dw).enumerate() {
                    // Source element shown at (dx, dy) after the turn.
                    let (x, y) = match turns {
                        0 => (dx, dy),
                        1 => (dy, sh.saturating_sub(1 + dx)),
                        2 => (sw.saturating_sub(1 + dx), sh.saturating_sub(1 + dy)),
                        _ => (sw.saturating_sub(1 + dy), dx),
                    };
                    let s = self
                        .row(plane, y)
                        .and_then(|r| r.get(x * element..(x + 1) * element))
                        .ok_or(FrameError::InvalidGeometry("rotation outside the plane"))?;
                    d.copy_from_slice(s);
                }
            }
        }
        Ok(out)
    }

    /// The same picture in a planar format: [`PixelFormat::SemiPlanar`]
    /// becomes [`PixelFormat::Yuv`] 4:2:0 with the values in the low bits;
    /// other formats are returned as they are.
    pub fn to_planar(&self) -> Result<Picture, FrameError> {
        let PixelFormat::SemiPlanar { depth } = self.format() else {
            return Ok(self.clone());
        };
        let format = PixelFormat::Yuv {
            chroma: crate::Chroma::Cs420,
            depth,
        };
        let bytes = format.sample_bytes();
        let shift = if bytes == 2 { 16 - u32::from(depth) } else { 0 };
        let mut out = Picture::zeroed(self.width(), self.height(), format)?;
        let (rows, _) = out
            .plane_rows(0)
            .ok_or(FrameError::InvalidGeometry("no luma plane"))?;
        for y in 0..rows {
            let src = self
                .row(0, y)
                .ok_or(FrameError::InvalidGeometry("short luma plane"))?;
            if let Some(dst) = out.row_mut(0, y) {
                copy_samples(src, dst, bytes, shift);
            }
        }
        let (crows, _) = out
            .plane_rows(1)
            .ok_or(FrameError::InvalidGeometry("no chroma plane"))?;
        for y in 0..crows {
            let src = self
                .row(1, y)
                .ok_or(FrameError::InvalidGeometry("short chroma plane"))?;
            for (component, plane) in [(0usize, 1usize), (1, 2)] {
                if let Some(dst) = out.row_mut(plane, y) {
                    for (d, pair) in dst.chunks_exact_mut(bytes).zip(src.chunks_exact(2 * bytes)) {
                        let s = pair
                            .get(component * bytes..(component + 1) * bytes)
                            .unwrap_or(&[]);
                        copy_samples(s, d, bytes, shift);
                    }
                }
            }
        }
        Ok(out)
    }

    /// Resizes every plane to its share of `width` × `height`, keeping the
    /// format. Chroma planes keep their siting (`chroma_location`, left
    /// when unknown): [`Picture::resize_with`] with
    /// [`ChromaSiting::Location`].
    pub fn resize(
        &self,
        width: u32,
        height: u32,
        filter: Filter,
        chroma_location: Option<ChromaLocation>,
    ) -> Result<Picture, FrameError> {
        self.resize_with(
            width,
            height,
            filter,
            ChromaSiting::Location(chroma_location),
        )
    }

    /// Resizes every plane to its share of `width` × `height`, keeping the
    /// format, with chroma placed as `siting` says.
    /// [`ChromaSiting::FfmpegCompatible`] gives what FFmpeg's default
    /// `scale` filter gives for the same format
    /// (`docs/design/thumbnails.md`, "Y'CbCr to Y'CbCr").
    pub fn resize_with(
        &self,
        width: u32,
        height: u32,
        filter: Filter,
        siting: ChromaSiting,
    ) -> Result<Picture, FrameError> {
        if let PixelFormat::SemiPlanar { .. } = self.format() {
            return self
                .to_planar()?
                .resize_with(width, height, filter, siting)?
                .to_semi_planar();
        }
        let chroma_location = match siting {
            ChromaSiting::Location(location) => location,
            ChromaSiting::FfmpegCompatible => return self.resize_compatible(width, height, filter),
        };
        let format = self.format();
        let mut out = Picture::zeroed(width, height, format)?;
        let siting = chroma_location.unwrap_or(ChromaLocation::Left).offsets();
        let (sx, sy) = subsampling(format);
        let lw = f64::from(self.width()) / f64::from(width);
        let lh = f64::from(self.height()) / f64::from(height);
        for plane in 0..format.plane_count() {
            let (fx, fy) = if plane == 0 { (0, 0) } else { (sx, sy) };
            let (src_w, src_h) = plane_dims(format, plane, self.width(), self.height())?;
            let (dst_w, dst_h) = plane_dims(format, plane, width, height)?;
            let h = axis_taps(src_w, dst_w, lw, fx, siting.0, true, filter)?;
            let v = axis_taps(src_h, dst_h, lh, fy, siting.1, true, filter)?;
            let element = format.element_bytes(plane);
            let sample = format.sample_bytes();
            let channels = element / sample;
            for c in 0..channels {
                let source = self.source(plane, src_w, c * sample, element)?;
                let values = resample(&source, &h, &v, Order::VerticalFirst)?;
                write_channel(&mut out, plane, dst_w, c * sample, element, &values)?;
            }
        }
        Ok(out)
    }

    /// [`ChromaSiting::FfmpegCompatible`]: every plane from its own size to
    /// its own share of the new size, sample centres aligned on the
    /// plane's own grid, FFmpeg's edges.
    fn resize_compatible(
        &self,
        width: u32,
        height: u32,
        filter: Filter,
    ) -> Result<Picture, FrameError> {
        let format = self.format();
        if !matches!(format, PixelFormat::Yuv { .. }) {
            // Measured for Y'CbCr (and grey) only.
            return Err(FrameError::Unsupported(
                "FFmpeg-compatible resize of a format other than planar Y'CbCr",
            ));
        }
        let mut out = Picture::zeroed(width, height, format)?;
        // FFmpeg's intermediate rows stop at 2^depth (measured at 8 and 10
        // bits): an overshoot of the horizontal pass above it is cut
        // before the vertical one.
        let order = Order::HorizontalFirst {
            max: 2f32.powi(i32::from(format.depth())),
        };
        for plane in 0..format.plane_count() {
            let (src_w, src_h) = plane_dims(format, plane, self.width(), self.height())?;
            let (dst_w, dst_h) = plane_dims(format, plane, width, height)?;
            let h = ffmpeg_taps(src_w, dst_w, filter)?;
            let v = ffmpeg_taps(src_h, dst_h, filter)?;
            let element = format.element_bytes(plane);
            let sample = format.sample_bytes();
            for c in 0..element / sample {
                let source = self.source(plane, src_w, c * sample, element)?;
                let values = resample(&source, &h, &v, order)?;
                write_channel(&mut out, plane, dst_w, c * sample, element, &values)?;
            }
        }
        Ok(out)
    }

    /// Converts to [`PixelFormat::Rgb24`] at the same size, chroma
    /// upsampled from its own siting.
    pub fn to_rgb(&self, color: &ColorInfo) -> Result<Picture, FrameError> {
        self.resize_to_rgb(
            self.width(),
            self.height(),
            Filter::Bicubic,
            color,
            ChromaUpsampling::Full,
        )
    }

    /// Resizes to `width` × `height` and converts to
    /// [`PixelFormat::Rgb24`] in one pass: luma is resampled to the target
    /// size, and each chroma plane from its own size straight to its share
    /// of the target size (`chroma`), then every pixel is converted
    /// (`docs/design/thumbnails.md`).
    pub fn resize_to_rgb(
        &self,
        width: u32,
        height: u32,
        filter: Filter,
        color: &ColorInfo,
        chroma: ChromaUpsampling,
    ) -> Result<Picture, FrameError> {
        let format = self.format();
        if let PixelFormat::SemiPlanar { .. } = format {
            return self
                .to_planar()?
                .resize_to_rgb(width, height, filter, color, chroma);
        }
        if matches!(format, PixelFormat::Rgb24 | PixelFormat::Rgba) {
            let resized = self.resize(width, height, filter, None)?;
            return if format == PixelFormat::Rgb24 {
                Ok(resized)
            } else {
                resized.drop_alpha()
            };
        }
        let mut out = Picture::zeroed(width, height, PixelFormat::Rgb24)?;
        let compatible = chroma == ChromaUpsampling::FfmpegCompatible
            && !matches!(format, PixelFormat::Gbr { .. });
        let siting = if compatible {
            ChromaLocation::Center
        } else {
            color.chroma_location.unwrap_or(ChromaLocation::Left)
        }
        .offsets();
        let (sx, sy) = subsampling(format);
        let lw = f64::from(self.width()) / f64::from(width);
        let lh = f64::from(self.height()) / f64::from(height);
        let (dst_w, dst_h) = (to_usize(width)?, to_usize(height)?);
        let unscaled = width == self.width() && height == self.height();
        // In the FFmpeg-compatible mode chroma has half the output width.
        let chroma_w = if compatible && !unscaled {
            dst_w.div_ceil(2)
        } else {
            dst_w
        };
        let sample = format.sample_bytes();
        let mut channels: Vec<Vec<f32>> = Vec::with_capacity(3);
        for plane in 0..format.plane_count() {
            let (fx, fy) = if plane == 0 { (0, 0) } else { (sx, sy) };
            let (src_w, src_h) = plane_dims(format, plane, self.width(), self.height())?;
            let (h, v) = if plane == 0 || !compatible {
                (
                    axis_taps(src_w, dst_w, lw, fx, siting.0, false, filter)?,
                    axis_taps(src_h, dst_h, lh, fy, siting.1, false, filter)?,
                )
            } else if unscaled {
                (nearest_taps(dst_w, fx), nearest_taps(dst_h, fy))
            } else {
                (
                    pair_taps(src_w, chroma_w, lw, fx, siting.0, filter)?,
                    axis_taps(src_h, dst_h, lh, fy, siting.1, false, filter)?,
                )
            };
            let source = self.source(plane, src_w, 0, sample)?;
            channels.push(resample(&source, &h, &v, Order::VerticalFirst)?);
        }
        let matrix = if let PixelFormat::Gbr { .. } = format {
            Matrix::Identity
        } else {
            Matrix::from_code(color.matrix)
        };
        let levels = Levels::new(format.depth(), color.full_range.unwrap_or(false));
        let mid = f32::from(1u16 << (format.depth().clamp(8, 16) - 1));
        // Monochrome has no chroma planes: neutral chroma.
        let neutral = vec![mid; dst_w];
        let empty: Vec<f32> = Vec::new();
        let (a, b, c) = match channels.as_slice() {
            [a, b, c] => (a.as_slice(), b.as_slice(), c.as_slice()),
            [a] => (a.as_slice(), empty.as_slice(), empty.as_slice()),
            _ => return Err(FrameError::Unsupported("pixel format")),
        };
        for y in 0..dst_h {
            let Some(row) = out.row_mut(0, y) else {
                continue;
            };
            let range = y * dst_w..(y + 1) * dst_w;
            let chroma_range = y * chroma_w..(y + 1) * chroma_w;
            let (lb, lc) = (
                line(b, &chroma_range, &neutral),
                line(c, &chroma_range, &neutral),
            );
            let step = if chroma_w == dst_w { 1 } else { 2 };
            row_to_rgb8(
                matrix,
                levels,
                (line(a, &range, &neutral), lb, lc),
                step,
                mid,
                row,
            );
        }
        Ok(out)
    }

    fn drop_alpha(&self) -> Result<Picture, FrameError> {
        let mut out = Picture::zeroed(self.width(), self.height(), PixelFormat::Rgb24)?;
        let (rows, _) = self
            .plane_rows(0)
            .ok_or(FrameError::InvalidGeometry("no plane"))?;
        for y in 0..rows {
            let src = self
                .row(0, y)
                .ok_or(FrameError::InvalidGeometry("short plane"))?;
            if let Some(dst) = out.row_mut(0, y) {
                for (d, s) in dst.chunks_exact_mut(3).zip(src.chunks_exact(4)) {
                    d.copy_from_slice(s.get(..3).unwrap_or(&[0, 0, 0]));
                }
            }
        }
        Ok(out)
    }

    fn to_semi_planar(&self) -> Result<Picture, FrameError> {
        let PixelFormat::Yuv { depth, .. } = self.format() else {
            return Err(FrameError::Unsupported("not planar Y'CbCr"));
        };
        let format = PixelFormat::SemiPlanar { depth };
        let bytes = format.sample_bytes();
        let shift = if bytes == 2 { 16 - u32::from(depth) } else { 0 };
        let (w, h) = (self.width(), self.height());
        let (luma_rows, luma_bytes) = self
            .plane_rows(0)
            .ok_or(FrameError::InvalidGeometry("no luma plane"))?;
        let mut luma = zeros(luma_rows * luma_bytes)?;
        for (dst, src) in luma.chunks_exact_mut(luma_bytes.max(1)).zip(self.rows(0)) {
            for (d, s) in dst.chunks_exact_mut(bytes).zip(src.chunks_exact(bytes)) {
                shift_sample(s, d, shift);
            }
        }
        let (crows, cbytes) = self
            .plane_rows(1)
            .ok_or(FrameError::InvalidGeometry("no chroma plane"))?;
        let mut chroma = zeros(crows * cbytes * 2)?;
        for (y, dst) in chroma.chunks_exact_mut((cbytes * 2).max(1)).enumerate() {
            let (cb, cr) = (self.row(1, y), self.row(2, y));
            let (Some(cb), Some(cr)) = (cb, cr) else {
                continue;
            };
            for ((pair, b), r) in dst
                .chunks_exact_mut(2 * bytes)
                .zip(cb.chunks_exact(bytes))
                .zip(cr.chunks_exact(bytes))
            {
                let (pb, pr) = pair.split_at_mut(bytes);
                shift_sample(b, pb, shift);
                shift_sample(r, pr, shift);
            }
        }
        Picture::new(
            w,
            h,
            format,
            vec![Plane::new(luma, luma_bytes), Plane::new(chroma, cbytes * 2)],
        )
    }

    fn source(
        &self,
        plane: usize,
        width: usize,
        offset: usize,
        element: usize,
    ) -> Result<Source<'_>, FrameError> {
        let format = self.format();
        let shift = match format {
            PixelFormat::SemiPlanar { depth } if depth > 8 => 16 - u32::from(depth),
            _ => 0,
        };
        Ok(Source {
            rows: self.rows(plane).collect(),
            width,
            channel: Channel {
                element,
                offset,
                sample: format.sample_bytes(),
                shift,
            },
        })
    }
}

/// How chroma reaches each output pixel in [`Picture::resize_to_rgb`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ChromaUpsampling {
    /// Each chroma plane is resampled from its own siting (the colour
    /// description's chroma location, left when unknown) straight to every
    /// output pixel.
    Full,
    /// As FFmpeg's default scaler converts to RGB, measured as a black box
    /// (`docs/design/thumbnails.md`): chroma is taken as centred whatever
    /// the stream signals, resampled to half the output width (rounded up)
    /// and the full output height, and each chroma sample is shown on two
    /// output pixels; at the source size, each chroma sample is repeated
    /// over the pixels it covers.
    #[default]
    FfmpegCompatible,
}

/// Where the chroma samples of [`Picture::resize_with`] sit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ChromaSiting {
    /// As FFmpeg's default `scale` filter resizes to the same pixel format,
    /// measured as a black box (`docs/design/thumbnails.md`, "Y'CbCr to
    /// Y'CbCr"): each plane is resampled on its own grid, from its own size
    /// to its own share of the new size, with sample centres aligned, which
    /// takes chroma as centred whatever the stream signals. FFmpeg's
    /// position step (a multiple of 2^-16), its edges (a few far taps
    /// before the first sample are dropped when the bicubic enlarges) and
    /// its intermediate rows (horizontal pass first, cut at 2^depth) are
    /// followed too. Planar Y'CbCr (and NV12, through planar) only; other
    /// formats give [`FrameError::Unsupported`].
    FfmpegCompatible,
    /// From the given chroma location (left when `None`), mapped through
    /// the luma grid, with every edge clamped.
    Location(Option<ChromaLocation>),
}

/// Taps of [`ChromaSiting::FfmpegCompatible`] from `src` samples to `dst`:
/// sample centres aligned on the axis's own grid, stepping by
/// `src / dst` rounded to a multiple of 2^-16 as FFmpeg's scaler steps
/// (measured: without the rounding, positions drift apart by up to a
/// hundredth of a sample across a 1080p row), and FFmpeg's edges.
fn ffmpeg_taps(src: usize, dst: usize, filter: Filter) -> Result<Vec<Taps>, FrameError> {
    let (s, d) = (
        u64::try_from(src).map_err(|_| FrameError::InvalidGeometry("size does not fit"))?,
        u64::try_from(dst.max(1)).map_err(|_| FrameError::InvalidGeometry("size does not fit"))?,
    );
    // `s` is a plane size, below 2^32, so `s << 16` fits.
    // At least 2^-16: enlarging more than 2^17 times would round it to 0.
    let fixed = (((s << 16) + d / 2) / d).max(1);
    // Below 2^48: exact in an f64.
    #[allow(clippy::cast_precision_loss)]
    let step = fixed as f64 / 65536.0;
    taps(src, dst, step, filter, Edge::FfmpegCompatible, |i| {
        (index_f64(i) + 0.5) * step - 0.5
    })
}

/// Each output position reads the source sample `i >> shift`.
fn nearest_taps(dst: usize, shift: u32) -> Vec<Taps> {
    (0..dst)
        .map(|i| Taps {
            start: i >> shift,
            weights: vec![1.0],
        })
        .collect()
}

/// Horizontal chroma taps of the FFmpeg-compatible mode: output chroma
/// sample `m` stands for output pixels `2m` and `2m + 1`, centred between
/// them.
fn pair_taps(
    src: usize,
    dst: usize,
    step: f64,
    shift: u32,
    siting: f64,
    filter: Filter,
) -> Result<Vec<Taps>, FrameError> {
    let factor = f64::from(1u32 << shift);
    let off = if shift == 0 { 0.0 } else { siting };
    taps(src, dst, 2.0 * step / factor, filter, Edge::Clamp, |m| {
        ((2.0 * index_f64(m) + 1.0) * step - 0.5 - off) / factor
    })
}

/// Taps for one axis of one plane. `step` is source luma samples per
/// output luma sample, `shift` the plane's subsampling on this axis and
/// `siting` the chroma location's offset in luma samples. With
/// `subsampled_output`, output positions are the output plane's own
/// (subsampled) samples; without, every output sample is a luma sample.
fn axis_taps(
    src: usize,
    dst: usize,
    step: f64,
    shift: u32,
    siting: f64,
    subsampled_output: bool,
    filter: Filter,
) -> Result<Vec<Taps>, FrameError> {
    if shift == 0 {
        return taps(src, dst, step, filter, Edge::Clamp, |i| {
            (index_f64(i) + 0.5) * step - 0.5
        });
    }
    let factor = f64::from(1u32 << shift);
    let off = siting;
    if subsampled_output {
        // Output chroma sample i sits at output luma position
        // factor·i + off; map it to source luma, then to source chroma.
        taps(src, dst, step, filter, Edge::Clamp, |i| {
            let u = factor * index_f64(i) + off;
            ((u + 0.5) * step - 0.5 - off) / factor
        })
    } else {
        taps(src, dst, step / factor, filter, Edge::Clamp, |i| {
            ((index_f64(i) + 0.5) * step - 0.5 - off) / factor
        })
    }
}

#[inline]
fn index_f64(i: usize) -> f64 {
    // Picture sizes are at most 2^32, exact in an f64.
    #[allow(clippy::cast_precision_loss)]
    let x = i as f64;
    x
}

/// Row `range` of a resampled channel, or `neutral` for a channel the
/// format lacks.
fn line<'a>(channel: &'a [f32], range: &core::ops::Range<usize>, neutral: &'a [f32]) -> &'a [f32] {
    channel.get(range.clone()).unwrap_or(neutral)
}

fn subsampling(format: PixelFormat) -> (u32, u32) {
    format.chroma().map_or((0, 0), crate::Chroma::shifts)
}

fn plane_dims(
    format: PixelFormat,
    plane: usize,
    width: u32,
    height: u32,
) -> Result<(usize, usize), FrameError> {
    let (w, h) = format
        .plane_size(plane, width, height)
        .ok_or(FrameError::InvalidGeometry("no such plane"))?;
    Ok((to_usize(w)?, to_usize(h)?))
}

fn to_usize(v: u32) -> Result<usize, FrameError> {
    usize::try_from(v).map_err(|_| FrameError::InvalidGeometry("size does not fit"))
}

fn write_channel(
    out: &mut Picture,
    plane: usize,
    width: usize,
    offset: usize,
    element: usize,
    values: &[f32],
) -> Result<(), FrameError> {
    let format = out.format();
    let sample = format.sample_bytes();
    let max = f32::from(u16::MAX >> (16 - u32::from(format.depth().clamp(8, 16))));
    let (rows, _) = out
        .plane_rows(plane)
        .ok_or(FrameError::InvalidGeometry("no such plane"))?;
    for y in 0..rows {
        let Some(row) = out.row_mut(plane, y) else {
            continue;
        };
        let line = values
            .get(y * width..(y + 1) * width)
            .ok_or(FrameError::InvalidGeometry("short resampled plane"))?;
        for (e, v) in row.chunks_exact_mut(element).zip(line) {
            let clamped = (v + 0.5).clamp(0.0, max);
            // In 0..=max after the clamp.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let s = clamped as u16;
            if let Some(dst) = e.get_mut(offset..offset + sample) {
                if sample == 1 {
                    dst.copy_from_slice(&[s.to_le_bytes()[0]]);
                } else {
                    dst.copy_from_slice(&s.to_le_bytes());
                }
            }
        }
    }
    Ok(())
}

/// Copies samples, shifting 16-bit ones right by `shift`.
fn copy_samples(src: &[u8], dst: &mut [u8], bytes: usize, shift: u32) {
    if bytes == 1 || shift == 0 {
        let n = dst.len().min(src.len());
        if let (Some(d), Some(s)) = (dst.get_mut(..n), src.get(..n)) {
            d.copy_from_slice(s);
        }
        return;
    }
    for (d, s) in dst.chunks_exact_mut(2).zip(src.chunks_exact(2)) {
        if let [a, b] = s {
            let v = u16::from_le_bytes([*a, *b]) >> shift;
            d.copy_from_slice(&v.to_le_bytes());
        }
    }
}

/// Copies one sample, shifting a 16-bit one left by `shift`.
fn shift_sample(src: &[u8], dst: &mut [u8], shift: u32) {
    match (src, dst) {
        ([a, b], [c, d]) => {
            let v = u16::from_le_bytes([*a, *b]) << shift;
            let [x, y] = v.to_le_bytes();
            *c = x;
            *d = y;
        }
        ([a], [c]) => *c = *a,
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Chroma;

    fn gradient(w: u32, h: u32, format: PixelFormat) -> Picture {
        let mut p = Picture::zeroed(w, h, format).expect("picture");
        for plane in 0..format.plane_count() {
            let (rows, _) = p.plane_rows(plane).expect("rows");
            for y in 0..rows {
                let row = p.row_mut(plane, y).expect("row");
                for (x, b) in row.iter_mut().enumerate() {
                    *b = ((x * 7 + y * 13 + plane * 50) % 251) as u8;
                }
            }
        }
        p
    }

    #[test]
    fn crop_and_alignment() {
        let p = gradient(16, 8, PixelFormat::YUV420P);
        let c = p
            .crop(Rect {
                x: 2,
                y: 4,
                width: 5,
                height: 3,
            })
            .expect("crop");
        assert_eq!((c.width(), c.height()), (5, 3));
        assert_eq!(c.row(0, 0), p.row(0, 4).map(|r| &r[2..7]));
        assert_eq!(c.row(1, 1), p.row(1, 3).map(|r| &r[1..4]));
        assert!(
            p.crop(Rect {
                x: 1,
                y: 0,
                width: 2,
                height: 2
            })
            .is_err()
        );
        assert!(
            p.crop(Rect {
                x: 10,
                y: 0,
                width: 7,
                height: 2
            })
            .is_err()
        );
    }

    #[test]
    fn four_quarter_turns_are_identity() {
        for format in [
            PixelFormat::YUV420P,
            PixelFormat::Rgb24,
            PixelFormat::Yuv {
                chroma: Chroma::Cs444,
                depth: 10,
            },
            PixelFormat::SemiPlanar { depth: 8 },
        ] {
            let p = gradient(6, 4, format);
            let mut r = p.clone();
            for _ in 0..4 {
                r = r.rotate(1).expect("rotate");
            }
            assert_eq!(r, p, "{format:?}");
            let half = p.rotate(2).expect("rotate");
            assert_eq!(half.rotate(2).expect("rotate"), p);
        }
        let q = gradient(3, 2, PixelFormat::Rgb24)
            .rotate(1)
            .expect("rotate");
        assert_eq!((q.width(), q.height()), (2, 3));
        // The bottom-left pixel becomes the top-left one.
        let src = gradient(3, 2, PixelFormat::Rgb24);
        assert_eq!(q.row(0, 0).map(|r| &r[..3]), src.row(0, 1).map(|r| &r[..3]));
        let y422 = gradient(
            4,
            4,
            PixelFormat::Yuv {
                chroma: Chroma::Cs422,
                depth: 8,
            },
        );
        assert!(y422.rotate(1).is_err());
        assert!(y422.rotate(2).is_ok());
    }

    #[test]
    fn semi_planar_round_trip() {
        let p = gradient(6, 4, PixelFormat::YUV420P);
        let sp = p.to_semi_planar().expect("semi-planar");
        assert_eq!(sp.to_planar().expect("planar"), p);
    }

    #[test]
    fn resize_identity_and_flat() {
        let p = gradient(16, 8, PixelFormat::YUV420P);
        let same = p.resize(16, 8, Filter::Bicubic, None).expect("resize");
        assert_eq!(same, p);
        let mut flat = Picture::zeroed(64, 32, PixelFormat::YUV420P).expect("picture");
        for plane in 0..3 {
            let (rows, _) = flat.plane_rows(plane).expect("rows");
            for y in 0..rows {
                flat.row_mut(plane, y).expect("row").fill(100);
            }
        }
        for filter in [Filter::Bilinear, Filter::Bicubic, Filter::Area] {
            for siting in [ChromaSiting::Location(None), ChromaSiting::FfmpegCompatible] {
                let small = flat.resize_with(13, 7, filter, siting).expect("resize");
                assert!(small.rows(0).all(|r| r.iter().all(|v| *v == 100)));
                assert!(small.rows(2).all(|r| r.iter().all(|v| *v == 100)));
            }
        }
        let same = p
            .resize_with(16, 8, Filter::Bicubic, ChromaSiting::FfmpegCompatible)
            .expect("resize");
        assert_eq!(same, p);
        let sp = p.to_semi_planar().expect("semi-planar");
        let resized = sp
            .resize_with(8, 4, Filter::Bicubic, ChromaSiting::FfmpegCompatible)
            .expect("resize");
        let planar = p
            .resize_with(8, 4, Filter::Bicubic, ChromaSiting::FfmpegCompatible)
            .expect("resize");
        assert_eq!(resized.to_planar().expect("planar"), planar);
    }

    /// Weights of FFmpeg's default scaler, measured as a black box: 16-bit
    /// grey, one impulse per source column (`docs/design/thumbnails.md`,
    /// "Y'CbCr to Y'CbCr").
    #[test]
    fn ffmpeg_taps_match_measured_weights() {
        let check = |src: usize, dst: usize, i: usize, start: usize, want: &[f32]| {
            let t = ffmpeg_taps(src, dst, Filter::Bicubic).expect("taps");
            let tap = &t[i];
            assert_eq!(tap.start, start, "{src}->{dst} position {i}");
            assert_eq!(tap.weights.len(), want.len(), "{src}->{dst} position {i}");
            for (w, m) in tap.weights.iter().zip(want) {
                assert!((w - m).abs() < 2e-4, "{src}->{dst} {i}: {:?}", tap.weights);
            }
        };
        // Enlarging by 2: the interior is Keys' cubic, a = -0.6.
        check(16, 32, 4, 0, &[-0.0281, 0.2407, 0.8718, -0.0844]);
        // The left edge: a tap before sample 0 within 1.5 is read from
        // sample 0, a farther one is dropped before normalizing.
        check(16, 32, 0, 0, &[1.0821, -0.0821]);
        check(16, 32, 1, 0, &[0.7875, 0.2407, -0.0281]);
        check(16, 32, 2, 0, &[0.2341, 0.8480, -0.0821]);
        // The right edge clamps.
        check(16, 32, 29, 13, &[-0.0844, 0.8718, 0.2125]);
        check(16, 32, 31, 14, &[-0.0844, 1.0844]);
        // At 1.5 exactly, the 2^-16 step decides: dropped for 8 -> 12
        // (step rounded up), kept for 16 -> 40 (rounded down).
        check(8, 12, 1, 0, &[0.5349, 0.5349, -0.0698]);
        check(16, 40, 2, 0, &[0.5000, 0.5750, -0.0750]);
        // Shrinking by 2 stretches the kernel; the edges clamp.
        check(32, 16, 0, 0, &[0.5000, 0.4359, 0.1204, -0.0422, -0.0140]);
        check(
            32,
            16,
            1,
            0,
            &[-0.0563, 0.1204, 0.4359, 0.4359, 0.1204, -0.0422, -0.0140],
        );
    }

    #[test]
    fn rgb_of_grey_and_colours() {
        let mut p = Picture::zeroed(8, 8, PixelFormat::YUV420P).expect("picture");
        for (plane, value) in [(0, 81u8), (1, 90), (2, 240)] {
            let (rows, _) = p.plane_rows(plane).expect("rows");
            for y in 0..rows {
                p.row_mut(plane, y).expect("row").fill(value);
            }
        }
        let rgb = p.to_rgb(&ColorInfo::default()).expect("rgb");
        assert!(rgb.rows(0).all(|r| r.chunks(3).all(|px| px == [254, 0, 0])));
        let small = p
            .resize_to_rgb(
                3,
                5,
                Filter::Bicubic,
                &ColorInfo::default(),
                ChromaUpsampling::FfmpegCompatible,
            )
            .expect("rgb");
        assert_eq!((small.width(), small.height()), (3, 5));
        assert!(
            small
                .rows(0)
                .all(|r| r.chunks(3).all(|px| px == [254, 0, 0]))
        );
        let mono = Picture::zeroed(
            4,
            4,
            PixelFormat::Yuv {
                chroma: Chroma::Mono,
                depth: 8,
            },
        )
        .expect("mono");
        let grey = mono.to_rgb(&ColorInfo::default()).expect("rgb");
        assert!(grey.rows(0).all(|r| r.iter().all(|v| *v == 0)));
    }
}

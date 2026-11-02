//! A frame's headers: the uncompressed header (clauses 6.2 and 7.2), then
//! the compressed header (clause 6.3) with the probabilities it updates.

use crate::bits::BitReader;
use crate::compressed::{self, CompressedHeader};
use crate::error::{Error, Result};
use crate::picture::Picture;
use crate::probs::{Contexts, Probs};

/// The smallest width of a tile, in superblocks (`MIN_TILE_WIDTH_B64`).
const MIN_TILE_WIDTH_B64: u32 = 4;
/// The largest width of a tile, in superblocks (`MAX_TILE_WIDTH_B64`).
const MAX_TILE_WIDTH_B64: u32 = 64;
/// Segments of the segmentation map (`MAX_SEGMENTS`).
pub const MAX_SEGMENTS: usize = 8;
/// Features of a segment (`SEG_LVL_MAX`): quantizer, loop filter level,
/// reference frame and skip.
pub const SEG_LVL_MAX: usize = 4;
/// The bits of each segment feature's value (`segmentation_feature_bits`).
const FEATURE_BITS: [u32; SEG_LVL_MAX] = [8, 6, 2, 0];
/// Whether each segment feature's value has a sign
/// (`segmentation_feature_signed`).
const FEATURE_SIGNED: [bool; SEG_LVL_MAX] = [true, true, false, false];

/// The colour space a frame states (`color_space`, clause 7.2.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorSpace {
    /// `CS_UNKNOWN`: signalled outside the bitstream.
    Unknown,
    /// `CS_BT_601`.
    Bt601,
    /// `CS_BT_709`.
    Bt709,
    /// `CS_SMPTE_170`.
    Smpte170,
    /// `CS_SMPTE_240`.
    Smpte240,
    /// `CS_BT_2020`.
    Bt2020,
    /// `CS_RESERVED`.
    Reserved,
    /// `CS_RGB`: sRGB.
    Rgb,
}

impl ColorSpace {
    fn from_code(code: u32) -> ColorSpace {
        match code {
            0 => ColorSpace::Unknown,
            1 => ColorSpace::Bt601,
            2 => ColorSpace::Bt709,
            3 => ColorSpace::Smpte170,
            4 => ColorSpace::Smpte240,
            5 => ColorSpace::Bt2020,
            6 => ColorSpace::Reserved,
            _ => ColorSpace::Rgb,
        }
    }
}

/// How a frame's samples are coded (`color_config()`, clause 6.2.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ColorConfig {
    /// 8, 10 or 12 (`BitDepth`).
    pub bit_depth: u8,
    /// The colour space.
    pub color_space: ColorSpace,
    /// `color_range` equal to 1: no restriction on the sample values.
    pub full_range: bool,
    /// Chroma has half the luma columns (`subsampling_x`).
    pub subsampling_x: bool,
    /// Chroma has half the luma rows (`subsampling_y`).
    pub subsampling_y: bool,
}

/// The interpolation filter of inter prediction (`interpolation_filter`,
/// clause 7.2.7).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InterpolationFilter {
    /// `EIGHTTAP`.
    EightTap,
    /// `EIGHTTAP_SMOOTH`.
    EightTapSmooth,
    /// `EIGHTTAP_SHARP`.
    EightTapSharp,
    /// `BILINEAR`.
    Bilinear,
    /// `SWITCHABLE`: chosen per block.
    Switchable,
}

/// The loop filter's parameters (`loop_filter_params()`, clause 6.2.8).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LoopFilter {
    /// `loop_filter_level`, 0 to 63.
    pub level: u8,
    /// `loop_filter_sharpness`, 0 to 7.
    pub sharpness: u8,
    /// `loop_filter_delta_enabled`.
    pub delta_enabled: bool,
    /// `loop_filter_ref_deltas`, by reference frame (intra, last, golden,
    /// altref), after this frame's updates.
    pub ref_deltas: [i8; 4],
    /// `loop_filter_mode_deltas`, after this frame's updates.
    pub mode_deltas: [i8; 2],
}

/// The quantizer's parameters (`quantization_params()`, clause 6.2.9).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Quantization {
    /// `base_q_idx`.
    pub base_q_idx: u8,
    /// `delta_q_y_dc`.
    pub delta_q_y_dc: i8,
    /// `delta_q_uv_dc`.
    pub delta_q_uv_dc: i8,
    /// `delta_q_uv_ac`.
    pub delta_q_uv_ac: i8,
}

impl Quantization {
    /// `Lossless`: every quantizer is 0, and the frame uses the
    /// Walsh-Hadamard transform.
    pub fn lossless(&self) -> bool {
        self.base_q_idx == 0
            && self.delta_q_y_dc == 0
            && self.delta_q_uv_dc == 0
            && self.delta_q_uv_ac == 0
    }
}

/// The segmentation parameters (`segmentation_params()`, clause 6.2.11).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Segmentation {
    /// `segmentation_enabled`.
    pub enabled: bool,
    /// `segmentation_update_map`.
    pub update_map: bool,
    /// `segmentation_tree_probs`, where the map is updated.
    pub tree_probs: [u8; 7],
    /// `segmentation_pred_prob`, where the map is updated.
    pub pred_probs: [u8; 3],
    /// `segmentation_temporal_update`.
    pub temporal_update: bool,
    /// `segmentation_abs_or_delta_update`, after this frame's updates.
    pub abs_or_delta_update: bool,
    /// `FeatureData` of each segment and feature where `FeatureEnabled`
    /// is 1, after this frame's updates.
    pub features: [[Option<i16>; SEG_LVL_MAX]; MAX_SEGMENTS],
}

/// The tile layout (`tile_info()`, clause 6.2.13).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TileInfo {
    /// `tile_cols_log2`.
    pub cols_log2: u8,
    /// `tile_rows_log2`.
    pub rows_log2: u8,
}

/// A reference slot as the uncompressed header needs it: the size, profile
/// and colour configuration of the frame clause 8.10 stores there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Slot {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) profile: u8,
    pub(crate) color: ColorConfig,
}

impl Slot {
    /// Whether a frame of `profile` coded in `color` may predict from this
    /// slot: the same bit depth, profile, subsampling and colour space
    /// (clause 7.2, `ref_frame_idx`).
    fn matches(&self, profile: u8, color: &ColorConfig) -> bool {
        self.profile == profile
            && self.color.bit_depth == color.bit_depth
            && self.color.subsampling_x == color.subsampling_x
            && self.color.subsampling_y == color.subsampling_y
            && self.color.color_space == color.color_space
    }
}

/// What headers carry from one frame to the next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Carried {
    /// The colour configuration of the last frame that stated one; an inter
    /// frame keeps it.
    pub(crate) color: Option<ColorConfig>,
    pub(crate) ref_deltas: [i8; 4],
    pub(crate) mode_deltas: [i8; 2],
    pub(crate) abs_or_delta_update: bool,
    pub(crate) features: [[Option<i16>; SEG_LVL_MAX]; MAX_SEGMENTS],
}

impl Carried {
    /// The values `setup_past_independence` sets (clause 7.2), with the
    /// colour configuration kept.
    fn independent(color: Option<ColorConfig>) -> Carried {
        Carried {
            color,
            ref_deltas: [1, 0, -1, -1],
            mode_deltas: [0, 0],
            abs_or_delta_update: false,
            features: [[None; SEG_LVL_MAX]; MAX_SEGMENTS],
        }
    }
}

impl Default for Carried {
    fn default() -> Self {
        Carried::independent(None)
    }
}

/// A frame's uncompressed header, for a frame that is not
/// `show_existing_frame`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrameHeader {
    /// `Profile`, 0 to 3.
    pub profile: u8,
    /// `frame_type` equal to `KEY_FRAME`.
    pub key_frame: bool,
    /// `show_frame`.
    pub show_frame: bool,
    /// `error_resilient_mode`.
    pub error_resilient: bool,
    /// `intra_only`.
    pub intra_only: bool,
    /// `reset_frame_context`, 0 to 3.
    pub reset_frame_context: u8,
    /// The colour configuration the frame is coded in.
    pub color: ColorConfig,
    /// `FrameWidth`.
    pub width: u32,
    /// `FrameHeight`.
    pub height: u32,
    /// The render size: a hint to the application (clause 7.2.4).
    pub render_size: (u32, u32),
    /// `refresh_frame_flags`: bit `i` replaces reference slot `i`.
    pub refresh_frame_flags: u8,
    /// `ref_frame_idx`: the slots of the last, golden and altref frames.
    pub ref_frame_idx: [u8; 3],
    /// `ref_frame_sign_bias`, by reference frame (intra, last, golden,
    /// altref).
    pub ref_frame_sign_bias: [bool; 4],
    /// `allow_high_precision_mv`.
    pub allow_high_precision_mv: bool,
    /// `interpolation_filter`.
    pub interpolation_filter: InterpolationFilter,
    /// `refresh_frame_context`.
    pub refresh_frame_context: bool,
    /// `frame_parallel_decoding_mode`.
    pub frame_parallel: bool,
    /// `frame_context_idx` as decoding uses it: 0 for an intra or error
    /// resilient frame, whatever the bitstream coded.
    pub frame_context_idx: u8,
    /// `frame_context_idx` as coded, which an intra-only frame with
    /// `reset_frame_context` equal to 2 resets.
    pub coded_frame_context_idx: u8,
    /// The loop filter's parameters.
    pub loop_filter: LoopFilter,
    /// The quantizer's parameters.
    pub quantization: Quantization,
    /// The segmentation parameters.
    pub segmentation: Segmentation,
    /// The tile layout.
    pub tiles: TileInfo,
    /// `header_size_in_bytes`: the compressed header's size.
    pub compressed_header_size: u16,
    /// The size of the uncompressed header with its trailing bits, in bytes.
    pub uncompressed_size: usize,
    /// Whether every `zero_bit` of the trailing bits was 0 (clause 7.1.1).
    pub trailing_bits_zero: bool,
}

impl FrameHeader {
    /// `FrameIsIntra`: a key frame or an intra-only frame.
    pub fn is_intra(&self) -> bool {
        self.key_frame || self.intra_only
    }

    /// `MiCols`: the frame's width in 8x8 blocks.
    pub fn mi_cols(&self) -> u32 {
        self.width.div_ceil(8)
    }

    /// `MiRows`: the frame's height in 8x8 blocks.
    pub fn mi_rows(&self) -> u32 {
        self.height.div_ceil(8)
    }

    /// The slot the frame leaves in each reference slot it refreshes
    /// (clause 8.10).
    pub(crate) fn slot(&self) -> Slot {
        Slot {
            width: self.width,
            height: self.height,
            profile: self.profile,
            color: self.color,
        }
    }

    /// What the frame leaves for the next one's header.
    pub(crate) fn carried(&self) -> Carried {
        Carried {
            color: Some(self.color),
            ref_deltas: self.loop_filter.ref_deltas,
            mode_deltas: self.loop_filter.mode_deltas,
            abs_or_delta_update: self.segmentation.abs_or_delta_update,
            features: self.segmentation.features,
        }
    }
}

/// A frame's headers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Header {
    /// `show_existing_frame`: show the frame in this reference slot again.
    ShowExisting {
        /// `Profile`.
        profile: u8,
        /// `frame_to_show_map_idx`.
        slot: u8,
        /// The width of the frame in that slot.
        width: u32,
        /// The height of the frame in that slot.
        height: u32,
    },
    /// A frame to decode.
    Frame {
        /// Its uncompressed header.
        header: Box<FrameHeader>,
        /// Its compressed header.
        compressed: CompressedHeader,
        /// Whether every tile's boolean decoder ended as clause 9.2.3
        /// requires; `None` for a frame whose tiles are not read yet (an
        /// inter frame, until inter prediction lands).
        tiles_end_cleanly: Option<bool>,
        /// The decoded frame of an intra frame, without the loop filter
        /// (`docs/design/vp9.md`, milestone 3); `None` for an inter frame,
        /// until inter prediction lands.
        picture: Option<Box<Picture>>,
    },
}

/// A frame's headers, and for a frame to decode, the probabilities.
pub(crate) struct Parsed {
    pub(crate) header: Header,
    /// The saved frame contexts after the uncompressed header's resets
    /// (clause 7.2), and the frame's probabilities after its compressed
    /// header; `None` for `show_existing_frame`.
    pub(crate) probs: Option<Box<(Contexts, Probs)>>,
}

/// Reads the headers of the frame `data`, given the reference slots, what
/// earlier headers left and the saved frame contexts.
pub(crate) fn parse(
    data: &[u8],
    slots: &[Option<Slot>; 8],
    carried: &Carried,
    contexts: &Contexts,
) -> Result<Parsed> {
    let mut r = BitReader::new(data);
    const H: &str = "the uncompressed header";
    if r.f(2, H)? != 2 {
        return Err(Error::Invalid("frame_marker is not 2"));
    }
    let low = r.f(1, H)?;
    let high = r.f(1, H)?;
    let profile = ((high << 1) | low) as u8;
    if profile == 3 && r.bit(H)? {
        return Err(Error::Invalid("reserved_zero is not 0"));
    }
    if r.bit(H)? {
        let slot = r.f(3, H)? as u8;
        let shown = slots
            .get(usize::from(slot))
            .copied()
            .flatten()
            .ok_or(Error::Invalid(
                "show_existing_frame of an empty reference slot",
            ))?;
        return Ok(Parsed {
            header: Header::ShowExisting {
                profile,
                slot,
                width: shown.width,
                height: shown.height,
            },
            probs: None,
        });
    }
    let key_frame = !r.bit(H)?;
    let show_frame = r.bit(H)?;
    let error_resilient = r.bit(H)?;
    let mut intra_only = false;
    let mut reset_frame_context = 0;
    let color;
    let refresh_frame_flags;
    let mut ref_frame_idx = [0u8; 3];
    let mut sign_bias = [false; 4];
    let mut allow_high_precision_mv = false;
    let mut interpolation_filter = InterpolationFilter::EightTap;
    let (width, height);
    let render_size;
    if key_frame {
        frame_sync_code(&mut r)?;
        color = color_config(&mut r, profile)?;
        (width, height) = frame_size(&mut r)?;
        render_size = read_render_size(&mut r, width, height)?;
        refresh_frame_flags = 0xFF;
    } else {
        if !show_frame {
            intra_only = r.bit(H)?;
        }
        if !error_resilient {
            reset_frame_context = r.f(2, H)? as u8;
        }
        if intra_only {
            frame_sync_code(&mut r)?;
            color = if profile > 0 {
                color_config(&mut r, profile)?
            } else {
                ColorConfig {
                    bit_depth: 8,
                    color_space: ColorSpace::Bt601,
                    full_range: carried.color.is_some_and(|c| c.full_range),
                    subsampling_x: true,
                    subsampling_y: true,
                }
            };
            refresh_frame_flags = r.f(8, H)? as u8;
            (width, height) = frame_size(&mut r)?;
            render_size = read_render_size(&mut r, width, height)?;
        } else {
            color = carried
                .color
                .ok_or(Error::Invalid("an inter frame before any intra frame"))?;
            refresh_frame_flags = r.f(8, H)? as u8;
            for i in 0..3 {
                let idx = r.f(3, H)? as u8;
                if let (Some(slot), Some(bias)) =
                    (ref_frame_idx.get_mut(i), sign_bias.get_mut(i + 1))
                {
                    *slot = idx;
                    *bias = r.bit(H)?;
                }
            }
            let refs = match ref_frame_idx.map(|idx| slots.get(usize::from(idx)).copied().flatten())
            {
                [Some(last), Some(golden), Some(altref)] => [last, golden, altref],
                _ => {
                    return Err(Error::Invalid(
                        "ref_frame_idx names an empty reference slot",
                    ));
                }
            };
            if !refs.iter().all(|slot| slot.matches(profile, &color)) {
                return Err(Error::Invalid(
                    "a reference frame of another profile, bit depth, subsampling or colour space",
                ));
            }
            (width, height) = frame_size_with_refs(&mut r, &refs)?;
            if !refs.iter().any(|s| {
                2 * u64::from(width) >= u64::from(s.width)
                    && 2 * u64::from(height) >= u64::from(s.height)
                    && u64::from(width) <= 16 * u64::from(s.width)
                    && u64::from(height) <= 16 * u64::from(s.height)
            }) {
                return Err(Error::Invalid(
                    "no reference frame within the scaling ratios",
                ));
            }
            render_size = read_render_size(&mut r, width, height)?;
            allow_high_precision_mv = r.bit(H)?;
            interpolation_filter = if r.bit(H)? {
                InterpolationFilter::Switchable
            } else {
                match r.f(2, H)? {
                    0 => InterpolationFilter::EightTapSmooth,
                    1 => InterpolationFilter::EightTap,
                    2 => InterpolationFilter::EightTapSharp,
                    _ => InterpolationFilter::Bilinear,
                }
            };
        }
    }
    let (refresh_frame_context, frame_parallel) = if error_resilient {
        (false, true)
    } else {
        (r.bit(H)?, r.bit(H)?)
    };
    let coded_frame_context_idx = r.f(2, H)? as u8;
    let intra = key_frame || intra_only;
    let independent = intra || error_resilient;
    let frame_context_idx = if independent {
        0
    } else {
        coded_frame_context_idx
    };
    let mut carried = if independent {
        Carried::independent(Some(color))
    } else {
        *carried
    };
    if independent {
        sign_bias = [false; 4];
    }
    let loop_filter = loop_filter_params(&mut r, &mut carried)?;
    let quantization = quantization_params(&mut r)?;
    let segmentation = segmentation_params(&mut r, &carried)?;
    let tiles = tile_info(&mut r, width)?;
    let compressed_header_size = r.f(16, H)? as u16;
    let trailing_bits_zero = r.byte_align(H)?;
    let h = FrameHeader {
        profile,
        key_frame,
        show_frame,
        error_resilient,
        intra_only,
        reset_frame_context,
        color,
        width,
        height,
        render_size,
        refresh_frame_flags,
        ref_frame_idx,
        ref_frame_sign_bias: sign_bias,
        allow_high_precision_mv,
        interpolation_filter,
        refresh_frame_context,
        frame_parallel,
        frame_context_idx,
        coded_frame_context_idx,
        loop_filter,
        quantization,
        segmentation,
        tiles,
        compressed_header_size,
        uncompressed_size: r.position() / 8,
        trailing_bits_zero,
    };
    if compressed_header_size == 0 {
        return Err(Error::Invalid("header_size_in_bytes is 0"));
    }
    let bytes = data
        .get(h.uncompressed_size..h.uncompressed_size + usize::from(compressed_header_size))
        .ok_or(Error::Truncated("the compressed header"))?;
    let mut contexts = contexts.clone();
    contexts.reset(&h);
    let mut probs = contexts.load(h.frame_context_idx);
    let compressed = compressed::read(bytes, &h, &mut probs);
    Ok(Parsed {
        header: Header::Frame {
            header: Box::new(h),
            compressed,
            tiles_end_cleanly: None,
            picture: None,
        },
        probs: Some(Box::new((contexts, probs))),
    })
}

fn frame_sync_code(r: &mut BitReader<'_>) -> Result<()> {
    if r.f(24, "frame_sync_code")? != 0x49_83_42 {
        return Err(Error::Invalid("frame_sync_code is not 0x49 0x83 0x42"));
    }
    Ok(())
}

fn color_config(r: &mut BitReader<'_>, profile: u8) -> Result<ColorConfig> {
    const H: &str = "color_config";
    let bit_depth = if profile >= 2 {
        if r.bit(H)? { 12 } else { 10 }
    } else {
        8
    };
    let color_space = ColorSpace::from_code(r.f(3, H)?);
    let odd_profile = profile == 1 || profile == 3;
    let (full_range, subsampling_x, subsampling_y) = if color_space != ColorSpace::Rgb {
        let full_range = r.bit(H)?;
        if odd_profile {
            let sx = r.bit(H)?;
            let sy = r.bit(H)?;
            if r.bit(H)? {
                return Err(Error::Invalid("reserved_zero is not 0"));
            }
            if sx && sy {
                return Err(Error::Invalid("4:2:0 in profile 1 or 3"));
            }
            (full_range, sx, sy)
        } else {
            (full_range, true, true)
        }
    } else {
        if !odd_profile {
            return Err(Error::Invalid("sRGB in profile 0 or 2"));
        }
        if r.bit(H)? {
            return Err(Error::Invalid("reserved_zero is not 0"));
        }
        (true, false, false)
    };
    Ok(ColorConfig {
        bit_depth,
        color_space,
        full_range,
        subsampling_x,
        subsampling_y,
    })
}

fn frame_size(r: &mut BitReader<'_>) -> Result<(u32, u32)> {
    let width = r.f(16, "frame_size")? + 1;
    let height = r.f(16, "frame_size")? + 1;
    Ok((width, height))
}

fn read_render_size(r: &mut BitReader<'_>, width: u32, height: u32) -> Result<(u32, u32)> {
    if r.bit("render_size")? {
        Ok((r.f(16, "render_size")? + 1, r.f(16, "render_size")? + 1))
    } else {
        Ok((width, height))
    }
}

fn frame_size_with_refs(r: &mut BitReader<'_>, refs: &[Slot; 3]) -> Result<(u32, u32)> {
    for slot in refs {
        if r.bit("frame_size_with_refs")? {
            return Ok((slot.width, slot.height));
        }
    }
    frame_size(r)
}

fn loop_filter_params(r: &mut BitReader<'_>, carried: &mut Carried) -> Result<LoopFilter> {
    const H: &str = "loop_filter_params";
    let level = r.f(6, H)? as u8;
    let sharpness = r.f(3, H)? as u8;
    let delta_enabled = r.bit(H)?;
    if delta_enabled && r.bit(H)? {
        for delta in &mut carried.ref_deltas {
            if r.bit(H)? {
                *delta = r.s(6, H)? as i8;
            }
        }
        for delta in &mut carried.mode_deltas {
            if r.bit(H)? {
                *delta = r.s(6, H)? as i8;
            }
        }
    }
    Ok(LoopFilter {
        level,
        sharpness,
        delta_enabled,
        ref_deltas: carried.ref_deltas,
        mode_deltas: carried.mode_deltas,
    })
}

fn quantization_params(r: &mut BitReader<'_>) -> Result<Quantization> {
    const H: &str = "quantization_params";
    let base_q_idx = r.f(8, H)? as u8;
    let mut delta = || -> Result<i8> { Ok(if r.bit(H)? { r.s(4, H)? as i8 } else { 0 }) };
    Ok(Quantization {
        base_q_idx,
        delta_q_y_dc: delta()?,
        delta_q_uv_dc: delta()?,
        delta_q_uv_ac: delta()?,
    })
}

fn read_prob(r: &mut BitReader<'_>) -> Result<u8> {
    Ok(if r.bit("read_prob")? {
        r.f(8, "read_prob")? as u8
    } else {
        255
    })
}

fn segmentation_params(r: &mut BitReader<'_>, carried: &Carried) -> Result<Segmentation> {
    const H: &str = "segmentation_params";
    let mut seg = Segmentation {
        enabled: r.bit(H)?,
        update_map: false,
        tree_probs: [255; 7],
        pred_probs: [255; 3],
        temporal_update: false,
        abs_or_delta_update: carried.abs_or_delta_update,
        features: carried.features,
    };
    if !seg.enabled {
        return Ok(seg);
    }
    seg.update_map = r.bit(H)?;
    if seg.update_map {
        for p in &mut seg.tree_probs {
            *p = read_prob(r)?;
        }
        seg.temporal_update = r.bit(H)?;
        if seg.temporal_update {
            for p in &mut seg.pred_probs {
                *p = read_prob(r)?;
            }
        }
    }
    if r.bit(H)? {
        seg.abs_or_delta_update = r.bit(H)?;
        for segment in &mut seg.features {
            for ((feature, bits), signed) in
                segment.iter_mut().zip(FEATURE_BITS).zip(FEATURE_SIGNED)
            {
                *feature = None;
                if r.bit(H)? {
                    let mut value = r.f(bits, H)? as i16;
                    if signed && r.bit(H)? {
                        if seg.abs_or_delta_update {
                            return Err(Error::Invalid("a negative absolute segment feature"));
                        }
                        value = -value;
                    }
                    *feature = Some(value);
                }
            }
        }
    }
    Ok(seg)
}

fn tile_info(r: &mut BitReader<'_>, width: u32) -> Result<TileInfo> {
    const H: &str = "tile_info";
    let sb64_cols = width.div_ceil(64);
    let mut min_log2 = 0;
    while (MAX_TILE_WIDTH_B64 << min_log2) < sb64_cols {
        min_log2 += 1;
    }
    let mut max_log2 = 1;
    while (sb64_cols >> max_log2) >= MIN_TILE_WIDTH_B64 {
        max_log2 += 1;
    }
    max_log2 -= 1;
    let mut cols_log2 = min_log2;
    while cols_log2 < max_log2 && r.bit(H)? {
        cols_log2 += 1;
    }
    if cols_log2 > 6 {
        return Err(Error::Invalid("tile_cols_log2 is above 6"));
    }
    let mut rows_log2 = u8::from(r.bit(H)?);
    if rows_log2 == 1 {
        rows_log2 += u8::from(r.bit(H)?);
    }
    Ok(TileInfo {
        cols_log2: cols_log2 as u8,
        rows_log2,
    })
}

#[cfg(test)]
mod tests;

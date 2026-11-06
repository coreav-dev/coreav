use super::common;
use coreav_hevc::{DecodeLimits, DecodeSummary, DecodedPicture, Error, InterStats, Result};
use md5::{Digest, Md5};
use serde_json::{Value, json};
use std::io::Write;

pub(super) fn inter_stats(stats: InterStats) -> Value {
    json!({"skip_units":stats.skip_units,"prediction_units":stats.prediction_units,
        "p_units":stats.p_units,"b_units":stats.b_units,"bi_units":stats.bi_units,
        "merge_units":stats.merge_units,"amvp_lists":stats.amvp_lists,
        "temporal_units":stats.temporal_units,"weighted_units":stats.weighted_units,
        "weighted_changed_samples":stats.weighted_changed_samples,
        "fractional_luma_units":stats.fractional_luma_units,
        "fractional_chroma_units":stats.fractional_chroma_units,
        "negative_motion_units":stats.negative_motion_units,"long_term_units":stats.long_term_units,
        "list_modification_units":stats.list_modification_units,"partitions":stats.partitions,
        "boundary_strengths":stats.boundary_strengths,"boundary_changed_samples":stats.boundary_changed_samples})
}

pub(super) fn picture(picture: DecodedPicture<'_>, stream: &mut Md5) -> Result<(Value, u64)> {
    let info = picture.info;
    if !matches!(info.bit_depth, 8 | 10)
        || info.width == 0
        || info.height == 0
        || info.width % 2 != 0
        || info.height % 2 != 0
    {
        return Err(Error::InvalidData("decode_picture_geometry"));
    }
    let [left, right, top, bottom] = info.conformance_window;
    let visible_width = left
        .checked_add(right)
        .and_then(|n| n.checked_mul(2))
        .and_then(|n| info.width.checked_sub(n))
        .filter(|&n| n != 0)
        .ok_or(Error::InvalidData("decode_picture_crop"))?;
    let visible_height = top
        .checked_add(bottom)
        .and_then(|n| n.checked_mul(2))
        .and_then(|n| info.height.checked_sub(n))
        .filter(|&n| n != 0)
        .ok_or(Error::InvalidData("decode_picture_crop"))?;
    if (visible_width, visible_height) != (info.visible_width, info.visible_height) {
        return Err(Error::InvalidData("decode_picture_crop"));
    }
    let sample_bytes = if info.bit_depth == 8 { 1 } else { 2 };
    let mut visible_hash = Md5::new();
    let mut output_bytes = 0u64;
    let mut coded_components = Vec::new();
    let mut packed = [0u8; 4096];
    for (component, plane) in picture.planes.iter().enumerate() {
        let divisor = if component == 0 { 1 } else { 2 };
        let width =
            usize::try_from(info.width / divisor).map_err(|_| Error::Limit("decode_plane_size"))?;
        let height = usize::try_from(info.height / divisor)
            .map_err(|_| Error::Limit("decode_plane_size"))?;
        let visible_width = usize::try_from(visible_width / divisor)
            .map_err(|_| Error::Limit("decode_plane_size"))?;
        let visible_height = usize::try_from(visible_height / divisor)
            .map_err(|_| Error::Limit("decode_plane_size"))?;
        let crop_x = usize::try_from(left)
            .ok()
            .and_then(|n| n.checked_mul((2 / divisor) as usize))
            .ok_or(Error::Limit("decode_plane_size"))?;
        let crop_y = usize::try_from(top)
            .ok()
            .and_then(|n| n.checked_mul((2 / divisor) as usize))
            .ok_or(Error::Limit("decode_plane_size"))?;
        let end_x = crop_x
            .checked_add(visible_width)
            .ok_or(Error::Limit("decode_plane_size"))?;
        let end_y = crop_y
            .checked_add(visible_height)
            .ok_or(Error::Limit("decode_plane_size"))?;
        if plane.width != width
            || plane.height != height
            || plane.stride < width
            || end_x > width
            || end_y > height
        {
            return Err(Error::InvalidData("decode_plane_geometry"));
        }
        let mut coded_hash = Md5::new();
        let mut coded_bytes = 0u64;
        for y in 0..height {
            let start = y
                .checked_mul(plane.stride)
                .ok_or(Error::Limit("decode_plane_size"))?;
            let end = start
                .checked_add(width)
                .ok_or(Error::Limit("decode_plane_size"))?;
            let row = plane
                .samples
                .get(start..end)
                .ok_or(Error::InvalidData("decode_plane_samples"))?;
            let mut x = 0usize;
            for samples in row.chunks(2048) {
                for (index, &sample) in samples.iter().enumerate() {
                    if u32::from(sample) >= 1 << info.bit_depth {
                        return Err(Error::InvalidData("decode_sample_range"));
                    }
                    let bytes = sample.to_le_bytes();
                    packed
                        .get_mut(index * sample_bytes..(index + 1) * sample_bytes)
                        .ok_or(Error::InvalidData("decode_packing"))?
                        .copy_from_slice(
                            bytes
                                .get(..sample_bytes)
                                .ok_or(Error::InvalidData("decode_packing"))?,
                        );
                }
                let bytes = packed
                    .get(..samples.len() * sample_bytes)
                    .ok_or(Error::InvalidData("decode_packing"))?;
                coded_hash.update(bytes);
                coded_bytes = coded_bytes
                    .checked_add(bytes.len() as u64)
                    .ok_or(Error::Limit("decode_output_size"))?;
                if (crop_y..end_y).contains(&y) {
                    let start = crop_x.saturating_sub(x).min(samples.len()) * sample_bytes;
                    let end = end_x.saturating_sub(x).min(samples.len()) * sample_bytes;
                    let visible = bytes
                        .get(start..end)
                        .ok_or(Error::InvalidData("decode_picture_crop"))?;
                    visible_hash.update(visible);
                    stream.update(visible);
                    output_bytes = output_bytes
                        .checked_add(visible.len() as u64)
                        .ok_or(Error::Limit("decode_output_size"))?;
                }
                x = x
                    .checked_add(samples.len())
                    .ok_or(Error::Limit("decode_plane_size"))?;
            }
        }
        coded_components.push(json!({"width":width,"height":height,"bytes":coded_bytes,"md5":format!("{:x}",coded_hash.finalize())}));
    }
    Ok((
        json!({"type":"picture","picture_index":picture.index,"poc":picture.poc,"timestamp":picture.timestamp,
        "coded_width":info.width,"coded_height":info.height,"width":info.visible_width,"height":info.visible_height,
        "bit_depth":info.bit_depth,"profile_idc":info.profile_idc,"ctb_log2":info.ctb_log2,
        "conformance_window":info.conformance_window,"packing":if info.bit_depth==8 {"yuv420p"} else {"yuv420p10le"},
        "output_bytes":output_bytes,"output_md5":format!("{:x}",visible_hash.finalize()),"coded_components":coded_components,
        "color_declarations":{"primaries":info.vui.colour_primaries,"transfer":info.vui.transfer_characteristics,
            "matrix":info.vui.matrix_coeffs,"full_range":info.vui.full_range,
            "chroma_location":info.vui.chroma_location.map(|(top,_)|top)},
        "stats":common::reconstruction_stats(picture.stats),"inter":inter_stats(picture.inter)}),
        output_bytes,
    ))
}

pub(super) fn json_line(writer: &mut impl Write, value: &Value) -> Result<()> {
    serde_json::to_writer(&mut *writer, value)
        .map_err(|_| Error::Limit("decode_metadata_output"))?;
    writer
        .write_all(b"\n")
        .map_err(|_| Error::Limit("decode_metadata_output"))
}

pub(super) struct Output<'a, W> {
    writer: &'a mut W,
    hash: Md5,
    pub(super) pictures: u64,
    pub(super) bytes: u64,
}

impl<'a, W: Write> Output<'a, W> {
    pub(super) fn record(&mut self, value: &Value) -> Result<()> {
        json_line(self.writer, value)
    }

    pub(super) fn new(writer: &'a mut W) -> Self {
        Self {
            writer,
            hash: Md5::new(),
            pictures: 0,
            bytes: 0,
        }
    }

    pub(super) fn picture(&mut self, decoded: DecodedPicture<'_>) -> Result<()> {
        let (value, bytes) = picture(decoded, &mut self.hash)?;
        let pictures = self
            .pictures
            .checked_add(1)
            .ok_or(Error::Limit("decode_output_count"))?;
        let bytes = self
            .bytes
            .checked_add(bytes)
            .ok_or(Error::Limit("decode_output_size"))?;
        self.record(&value)?;
        self.pictures = pictures;
        self.bytes = bytes;
        Ok(())
    }

    pub(super) fn finish(
        self,
        summary: DecodeSummary,
        input: common::Receipt,
        limits: DecodeLimits,
    ) -> Result<()> {
        if self.pictures != summary.headers.output_pictures {
            return Err(Error::InvalidData("decode_output_count"));
        }
        let headers = summary.headers;
        json_line(
            self.writer,
            &json!({"type":"summary","complete":true,
            "headers":{"nals":headers.nals,"pictures":headers.pictures,
                "warnings":{"profile_bit_depth":headers.warnings.profile_bit_depth,
                    "invalid_sample_aspect_ratio":headers.warnings.invalid_sample_aspect_ratio,
                    "temporal_nesting_nal_type":headers.warnings.temporal_nesting_nal_type},
                "output_flag_pictures":headers.output_flag_pictures,"output_pictures":headers.output_pictures,
                "independent_slices":headers.independent_slices,"dependent_slices":headers.dependent_slices,
                "picture_hashes":headers.picture_hashes,"reserved_hashes":headers.reserved_hashes},
            "decoded_pictures":summary.decoded_pictures,"skipped_pictures":summary.skipped_pictures,
            "skipped_picture_hashes":summary.skipped_picture_hashes,"skipped_reserved_hashes":summary.skipped_reserved_hashes,
            "stats":common::reconstruction_stats(summary.reconstruction),"inter":inter_stats(summary.inter),
            "memory_bytes":summary.memory_bytes,"input_bytes":input.bytes,"input_capacity":input.capacity,
            "output_pictures":self.pictures,"output_bytes":self.bytes,"output_md5":format!("{:x}",self.hash.finalize()),
            "max_memory_bytes":limits.max_memory_bytes,"max_stream_bins":limits.max_stream_bins,
            "max_picture_bins":limits.max_picture_bins,"max_nal_bytes":limits.headers.max_nal_bytes,
            "input_byte_limit":common::INPUT_LIMIT}),
        )?;
        self.writer
            .flush()
            .map_err(|_| Error::Limit("decode_metadata_output"))
    }
}

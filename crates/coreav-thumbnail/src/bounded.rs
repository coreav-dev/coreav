use coreav_backend_h264::{NativeH264, OneIdrGeometry};
use coreav_decode::{Backend, Codec, Error as DecodeError, ErrorKind, Packet};
use coreav_isobmff::{PlanningBudget, Source};

use crate::{
    BuiltinEncoder, DisplayGeometry, Error, Thumbnail, ThumbnailOptions, negate,
    planned_with_planning, read_packet, render_bounded, select,
};

const PACKET_BYTES: u64 = 4 << 20;
const PHASE_BYTES: u64 = 32 << 20;

/// The bounded one-IDR profile used by the opt-in WebAssembly package.
/// Selection is the native selector; unsupported selected pictures are refused,
/// never replaced with another keyframe. This does not establish a host-memory
/// or hosted-runtime verdict.
#[doc(hidden)]
pub fn thumbnail_bounded<S: Source>(
    source: S,
    options: &ThumbnailOptions,
) -> Result<Thumbnail, Error> {
    thumbnail_bounded_with_budgets(source, options, 4 << 20, 4 << 20)
}

/// The same profile with lower-only planning and encoded-output byte budgets.
#[doc(hidden)]
pub fn thumbnail_bounded_with_budgets<S: Source>(
    mut source: S,
    options: &ThumbnailOptions,
    planning_bytes: usize,
    output_bytes: usize,
) -> Result<Thumbnail, Error> {
    if options.exact {
        return Err(Error::Unsupported(
            "exact thumbnails are outside this profile".into(),
        ));
    }
    let mut limits = options.decode_limits.clone();
    limits.max_width = limits.max_width.min(1920);
    limits.max_height = limits.max_height.min(1920);
    limits.max_pixels = limits.max_pixels.min(2_088_960);
    limits.max_frame_bytes = limits.max_frame_bytes.min(PACKET_BYTES);
    limits.max_decoded_bytes = limits.max_decoded_bytes.min(PACKET_BYTES);
    limits.max_pending_bytes = limits.max_pending_bytes.min(PACKET_BYTES);
    limits.max_packet_bytes = limits.max_packet_bytes.min(PACKET_BYTES);
    limits.max_memory_bytes = limits.max_memory_bytes.min(PHASE_BYTES);
    let planning = PlanningBudget::new(planning_bytes.min(4 << 20));
    let prepared = planned_with_planning(&mut source, options, &planning)?;
    let crate::Prepared { mut plan, choice } = prepared;
    if plan.stream.codec != Codec::H264 {
        return Err(Error::Unsupported(
            "this thumbnail profile decodes H.264 only".into(),
        ));
    }
    let sample = plan.samples.get(choice.target).ok_or_else(no_frame)?;
    if choice.start != choice.target || select::offered(sample).is_none() {
        return Err(Error::Unsupported(
            "the selected frame is not an offered keyframe".into(),
        ));
    }
    let time = sample
        .time
        .and_then(|t| t.checked_add(negate(plan.start)?))
        .ok_or_else(|| Error::Unsupported("selected presentation time is out of range".into()))?;
    let packet = *plan.packets.get(choice.target).ok_or_else(no_frame)?;
    let data = read_packet(&mut source, &plan, choice.target, limits.max_packet_bytes)?;
    let backend = NativeH264::one_idr_with_admission(admit_geometry);
    let mut decoder = backend.open(&plan.stream, &limits)?;
    decoder.send_packet(&Packet {
        data: &data,
        pts: Some(packet.pts),
        dts: packet.dts,
        keyframe: true,
        discard: false,
    })?;
    decoder.send_eof()?;
    let frame = decoder.receive_frame()?.ok_or_else(no_frame)?;
    if frame.pts != Some(packet.pts) || decoder.receive_frame()?.is_some() {
        return Err(no_frame());
    }
    let decoder_info = decoder.info().clone();
    drop(decoder);
    drop(data);
    let display = DisplayGeometry {
        quarter_turns: plan.quarter_turns,
        pixel_aspect: plan.pixel_aspect,
    };
    let stream_index = plan.stream_index;
    let warnings = std::mem::take(&mut plan.warnings);
    drop(plan);
    drop(planning);
    let rgb = render_bounded(&frame, display, options)?;
    drop(frame);
    let encoder = BuiltinEncoder {
        format: options.format,
        quality: options.quality,
    };
    let image = encoder.encode_bounded(&rgb, output_bytes.min(4 << 20))?;
    Ok(Thumbnail {
        image,
        format: options.format,
        width: rgb.width(),
        height: rgb.height(),
        stream_index,
        codec: Codec::H264,
        time,
        keyframe: true,
        decode_start: time,
        packet: (packet.offset, packet.size),
        warnings,
        packets_decoded: 1,
        decoder: decoder_info,
        skipped: Vec::new(),
    })
}

fn no_frame() -> Error {
    Error::Decode(DecodeError::new(
        ErrorKind::InvalidData,
        "one complete selected frame is required",
    ))
}

fn admit_geometry(
    geometry: OneIdrGeometry,
    limits: &coreav_decode::Limits,
) -> coreav_h264::Result<()> {
    let admitted = |width: u32, height: u32, pixels: u64| {
        width > 0
            && height > 0
            && width <= limits.max_width.min(1920)
            && height <= limits.max_height.min(1920)
            && u64::from(width) * u64::from(height) <= limits.max_pixels.min(pixels)
    };
    if !admitted(geometry.coded_width, geometry.coded_height, 2_088_960)
        || !admitted(geometry.visible_width, geometry.visible_height, 2_073_600)
    {
        return Err(coreav_h264::Error::Limit(
            coreav_h264::LimitKind::PictureSize,
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geometry(width: u32, height: u32) -> OneIdrGeometry {
        OneIdrGeometry {
            coded_width: width,
            coded_height: height,
            visible_left: 0,
            visible_top: 0,
            visible_width: width,
            visible_height: height,
        }
    }

    #[test]
    fn coded_and_visible_caps_are_independent() {
        let limits = coreav_decode::Limits::default();
        let mut value = geometry(1920, 1088);
        value.visible_height = 1080;
        assert!(admit_geometry(value, &limits).is_ok());
        let mut portrait = geometry(1088, 1920);
        portrait.visible_width = 1080;
        assert!(admit_geometry(portrait, &limits).is_ok());
        assert!(admit_geometry(geometry(1920, 1088), &limits).is_err());
        value.coded_width = 3840;
        value.coded_height = 2160;
        value.visible_width = 16;
        value.visible_height = 16;
        assert!(admit_geometry(value, &limits).is_err());
        assert!(admit_geometry(geometry(0, 16), &limits).is_err());
        assert!(admit_geometry(geometry(u32::MAX, u32::MAX), &limits).is_err());
    }

    #[test]
    fn caller_geometry_limits_are_applied_by_preallocation_admission() {
        let mut limits = coreav_decode::Limits::default();
        limits.max_width = 64;
        assert!(admit_geometry(geometry(1920, 16), &limits).is_err());
        limits.max_width = 1920;
        limits.max_height = 64;
        assert!(admit_geometry(geometry(16, 1920), &limits).is_err());
        limits.max_height = 1920;
        limits.max_pixels = 1024;
        assert!(admit_geometry(geometry(64, 32), &limits).is_err());
        assert!(admit_geometry(geometry(32, 32), &limits).is_ok());
        let mut cropped = geometry(64, 32);
        cropped.visible_width = 32;
        assert!(admit_geometry(cropped, &limits).is_err());
    }
}

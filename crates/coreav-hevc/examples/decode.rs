//! Bounded streaming picture hashes for complete Main/Main10 Annex B streams.

use coreav_hevc::{
    DecodeLimits, DecodeSummary, DecodedPicture, Error, InterStats, PictureDecoder, Result,
};
use md5::{Digest, Md5};
use serde_json::{Value, json};
use std::io::{Read, Write};

mod common;

fn inter_stats(stats: InterStats) -> Value {
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

fn picture(picture: DecodedPicture<'_>, stream: &mut Md5) -> Result<(Value, u64)> {
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
        "stats":common::reconstruction_stats(picture.stats),"inter":inter_stats(picture.inter)}),
        output_bytes,
    ))
}

fn json_line(writer: &mut impl Write, value: &Value) -> Result<()> {
    serde_json::to_writer(&mut *writer, value)
        .map_err(|_| Error::Limit("decode_metadata_output"))?;
    writer
        .write_all(b"\n")
        .map_err(|_| Error::Limit("decode_metadata_output"))
}

struct Output<'a, W> {
    writer: &'a mut W,
    hash: Md5,
    pictures: u64,
    bytes: u64,
}

impl<'a, W: Write> Output<'a, W> {
    fn new(writer: &'a mut W) -> Self {
        Self {
            writer,
            hash: Md5::new(),
            pictures: 0,
            bytes: 0,
        }
    }

    fn picture(&mut self, decoded: DecodedPicture<'_>) -> Result<()> {
        let (value, bytes) = picture(decoded, &mut self.hash)?;
        let pictures = self
            .pictures
            .checked_add(1)
            .ok_or(Error::Limit("decode_output_count"))?;
        let bytes = self
            .bytes
            .checked_add(bytes)
            .ok_or(Error::Limit("decode_output_size"))?;
        json_line(self.writer, &value)?;
        self.pictures = pictures;
        self.bytes = bytes;
        Ok(())
    }

    fn finish(
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
                "output_flag_pictures":headers.output_flag_pictures,"output_pictures":headers.output_pictures,
                "independent_slices":headers.independent_slices,"dependent_slices":headers.dependent_slices,
                "picture_hashes":headers.picture_hashes,"reserved_hashes":headers.reserved_hashes},
            "decoded_pictures":summary.decoded_pictures,"skipped_pictures":summary.skipped_pictures,
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

fn decode(source: impl Read, writer: &mut impl Write, limits: DecodeLimits) -> Result<()> {
    let mut decoder = PictureDecoder::new(limits);
    let mut output = Output::new(writer);
    let receipt = common::read_nals(source, limits.headers.max_nal_bytes, |_, bytes| {
        decoder.read_nal(bytes, None, |picture| output.picture(picture))
    })?;
    let summary = decoder.finish(|picture| output.picture(picture))?;
    output.finish(summary, receipt, limits)
}

fn number<T: std::str::FromStr>(value: Option<std::ffi::OsString>) -> Result<T> {
    value
        .and_then(|value| value.into_string().ok())
        .and_then(|value| value.parse().ok())
        .ok_or(Error::InvalidData("decode_arguments"))
}

fn run(writer: &mut impl Write) -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let source = args.next().ok_or(Error::InvalidData("decode_arguments"))?;
    let limits = DecodeLimits {
        max_memory_bytes: number(args.next())?,
        max_stream_bins: number(args.next())?,
        max_picture_bins: number(args.next())?,
        ..DecodeLimits::default()
    };
    if args.next().is_some() {
        return Err(Error::InvalidData("decode_arguments"));
    }
    if source == "-" {
        decode(std::io::stdin().lock(), writer, limits)
    } else {
        let file = std::fs::File::open(source).map_err(|_| Error::InvalidData("input_open"))?;
        decode(file, writer, limits)
    }
}

fn main() -> std::process::ExitCode {
    let mut stdout = std::io::BufWriter::new(std::io::stdout().lock());
    match run(&mut stdout) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            let (category, reason) = match error {
                Error::InvalidData(reason) => ("invalid_data", reason),
                Error::Unsupported(reason) => ("unsupported", reason),
                Error::Limit(reason) => ("limit", reason),
            };
            let mut stderr = std::io::stderr().lock();
            let _ = serde_json::to_writer(
                &mut stderr,
                &json!({"kind":"refusal","category":category,"reason":reason}),
            );
            let _ = stderr.write_all(b"\n");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use coreav_hevc::{IntraPlane, IntraStats, PictureInfo, Summary, Vui};

    struct Fixture {
        info: PictureInfo,
        planes: [Vec<u16>; 3],
        strides: [usize; 3],
    }

    impl Fixture {
        fn new(depth: u32) -> Self {
            let strides = [11, 7, 8];
            let mut planes = [Vec::new(), Vec::new(), Vec::new()];
            for (component, (plane, stride)) in planes.iter_mut().zip(strides).enumerate() {
                let side = if component == 0 { 8 } else { 4 };
                for y in 0..side {
                    for x in 0..stride {
                        plane.push(if x < side {
                            (component * 64 + y * side + x) as u16
                                + if depth == 10 { 512 } else { 0 }
                        } else {
                            u16::MAX
                        });
                    }
                }
            }
            Self {
                info: PictureInfo {
                    width: 8,
                    height: 8,
                    visible_width: 4,
                    visible_height: 4,
                    bit_depth: depth,
                    profile_idc: if depth == 8 { 1 } else { 2 },
                    ctb_log2: 3,
                    tiles: false,
                    wpp: false,
                    conformance_window: [1; 4],
                    vui: Vui::default(),
                },
                planes,
                strides,
            }
        }

        fn picture(&self, index: u64, poc: i32) -> DecodedPicture<'_> {
            DecodedPicture {
                index,
                poc,
                timestamp: Some(9000 + i64::from(poc)),
                info: self.info,
                planes: std::array::from_fn(|component| {
                    let side = if component == 0 { 8 } else { 4 };
                    IntraPlane {
                        samples: self.planes.get(component).map_or(&[], Vec::as_slice),
                        width: side,
                        height: side,
                        stride: self.strides.get(component).copied().unwrap_or(0),
                    }
                }),
                stats: IntraStats::default(),
                inter: InterStats::default(),
            }
        }
    }

    fn packed(depth: u32, values: &[u8]) -> Vec<u8> {
        if depth == 8 {
            values.to_vec()
        } else {
            values
                .iter()
                .flat_map(|&v| (u16::from(v) + 512).to_le_bytes())
                .collect()
        }
    }

    fn digest(bytes: &[u8]) -> String {
        format!("{:x}", Md5::digest(bytes))
    }

    fn visible(depth: u32) -> Vec<u8> {
        packed(
            depth,
            &[
                18, 19, 20, 21, 26, 27, 28, 29, 34, 35, 36, 37, 42, 43, 44, 45, 69, 70, 73, 74,
                133, 134, 137, 138,
            ],
        )
    }

    fn record(fixture: &Fixture) -> Result<Value> {
        let mut stream = Md5::new();
        let (record, bytes) = picture(fixture.picture(7, -3), &mut stream)?;
        assert_eq!(record.get("output_bytes"), Some(&json!(bytes)));
        assert_eq!(
            record.get("output_md5"),
            Some(&json!(format!("{:x}", stream.finalize())))
        );
        Ok(record)
    }

    #[test]
    fn cropped_visible_and_coded_md5s_preserve_depth_packing_and_stride() -> Result<()> {
        for depth in [8, 10] {
            let fixture = Fixture::new(depth);
            let actual = record(&fixture)?;
            let expected = visible(depth);
            assert_eq!(actual.get("type"), Some(&json!("picture")));
            assert_eq!(actual.get("picture_index"), Some(&json!(7)));
            assert_eq!(actual.get("poc"), Some(&json!(-3)));
            assert_eq!(actual.get("timestamp"), Some(&json!(8997)));
            assert_eq!(
                actual.get("packing"),
                Some(&json!(if depth == 8 { "yuv420p" } else { "yuv420p10le" }))
            );
            assert_eq!(actual.get("output_bytes"), Some(&json!(expected.len())));
            assert_eq!(actual.get("output_md5"), Some(&json!(digest(&expected))));
            let components = actual
                .get("coded_components")
                .and_then(Value::as_array)
                .ok_or(Error::InvalidData("test_components"))?;
            assert_eq!(components.len(), 3);
            for (component, value) in components.iter().enumerate() {
                let side = if component == 0 { 8 } else { 4 };
                let samples: Vec<_> = (component * 64..component * 64 + side * side)
                    .map(|v| v as u8)
                    .collect();
                let expected = packed(depth, &samples);
                assert_eq!(
                    value,
                    &json!({"width":side,"height":side,"bytes":expected.len(),"md5":digest(&expected)})
                );
            }
        }
        Ok(())
    }

    #[test]
    fn inter_counter_projection_preserves_every_current_field() {
        let actual = inter_stats(InterStats {
            skip_units: 1,
            prediction_units: 2,
            p_units: 3,
            b_units: 4,
            bi_units: 5,
            merge_units: 6,
            amvp_lists: 7,
            temporal_units: 8,
            weighted_units: 9,
            weighted_changed_samples: [10, 11, 12],
            fractional_luma_units: 13,
            fractional_chroma_units: 14,
            negative_motion_units: 15,
            long_term_units: 16,
            list_modification_units: 17,
            partitions: [18, 19, 20, 21, 22, 23, 24, 25],
            boundary_strengths: [26, 27, 28],
            boundary_changed_samples: [[29, 30, 31], [32, 33, 34], [35, 36, 37]],
        });
        assert_eq!(
            actual,
            json!({"skip_units":1,"prediction_units":2,"p_units":3,"b_units":4,"bi_units":5,
            "merge_units":6,"amvp_lists":7,"temporal_units":8,"weighted_units":9,"weighted_changed_samples":[10,11,12],
            "fractional_luma_units":13,"fractional_chroma_units":14,"negative_motion_units":15,"long_term_units":16,
            "list_modification_units":17,"partitions":[18,19,20,21,22,23,24,25],"boundary_strengths":[26,27,28],
            "boundary_changed_samples":[[29,30,31],[32,33,34],[35,36,37]]})
        );
    }

    #[test]
    fn padding_is_ignored_but_cropped_samples_and_high_bytes_are_not() -> Result<()> {
        let mut fixture = Fixture::new(10);
        let original = record(&fixture)?;
        *fixture
            .planes
            .first_mut()
            .and_then(|p| p.get_mut(8))
            .ok_or(Error::InvalidData("test_sample"))? = 999;
        assert_eq!(record(&fixture)?, original);
        *fixture
            .planes
            .first_mut()
            .and_then(|p| p.first_mut())
            .ok_or(Error::InvalidData("test_sample"))? += 256;
        let cropped_change = record(&fixture)?;
        assert_eq!(cropped_change.get("output_md5"), original.get("output_md5"));
        assert_ne!(
            cropped_change.get("coded_components"),
            original.get("coded_components")
        );
        *fixture
            .planes
            .first_mut()
            .and_then(|p| p.get_mut(24))
            .ok_or(Error::InvalidData("test_sample"))? += 256;
        let visible_change = record(&fixture)?;
        assert_ne!(visible_change.get("output_md5"), original.get("output_md5"));
        assert_ne!(
            visible_change.get("coded_components"),
            cropped_change.get("coded_components")
        );
        Ok(())
    }

    #[test]
    fn uncropped_and_chunk_crossing_rows_have_exact_planar_hashes() -> Result<()> {
        for depth in [8, 10] {
            let mut fixture = Fixture::new(depth);
            fixture.info.conformance_window = [0; 4];
            fixture.info.visible_width = 8;
            fixture.info.visible_height = 8;
            let expected: Vec<_> = [0..64, 64..80, 128..144]
                .into_iter()
                .flatten()
                .map(|v| v as u8)
                .collect();
            let actual = record(&fixture)?;
            assert_eq!(
                actual.get("output_md5"),
                Some(&json!(digest(&packed(depth, &expected))))
            );
        }
        let depth = 10;
        let info = PictureInfo {
            width: 2064,
            height: 4,
            visible_width: 16,
            visible_height: 4,
            conformance_window: [1023, 1, 0, 0],
            ..Fixture::new(depth).info
        };
        let y = vec![513; 2070 * 4];
        let cb = vec![514; 1035 * 2];
        let cr = vec![515; 1036 * 2];
        let fixture = Fixture::new(depth);
        let mut decoded = fixture.picture(0, 0);
        decoded.info = info;
        decoded.planes = [
            IntraPlane {
                samples: &y,
                width: 2064,
                height: 4,
                stride: 2070,
            },
            IntraPlane {
                samples: &cb,
                width: 1032,
                height: 2,
                stride: 1035,
            },
            IntraPlane {
                samples: &cr,
                width: 1032,
                height: 2,
                stride: 1036,
            },
        ];
        let mut stream = Md5::new();
        let (actual, bytes) = picture(decoded, &mut stream)?;
        let mut expected = Vec::new();
        for (sample, count) in [(513u16, 64), (514, 16), (515, 16)] {
            for _ in 0..count {
                expected.extend(sample.to_le_bytes());
            }
        }
        assert_eq!(bytes, expected.len() as u64);
        assert_eq!(actual.get("output_md5"), Some(&json!(digest(&expected))));
        Ok(())
    }

    #[test]
    fn invalid_depth_geometry_crop_domain_and_short_planes_fail_closed() -> Result<()> {
        for case in 0..9 {
            let fixture = Fixture::new(8);
            let mut decoded = fixture.picture(0, 0);
            match case {
                0 => decoded.info.bit_depth = 12,
                1 => decoded.info.width = 7,
                2 => decoded.info.conformance_window = [u32::MAX, 1, 0, 0],
                3 => decoded.info.conformance_window = [4, 0, 0, 0],
                4 => decoded.info.visible_width = 8,
                5 => {
                    if let Some(plane) = decoded.planes.first_mut() {
                        plane.width = 7;
                    }
                }
                6 => {
                    if let Some(plane) = decoded.planes.first_mut() {
                        plane.stride = 7;
                    }
                }
                7 => {
                    if let Some(plane) = decoded.planes.first_mut() {
                        plane.samples = &[];
                    }
                }
                _ => {
                    if let Some(plane) = decoded.planes.first_mut() {
                        plane.stride = usize::MAX;
                    }
                }
            }
            assert!(picture(decoded, &mut Md5::new()).is_err());
        }
        for depth in [8, 10] {
            let mut fixture = Fixture::new(depth);
            *fixture
                .planes
                .first_mut()
                .and_then(|p| p.first_mut())
                .ok_or(Error::InvalidData("test_sample"))? = 1 << depth;
            assert_eq!(
                record(&fixture),
                Err(Error::InvalidData("decode_sample_range"))
            );
        }
        Ok(())
    }

    fn summary(count: u64) -> DecodeSummary {
        DecodeSummary {
            headers: Summary {
                nals: 10,
                pictures: 3,
                output_flag_pictures: 3,
                output_pictures: count,
                independent_slices: 3,
                dependent_slices: 1,
                picture_hashes: 3,
                reserved_hashes: 1,
            },
            decoded_pictures: 3,
            skipped_pictures: 0,
            reconstruction: IntraStats {
                ctus: 3,
                bins: 9,
                ..IntraStats::default()
            },
            inter: InterStats {
                prediction_units: 2,
                ..InterStats::default()
            },
            memory_bytes: 4096,
        }
    }

    fn lines(bytes: &[u8]) -> Result<Vec<Value>> {
        bytes
            .split(|&b| b == b'\n')
            .filter(|line| !line.is_empty())
            .map(|line| serde_json::from_slice(line).map_err(|_| Error::InvalidData("test_json")))
            .collect()
    }

    #[test]
    fn success_has_one_final_summary_and_a_whole_presentation_order_anchor() -> Result<()> {
        let eight = Fixture::new(8);
        let ten = Fixture::new(10);
        let mut writer = Vec::new();
        let mut output = Output::new(&mut writer);
        output.picture(eight.picture(7, -3))?;
        output.picture(ten.picture(2, 4))?;
        let limits = DecodeLimits {
            max_memory_bytes: 8000,
            max_stream_bins: 100,
            max_picture_bins: 50,
            ..DecodeLimits::default()
        };
        output.finish(
            summary(2),
            common::Receipt {
                bytes: 1234,
                capacity: 900,
            },
            limits,
        )?;
        let records = lines(&writer)?;
        assert_eq!(records.len(), 3);
        assert_eq!(
            records.first().and_then(|r| r.get("picture_index")),
            Some(&json!(7))
        );
        assert_eq!(
            records.get(1).and_then(|r| r.get("picture_index")),
            Some(&json!(2))
        );
        let actual = records.last().ok_or(Error::InvalidData("test_summary"))?;
        assert_eq!(actual.get("type"), Some(&json!("summary")));
        assert_eq!(actual.get("complete"), Some(&json!(true)));
        let mut expected = visible(8);
        expected.extend(visible(10));
        assert_eq!(actual.get("output_md5"), Some(&json!(digest(&expected))));
        assert_eq!(actual.get("output_bytes"), Some(&json!(72)));
        assert_eq!(actual.get("output_pictures"), Some(&json!(2)));
        assert_eq!(
            actual.get("headers"),
            Some(
                &json!({"nals":10,"pictures":3,"output_flag_pictures":3,"output_pictures":2,
            "independent_slices":3,"dependent_slices":1,"picture_hashes":3,"reserved_hashes":1})
            )
        );
        for (key, value) in [
            ("decoded_pictures", 3),
            ("skipped_pictures", 0),
            ("memory_bytes", 4096),
            ("input_bytes", 1234),
            ("input_capacity", 900),
            ("max_memory_bytes", 8000),
            ("max_stream_bins", 100),
            ("max_picture_bins", 50),
            ("max_nal_bytes", 16 << 20),
            ("input_byte_limit", 4u64 << 30),
        ] {
            assert_eq!(actual.get(key), Some(&json!(value)));
        }
        assert_eq!(
            actual.get("stats").and_then(|v| v.get("bins")),
            Some(&json!(9))
        );
        assert_eq!(
            actual.get("inter").and_then(|v| v.get("prediction_units")),
            Some(&json!(2))
        );
        Ok(())
    }

    #[test]
    fn failure_keeps_only_partial_diagnostics_and_never_writes_complete_summary() -> Result<()> {
        let fixture = Fixture::new(8);
        let mut writer = Vec::new();
        let attempt = (|| {
            let mut output = Output::new(&mut writer);
            output.picture(fixture.picture(0, 0))?;
            let mut invalid = fixture.picture(1, 1);
            invalid.info.visible_width = 8;
            output.picture(invalid)?;
            output.finish(
                summary(2),
                common::Receipt {
                    bytes: 0,
                    capacity: 0,
                },
                DecodeLimits::default(),
            )
        })();
        assert!(attempt.is_err());
        let records = lines(&writer)?;
        assert_eq!(records.len(), 1);
        assert!(
            records
                .iter()
                .all(|r| r.get("type") == Some(&json!("picture")) && r.get("complete").is_none())
        );
        let mut writer = Vec::new();
        assert!(decode(std::io::empty(), &mut writer, DecodeLimits::default()).is_err());
        assert!(writer.is_empty());
        let output = Output::new(&mut writer);
        assert_eq!(
            output.finish(
                summary(1),
                common::Receipt {
                    bytes: 0,
                    capacity: 0
                },
                DecodeLimits::default()
            ),
            Err(Error::InvalidData("decode_output_count"))
        );
        assert!(writer.is_empty());
        Ok(())
    }

    struct FailedWriter;
    impl Write for FailedWriter {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::ErrorKind::BrokenPipe.into())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Err(std::io::ErrorKind::BrokenPipe.into())
        }
    }

    #[test]
    fn output_counts_and_io_failures_are_checked() -> Result<()> {
        let fixture = Fixture::new(8);
        let mut writer = Vec::new();
        let mut output = Output::new(&mut writer);
        output.pictures = u64::MAX;
        assert_eq!(
            output.picture(fixture.picture(0, 0)),
            Err(Error::Limit("decode_output_count"))
        );
        assert!(writer.is_empty());
        let mut output = Output::new(&mut writer);
        output.bytes = u64::MAX;
        assert_eq!(
            output.picture(fixture.picture(0, 0)),
            Err(Error::Limit("decode_output_size"))
        );
        assert!(writer.is_empty());
        assert_eq!(
            Output::new(&mut FailedWriter).picture(fixture.picture(0, 0)),
            Err(Error::Limit("decode_metadata_output"))
        );
        Ok(())
    }
}

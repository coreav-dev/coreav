//! Bounded streaming picture hashes for complete Main/Main10 Annex B streams.

use coreav_hevc::{DecodeLimits, Error, PictureDecoder, Result};
use serde_json::json;
use std::io::{Read, Write};

#[cfg(test)]
use coreav_hevc::{DecodeSummary, DecodedPicture, InterStats};
#[cfg(test)]
use md5::{Digest, Md5};
#[cfg(test)]
use serde_json::Value;

mod common;
#[path = "common/decoded.rs"]
mod decoded;
use decoded::Output;
#[cfg(test)]
use decoded::{inter_stats, picture};

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
                origin: None,
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
                warnings: coreav_hevc::Warnings {
                    profile_bit_depth: true,
                    invalid_sample_aspect_ratio: true,
                    temporal_nesting_nal_type: true,
                },
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
            skipped_picture_hashes: 0,
            skipped_reserved_hashes: 0,
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
            "independent_slices":3,"dependent_slices":1,"picture_hashes":3,"reserved_hashes":1,
            "warnings":{"profile_bit_depth":true,"invalid_sample_aspect_ratio":true,"temporal_nesting_nal_type":true}})
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

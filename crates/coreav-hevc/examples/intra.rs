//! Bounded u16le output for one original HEVC intra conformance picture.

use coreav_hevc::{Error, IntraLimits, IntraPicture, IntraPictureDecoder};
use serde_json::{Value, json};
use std::io::Write;
use std::path::PathBuf;

mod common;

struct Output {
    path: PathBuf,
    writer: Option<std::io::BufWriter<std::fs::File>>,
    keep: bool,
}

impl Drop for Output {
    fn drop(&mut self) {
        if !self.keep {
            drop(self.writer.take());
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

fn number<T: std::str::FromStr>(value: Option<std::ffi::OsString>) -> coreav_hevc::Result<T> {
    value
        .and_then(|value| value.into_string().ok())
        .and_then(|value| value.parse().ok())
        .ok_or(Error::InvalidData("intra_arguments"))
}

fn picture(
    picture: IntraPicture<'_>,
    target: u64,
    writer: &mut impl Write,
) -> coreav_hevc::Result<Value> {
    if picture.index != target || !matches!(picture.info.bit_depth, 8 | 10) {
        return Err(Error::InvalidData("intra_picture_identity"));
    }
    let mut bytes = 0usize;
    for (component, plane) in picture.planes.iter().enumerate() {
        let divisor = if component == 0 { 1 } else { 2 };
        let width = (picture.info.width as usize).div_ceil(divisor);
        let height = (picture.info.height as usize).div_ceil(divisor);
        if plane.width != width || plane.height != height || plane.stride < width {
            return Err(Error::InvalidData("intra_plane_geometry"));
        }
        for y in 0..height {
            let start = y
                .checked_mul(plane.stride)
                .ok_or(Error::Limit("intra_plane_size"))?;
            let end = start
                .checked_add(width)
                .ok_or(Error::Limit("intra_plane_size"))?;
            for &sample in plane
                .samples
                .get(start..end)
                .ok_or(Error::InvalidData("intra_plane_samples"))?
            {
                if u32::from(sample) >= 1 << picture.info.bit_depth {
                    return Err(Error::InvalidData("intra_sample_range"));
                }
                writer
                    .write_all(&sample.to_le_bytes())
                    .map_err(|_| Error::Limit("intra_output"))?;
                bytes = bytes
                    .checked_add(2)
                    .ok_or(Error::Limit("intra_output_size"))?;
            }
        }
    }
    let planes = picture
        .planes
        .map(|plane| json!({"width":plane.width,"height":plane.height}));
    let stats = picture.stats;
    Ok(
        json!({"picture_index":picture.index,"coded_width":picture.info.width,
        "coded_height":picture.info.height,"width":picture.info.visible_width,
        "height":picture.info.visible_height,"bit_depth":picture.info.bit_depth,
        "ctb_log2":picture.info.ctb_log2,
        "profile_idc":picture.info.profile_idc,"conformance_window":picture.info.conformance_window,
        "planes":planes,"output_bytes":bytes,"packing":"planar_y_cb_cr_u16le",
        "stats":common::reconstruction_stats(stats)}),
    )
}

fn run(stdout: &mut impl Write) -> coreav_hevc::Result<()> {
    let mut args = std::env::args_os().skip(1);
    let source = args.next().ok_or(Error::InvalidData("intra_arguments"))?;
    let target: u64 = number(args.next())?;
    let destination = PathBuf::from(args.next().ok_or(Error::InvalidData("intra_arguments"))?);
    let max_memory_bytes: usize = number(args.next())?;
    let max_bins: u64 = number(args.next())?;
    if args.next().is_some() {
        return Err(Error::InvalidData("intra_arguments"));
    }
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&destination)
        .map_err(|_| Error::InvalidData("intra_output_create"))?;
    let mut output = Output {
        path: destination,
        writer: Some(std::io::BufWriter::new(file)),
        keep: false,
    };
    let source = std::fs::File::open(&source).map_err(|_| Error::InvalidData("input_open"))?;
    let limits = IntraLimits {
        max_memory_bytes,
        max_bins,
        ..IntraLimits::default()
    };
    let mut decoder = IntraPictureDecoder::new(limits, target);
    let mut metadata = None;
    let mut emit = |value: IntraPicture<'_>| {
        if metadata.is_some() {
            return Err(Error::InvalidData("intra_duplicate_picture"));
        }
        metadata = Some(picture(
            value,
            target,
            output
                .writer
                .as_mut()
                .ok_or(Error::InvalidData("intra_output_closed"))?,
        )?);
        Ok(())
    };
    let receipt = common::read_nals(source, limits.headers.max_nal_bytes, |_, bytes| {
        decoder.read_nal(bytes, &mut emit)
    })?;
    let summary = decoder.finish(&mut emit)?;
    let mut metadata = metadata.ok_or(Error::InvalidData("intra_target_absent"))?;
    let value = metadata
        .as_object_mut()
        .ok_or(Error::InvalidData("intra_metadata"))?;
    value.insert("kind".into(), json!("intra_picture"));
    value.insert("complete".into(), json!(true));
    value.insert("input_bytes".into(), json!(receipt.bytes));
    value.insert(
        "caller_input_capacity_bytes".into(),
        json!(receipt.capacity),
    );
    value.insert("input_byte_limit".into(), json!(common::INPUT_LIMIT));
    value.insert("max_nal_bytes".into(), json!(limits.headers.max_nal_bytes));
    value.insert("nals".into(), json!(summary.nals));
    value.insert("pictures".into(), json!(summary.pictures));
    value.insert("max_memory_bytes".into(), json!(max_memory_bytes));
    value.insert("max_bins".into(), json!(max_bins));
    output
        .writer
        .as_mut()
        .ok_or(Error::InvalidData("intra_output_closed"))?
        .flush()
        .map_err(|_| Error::Limit("intra_output"))?;
    serde_json::to_writer(&mut *stdout, &metadata)
        .map_err(|_| Error::Limit("intra_metadata_output"))?;
    stdout
        .write_all(b"\n")
        .and_then(|()| stdout.flush())
        .map_err(|_| Error::Limit("intra_metadata_output"))?;
    output.keep = true;
    Ok(())
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
                Error::Internal(reason) => ("backend", reason),
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

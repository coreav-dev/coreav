use coreav_h264::{AnnexB, Decoder, Error, Limits, Picture};
use serde_json::json;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Write};
use std::time::{Duration, Instant};

struct Output<W: Write> {
    writer: W,
    dimensions: Option<(usize, usize)>,
    frames: u64,
    bytes: u64,
}

impl<W: Write> Output<W> {
    fn picture(&mut self, picture: &Picture) -> coreav_h264::Result<()> {
        let dimensions = (picture.width(), picture.height());
        if dimensions.0 == 0
            || dimensions.1 == 0
            || dimensions.0 > 4096
            || dimensions.1 > 2304
            || dimensions.0 % 2 != 0
            || dimensions.1 % 2 != 0
            || self.dimensions.is_some_and(|old| old != dimensions)
            || self.frames >= 36_000
        {
            return Err(Error::Invalid("decoded geometry or frame count"));
        }
        let bytes = (dimensions.0 as u64)
            .checked_mul(dimensions.1 as u64)
            .and_then(|n| n.checked_mul(3))
            .map(|n| n / 2)
            .and_then(|n| self.bytes.checked_add(n))
            .filter(|&n| n <= 32 * 1024 * 1024 * 1024)
            .ok_or(Error::Invalid("decoded output byte limit"))?;
        let (u, v) = picture.chroma();
        for plane in [picture.luma(), u, v] {
            for row in plane.rows() {
                self.writer
                    .write_all(row)
                    .map_err(|_| Error::Invalid("decoded output I/O"))?;
            }
        }
        self.dimensions = Some(dimensions);
        self.frames += 1;
        self.bytes = bytes;
        Ok(())
    }
}

pub(super) fn decode(
    source: &mut impl Read,
    output: &mut impl Write,
) -> Result<(usize, usize, u64), String> {
    let limits = Limits {
        max_mbs: 256 * 144,
        max_nal_bytes: 4 * 1024 * 1024,
        max_threads: 1,
        max_memory_bytes: 512 * 1024 * 1024,
        ..Limits::default()
    };
    let mut decoder = Decoder::new(limits);
    let mut splitter = AnnexB::new(&limits);
    let mut sink = Output {
        writer: output,
        dimensions: None,
        frames: 0,
        bytes: 0,
    };
    let deadline = Instant::now() + Duration::from_secs(900);
    let mut unit = |nal: &[u8]| -> coreav_h264::Result<()> {
        if Instant::now() >= deadline {
            return Err(Error::Invalid("decode deadline"));
        }
        decoder.decode_nal(nal)?;
        if !decoder.warnings().is_empty() || decoder.warnings_dropped() != 0 {
            return Err(Error::Invalid("decoder warning"));
        }
        for picture in decoder.take_pictures() {
            sink.picture(&picture)?;
        }
        Ok(())
    };
    let mut buffer = [0; 16 * 1024];
    let mut input_bytes = 0u64;
    loop {
        let count = source
            .read(&mut buffer)
            .map_err(|_| "cannot read elementary input")?;
        if count == 0 {
            break;
        }
        input_bytes = input_bytes
            .checked_add(count as u64)
            .filter(|&n| n <= 1024 * 1024 * 1024)
            .ok_or("elementary input byte limit")?;
        splitter
            .push(
                buffer.get(..count).ok_or("invalid input extent")?,
                &mut unit,
            )
            .map_err(|error| error.to_string())?;
    }
    splitter
        .finish(&mut unit)
        .map_err(|error| error.to_string())?;
    decoder.flush().map_err(|error| error.to_string())?;
    if !decoder.warnings().is_empty() || decoder.warnings_dropped() != 0 {
        return Err("decoder warning at completion".into());
    }
    for picture in decoder.take_pictures() {
        sink.picture(&picture).map_err(|error| error.to_string())?;
    }
    if Instant::now() >= deadline {
        return Err("decode deadline".into());
    }
    sink.writer
        .flush()
        .map_err(|_| "cannot flush decoded output")?;
    let (width, height) = sink.dimensions.ok_or("no complete decoded frame")?;
    Ok((width, height, sink.frames))
}

pub(super) fn run() -> Result<(), String> {
    let mut options = BTreeMap::new();
    let mut args = std::env::args().skip(2);
    while let Some(key) = args.next() {
        let value = args.next().ok_or("decode option requires a value")?;
        if options.insert(key, value).is_some() {
            return Err("repeated decode option".into());
        }
    }
    let input = options
        .remove("--input")
        .ok_or("missing elementary input")?;
    let output = options.remove("--output").ok_or("missing decoded output")?;
    let report = options.remove("--report").ok_or("missing decode report")?;
    if !options.is_empty() {
        return Err("unknown decode option".into());
    }
    let mut input = File::open(input).map_err(|_| "cannot open elementary input")?;
    let mut output = File::options()
        .write(true)
        .create_new(true)
        .open(output)
        .map_err(|_| "cannot create decoded output")?;
    let (width, height, frames) = decode(&mut input, &mut output)?;
    let mut report = File::options()
        .write(true)
        .create_new(true)
        .open(report)
        .map_err(|_| "cannot create decoder report")?;
    serde_json::to_writer(
        &mut report,
        &json!({
            "schema": "coreav.videotoolbox-decode/1", "width": width, "height": height,
            "frames": frames, "warnings": 0,
        }),
    )
    .map_err(|_| "cannot write decoder report")?;
    report
        .write_all(b"\n")
        .map_err(|_| "cannot finish decoder report".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_and_truncated_streams_never_report_success() {
        for bytes in [&[][..], &[0, 0, 0, 1, 0x67][..]] {
            assert!(decode(&mut bytes.as_ref(), &mut Vec::new()).is_err());
        }
    }
}

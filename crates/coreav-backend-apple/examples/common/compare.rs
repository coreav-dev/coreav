use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Write};

fn measure(
    source: &mut impl Read,
    decoded: &mut impl Read,
    width: u32,
    height: u32,
    frames: u32,
) -> Result<Value, String> {
    if width == 0
        || height == 0
        || width > 4096
        || height > 2304
        || width % 2 != 0
        || height % 2 != 0
        || frames == 0
        || frames > 36_000
    {
        return Err("invalid comparison geometry or frame count".into());
    }
    let samples = u64::from(width) * u64::from(height) * 3 / 2;
    if samples * u64::from(frames) > 32 * 1024 * 1024 * 1024 {
        return Err("comparison byte limit".into());
    }
    let mut reference = [0; 65_536];
    let mut candidate = [0; 65_536];
    let mut records = Vec::new();
    records
        .try_reserve_exact(frames as usize)
        .map_err(|_| "cannot reserve error records")?;
    for id in 0..frames {
        let mut remaining = usize::try_from(samples).map_err(|_| "comparison frame byte limit")?;
        let mut squared_error = 0u64;
        while remaining != 0 {
            let count = remaining.min(reference.len());
            let left = reference
                .get_mut(..count)
                .ok_or("comparison buffer extent")?;
            let right = candidate
                .get_mut(..count)
                .ok_or("comparison buffer extent")?;
            source
                .read_exact(left)
                .map_err(|_| "incomplete comparison source")?;
            decoded
                .read_exact(right)
                .map_err(|_| "incomplete comparison decoded frame")?;
            let chunk_error: u64 = left
                .iter()
                .zip(right.iter())
                .map(|(&a, &b)| u64::from(a.abs_diff(b)).pow(2))
                .sum();
            squared_error = squared_error
                .checked_add(chunk_error)
                .ok_or("squared error overflow")?;
            remaining -= count;
        }
        records.push(json!({"id": id, "samples": samples, "squared_error": squared_error}));
    }
    if source
        .read(&mut [0; 1])
        .map_err(|_| "cannot check comparison source end")?
        != 0
        || decoded
            .read(&mut [0; 1])
            .map_err(|_| "cannot check comparison decoded end")?
            != 0
    {
        return Err("unexpected comparison trailing bytes".into());
    }
    Ok(
        json!({"schema": "coreav.videotoolbox-error/1", "width": width, "height": height, "frames": records}),
    )
}

pub(super) fn run() -> Result<(), String> {
    let mut options = BTreeMap::new();
    let mut args = std::env::args().skip(2);
    while let Some(key) = args.next() {
        let value = args.next().ok_or("comparison option requires a value")?;
        if options.insert(key, value).is_some() {
            return Err("repeated comparison option".into());
        }
    }
    let source = options
        .remove("--source")
        .ok_or("missing comparison source")?;
    let decoded = options
        .remove("--decoded")
        .ok_or("missing comparison decoded file")?;
    let report = options
        .remove("--report")
        .ok_or("missing comparison report")?;
    let mut number = |name: &str| -> Result<u32, String> {
        options
            .remove(name)
            .ok_or_else(|| format!("missing {name}"))?
            .parse()
            .map_err(|_| "invalid comparison number".into())
    };
    let (width, height, frames) = (number("--width")?, number("--height")?, number("--frames")?);
    if !options.is_empty() {
        return Err("unknown comparison option".into());
    }
    let mut source = File::open(source).map_err(|_| "cannot open comparison source")?;
    let mut decoded = File::open(decoded).map_err(|_| "cannot open comparison decoded file")?;
    let value = measure(&mut source, &mut decoded, width, height, frames)?;
    let mut report = File::options()
        .write(true)
        .create_new(true)
        .open(report)
        .map_err(|_| "cannot create comparison report")?;
    serde_json::to_writer(&mut report, &value).map_err(|_| "cannot write comparison report")?;
    report
        .write_all(b"\n")
        .map_err(|_| "cannot finish comparison report".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_planes_and_perfect_frames_keep_exact_integer_errors() {
        let source = [0; 12];
        let decoded = [0, 0, 0, 0, 0, 0, 255, 255, 255, 255, 255, 255];
        let result = measure(&mut source.as_slice(), &mut decoded.as_slice(), 2, 2, 2);
        assert_eq!(
            result,
            Ok(json!({
                "schema": "coreav.videotoolbox-error/1", "width": 2, "height": 2,
                "frames": [{"id": 0, "samples": 6, "squared_error": 0},
                           {"id": 1, "samples": 6, "squared_error": 390150}],
            }))
        );
    }

    #[test]
    fn wrong_extents_and_geometry_never_produce_error_facts() {
        assert!(measure(&mut [0; 5].as_slice(), &mut [0; 6].as_slice(), 2, 2, 1).is_err());
        assert!(measure(&mut [0; 6].as_slice(), &mut [0; 7].as_slice(), 2, 2, 1).is_err());
        assert!(measure(&mut [].as_slice(), &mut [].as_slice(), 3, 2, 1).is_err());
    }

    #[test]
    fn chroma_errors_are_pooled_with_all_four_luma_samples() {
        assert_eq!(
            measure(
                &mut [0; 6].as_slice(),
                &mut [0, 0, 0, 0, 1, 2].as_slice(),
                2,
                2,
                1
            ),
            Ok(
                json!({"schema": "coreav.videotoolbox-error/1", "width": 2, "height": 2,
                "frames": [{"id": 0, "samples": 6, "squared_error": 5}]})
            )
        );
    }
}

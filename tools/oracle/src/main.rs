//! Command-line interface for the CoreAV oracle.

use std::fs;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use coreav_oracle::compare::{self, Level};
use coreav_oracle::normalize;
use coreav_oracle::perturb;
use serde_json::Value;

const USAGE: &str = "\
coreav-oracle normalize streams --file NAME --exit-code N [--input PATH]
coreav-oracle normalize packets --file NAME --exit-code N [--input PATH]
coreav-oracle normalize frames --file NAME --exit-code N --stream-map I1,I2,... [--input PATH]
coreav-oracle compare --reference DIR --candidate DIR [--levels streams,packets,frames]
    [--profile fixtures|corpus] [--report PATH] [--max-diffs N]
coreav-oracle selfcheck --reference DIR [--candidate DIR]
";

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.is_empty()
        || args
            .iter()
            .any(|argument| argument == "--help" || argument == "-h")
    {
        print!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    let command = args.remove(0);
    let result = match command.as_str() {
        "normalize" => normalize_command(args),
        "compare" => compare_command(args),
        "selfcheck" => selfcheck_command(args),
        _ => Err(format!("unknown command `{command}`\n{USAGE}")),
    };
    match result {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            eprintln!("coreav-oracle: {error}");
            ExitCode::from(2)
        }
    }
}

fn normalize_command(mut args: Vec<String>) -> Result<u8, String> {
    let level_name = pop_required(&mut args, "normalization level")?;
    let level = parse_level(&level_name)?;
    let options = options(args)?;
    ensure_options(
        &options,
        &["--file", "--exit-code", "--input", "--stream-map"],
    )?;
    if level != Level::Frames && options.contains_key("--stream-map") {
        return Err("--stream-map is valid only for frames normalization".to_owned());
    }
    let file = required_option(&options, "--file")?;
    let exit_code = required_option(&options, "--exit-code")?
        .parse::<i32>()
        .map_err(|_| "--exit-code must be an integer".to_owned())?;
    let input = read_input(options.get("--input"))?;
    let normalized = match level {
        Level::Streams => normalize::streams(file, exit_code, &input),
        Level::Packets => normalize::packets(file, exit_code, &input),
        Level::Frames => {
            let stream_map = required_option(&options, "--stream-map")?;
            let stream_map = parse_stream_map(stream_map)?;
            normalize::frames(file, exit_code, &stream_map, &input)
        }
    };
    write_json(&normalized)?;
    Ok(0)
}

fn compare_command(args: Vec<String>) -> Result<u8, String> {
    let options = options(args)?;
    ensure_options(
        &options,
        &[
            "--reference",
            "--candidate",
            "--levels",
            "--profile",
            "--report",
            "--max-diffs",
        ],
    )?;
    let reference = PathBuf::from(required_option(&options, "--reference")?);
    let candidate = PathBuf::from(required_option(&options, "--candidate")?);
    let levels = options
        .get("--levels")
        .map(|value| parse_levels(value))
        .transpose()?
        .unwrap_or_else(all_levels);
    let profile = options
        .get("--profile")
        .map(String::as_str)
        .unwrap_or("fixtures");
    let max_diffs = options
        .get("--max-diffs")
        .map(|value| {
            value
                .parse::<usize>()
                .map_err(|_| "--max-diffs must be a non-negative integer".to_owned())
        })
        .transpose()?
        .unwrap_or(20);
    let results = compare::compare_directories(&reference, &candidate, &levels, profile)?;
    compare::print_summary(&results, max_diffs);
    if let Some(report_path) = options.get("--report") {
        let report = compare::report_json(&results, profile);
        let contents = serde_json::to_vec_pretty(&report)
            .map_err(|error| format!("unable to encode report: {error}"))?;
        fs::write(report_path, contents)
            .map_err(|error| format!("unable to write report {report_path}: {error}"))?;
    }
    Ok(if results.iter().all(|result| result.passed) {
        0
    } else {
        1
    })
}

fn selfcheck_command(args: Vec<String>) -> Result<u8, String> {
    let options = options(args)?;
    ensure_options(&options, &["--reference", "--candidate"])?;
    let reference = PathBuf::from(required_option(&options, "--reference")?);
    let candidate = options.get("--candidate").map(PathBuf::from);
    let passed = perturb::run_selfcheck(&reference, candidate.as_deref())?;
    Ok(if passed { 0 } else { 1 })
}

fn options(args: Vec<String>) -> Result<std::collections::BTreeMap<String, String>, String> {
    let mut parsed = std::collections::BTreeMap::new();
    let mut args = args.into_iter();
    while let Some(key) = args.next() {
        if !key.starts_with("--") {
            return Err(format!("unexpected argument `{key}`"));
        }
        let value = args
            .next()
            .ok_or_else(|| format!("missing value for `{key}`"))?;
        if parsed.insert(key.clone(), value).is_some() {
            return Err(format!("option `{key}` was specified more than once"));
        }
    }
    Ok(parsed)
}

fn ensure_options(
    options: &std::collections::BTreeMap<String, String>,
    allowed: &[&str],
) -> Result<(), String> {
    if let Some(option) = options
        .keys()
        .find(|option| !allowed.contains(&option.as_str()))
    {
        Err(format!("unknown option `{option}`"))
    } else {
        Ok(())
    }
}

fn required_option<'a>(
    options: &'a std::collections::BTreeMap<String, String>,
    name: &str,
) -> Result<&'a str, String> {
    options
        .get(name)
        .map(String::as_str)
        .ok_or_else(|| format!("missing required option `{name}`\n{USAGE}"))
}

fn pop_required(args: &mut Vec<String>, name: &str) -> Result<String, String> {
    if args.is_empty() {
        Err(format!("missing {name}\n{USAGE}"))
    } else {
        Ok(args.remove(0))
    }
}

fn read_input(path: Option<&String>) -> Result<String, String> {
    if let Some(path) = path {
        fs::read_to_string(path).map_err(|error| format!("unable to read input {path}: {error}"))
    } else {
        let mut input = String::new();
        io::stdin()
            .read_to_string(&mut input)
            .map_err(|error| format!("unable to read stdin: {error}"))?;
        Ok(input)
    }
}

fn write_json(value: &Value) -> Result<(), String> {
    let output =
        serde_json::to_vec(value).map_err(|error| format!("unable to encode JSON: {error}"))?;
    let mut stdout = io::stdout().lock();
    stdout
        .write_all(&output)
        .and_then(|()| stdout.write_all(b"\n"))
        .map_err(|error| format!("unable to write stdout: {error}"))
}

fn parse_level(value: &str) -> Result<Level, String> {
    match value {
        "streams" => Ok(Level::Streams),
        "packets" => Ok(Level::Packets),
        "frames" => Ok(Level::Frames),
        _ => Err(format!("unknown level `{value}`\n{USAGE}")),
    }
}

fn parse_levels(value: &str) -> Result<Vec<Level>, String> {
    let mut levels = Vec::new();
    for name in value.split(',') {
        let level = parse_level(name)?;
        if !levels.contains(&level) {
            levels.push(level);
        }
    }
    if levels.is_empty() {
        return Err("--levels must list one or more levels".to_owned());
    }
    Ok(levels)
}

fn all_levels() -> Vec<Level> {
    vec![Level::Streams, Level::Packets, Level::Frames]
}

fn parse_stream_map(value: &str) -> Result<Vec<i64>, String> {
    if value.is_empty() {
        return Ok(Vec::new());
    }
    value
        .split(',')
        .map(|index| {
            index
                .parse::<i64>()
                .map_err(|_| format!("invalid stream-map input index `{index}`"))
        })
        .collect()
}

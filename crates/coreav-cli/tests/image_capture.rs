//! Test-only pre-encode image capture with the CLI's sandboxed worker entry.

use std::path::Path;
use std::process::ExitCode;

fn capture() -> Result<(), String> {
    let request_path =
        std::env::var_os("COREAV_IMAGE_REQUEST").ok_or("image capture request missing")?;
    let metadata =
        std::fs::metadata(&request_path).map_err(|_| "image capture request metadata")?;
    if !metadata.is_file() || metadata.len() > 1 << 20 {
        return Err("image capture request size".to_owned());
    }
    let request: serde_json::Value = serde_json::from_slice(
        &std::fs::read(request_path).map_err(|_| "image capture request read")?,
    )
    .map_err(|_| "image capture request JSON")?;
    let args: Vec<String> = request
        .get("args")
        .and_then(serde_json::Value::as_array)
        .ok_or("image capture arguments")?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_owned)
                .ok_or("image capture argument type")
        })
        .collect::<Result<_, _>>()?;
    let directory = request
        .get("capture")
        .and_then(serde_json::Value::as_str)
        .ok_or("image capture directory")?;
    coreav_cli::ffmpeg::run_images_captured(
        &args,
        coreav_cli::strict_safe::StrictSafe::new(false),
        Path::new(directory),
    )
    .map_err(|error| match error {
        coreav_cli::ffmpeg::Stop::Refused(_) => "image capture command refused".to_owned(),
        coreav_cli::ffmpeg::Stop::Failed(failure) => {
            format!("image capture failed: {}", failure.to_json())
        }
    })?;
    Ok(())
}

fn main() -> ExitCode {
    if let Some(code) = coreav_cli::thumbnail::run_worker() {
        return code;
    }
    if std::env::var_os("COREAV_IMAGE_REQUEST").is_none() {
        return ExitCode::SUCCESS;
    }
    match capture() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

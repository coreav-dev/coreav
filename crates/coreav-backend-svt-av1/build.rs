//! Builds the CoreAV C bridge against one verified native recipe installation.

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

fn command(program: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|error| format!("cannot run {program}: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "{program} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    String::from_utf8(output.stdout).map_err(|_| format!("{program} returned non-UTF-8 output"))
}

fn text(path: &Path) -> Result<&str, String> {
    path.to_str()
        .ok_or_else(|| "native paths must be UTF-8".into())
}

fn run() -> Result<(), String> {
    println!("cargo::rerun-if-env-changed=SVT_AV1_PREFIX");
    println!("cargo::rerun-if-changed=src/native.c");
    if env::var_os("CARGO_FEATURE_NATIVE").is_none() {
        return Ok(());
    }
    let target = env::var("TARGET").map_err(|_| "missing Cargo target")?;
    let host = env::var("HOST").map_err(|_| "missing Cargo host")?;
    let linux = match target.as_str() {
        "x86_64-unknown-linux-musl" | "aarch64-unknown-linux-musl" => true,
        "x86_64-apple-darwin" | "aarch64-apple-darwin" => false,
        _ => {
            return Err(
                "the native SVT-AV1 feature supports only the four Unix release targets".into(),
            );
        }
    };
    if host.split('-').next() != target.split('-').next()
        || host.contains("linux") != linux
        || (!linux && !host.ends_with("apple-darwin"))
    {
        return Err("SVT-AV1 native compilation requires the target's native host".into());
    }
    let prefix = PathBuf::from(
        env::var_os("SVT_AV1_PREFIX")
            .ok_or("set SVT_AV1_PREFIX to the verified pinned recipe installation")?,
    );
    let manifest_dir =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").ok_or("missing manifest directory")?);
    let root = manifest_dir
        .parent()
        .and_then(Path::parent)
        .ok_or("missing repository root")?;
    let check = root.join("scripts/svt_av1_licenses.py");
    command(
        "python3",
        &[
            "-I",
            text(&check)?,
            "--prefix",
            text(&prefix)?,
            "--target",
            &target,
        ],
    )?;
    for path in [
        root.join("scripts/encoders/svt-av1/build.py"),
        root.join("scripts/svt_av1_licenses.py"),
        root.join("scripts/svt-av1-notices.json"),
        prefix.join("manifest.json"),
    ] {
        println!("cargo::rerun-if-changed={}", path.display());
    }
    for directory in [
        prefix.join("include/svt-av1"),
        prefix.join("lib"),
        prefix.join("share/licenses/svt-av1"),
    ] {
        for entry in fs::read_dir(directory).map_err(|error| error.to_string())? {
            println!(
                "cargo::rerun-if-changed={}",
                entry.map_err(|error| error.to_string())?.path().display()
            );
        }
    }
    let compiler = if linux { "musl-gcc" } else { "clang" };
    let linker = env::var("RUSTC_LINKER")
        .map_err(|_| "configure the native target's C compiler as the Rust linker")?;
    if Path::new(&linker)
        .file_name()
        .and_then(|value| value.to_str())
        != Some(compiler)
    {
        return Err(format!(
            "configure {compiler} as the Rust linker for the SVT-AV1 native feature"
        ));
    }
    let native_manifest = prefix.join("manifest.json");
    command(
        "python3",
        &[
            "-I",
            "-c",
            "import json,subprocess,sys; m=json.load(open(sys.argv[1])); assert subprocess.check_output([sys.argv[2],'--version'],text=True).strip()==m['compiler_version'], 'use the native archive compiler'",
            text(&native_manifest)?,
            compiler,
        ],
    )?;
    let out = PathBuf::from(env::var_os("OUT_DIR").ok_or("missing Cargo output directory")?);
    let source = manifest_dir.join("src/native.c");
    let object = out.join("coreav_svt.o");
    let library = out.join("libcoreav_svt_bridge.a");
    let include = prefix.join("include/svt-av1");
    let mut args = vec![
        "-std=c11",
        "-O2",
        "-fPIC",
        "-fno-lto",
        "-Wall",
        "-Wextra",
        "-Werror",
        "-I",
        text(&include)?,
        "-c",
        text(&source)?,
        "-o",
        text(&object)?,
    ];
    if !linux {
        args.push(if target.starts_with("x86_64-") {
            "-mmacosx-version-min=10.12"
        } else {
            "-mmacosx-version-min=11.0"
        });
    }
    command(compiler, &args)?;
    command(
        if linux { "gcc-ar" } else { "ar" },
        &["crs", text(&library)?, text(&object)?],
    )?;
    if linux {
        let rustc = env::var("RUSTC").map_err(|_| "missing Rust compiler")?;
        let directory = command(&rustc, &["--print", "target-libdir", "--target", &target])?;
        let unwind = Path::new(directory.trim()).join("self-contained/libunwind.a");
        let runtime = out.join("runtime");
        fs::create_dir_all(&runtime).map_err(|error| error.to_string())?;
        fs::copy(unwind, runtime.join("libunwind.a"))
            .map_err(|error| format!("install the Rust target's bundled libunwind: {error}"))?;
        println!("cargo::rustc-link-search=native={}", runtime.display());
    }
    println!("cargo::rustc-link-search=native={}", out.display());
    println!(
        "cargo::rustc-link-search=native={}",
        prefix.join("lib").display()
    );
    println!("cargo::rustc-link-lib=static=coreav_svt_bridge");
    println!("cargo::rustc-link-lib=static=SvtAv1Enc");
    println!("cargo::rustc-link-lib=pthread");
    println!("cargo::rustc-link-lib=m");
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        println!("cargo::error={error}");
        std::process::exit(1);
    }
}

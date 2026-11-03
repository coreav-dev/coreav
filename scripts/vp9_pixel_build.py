#!/usr/bin/env python3
"""Build media-free diagnostic artifacts only from the verified implementation merge."""
import argparse
import hashlib
import os
import pathlib
import platform
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import tomllib
import urllib.request

HERE = pathlib.Path(__file__).resolve().parent
ROOT = HERE.parent
sys.path.insert(0, str(HERE))
import oracle_pin
import vp9_pixel_ceiling as ceiling
import vp9_pixel_round as run

RECIPE = pathlib.Path('docs/measurements/vp9-pixel-round/build.json')
SCRIPTS = run.CAPTURE_SCRIPTS
ADAPTER = '''
    static HASHES: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let hashed = *HASHES.get_or_init(|| std::env::var_os("COREAV_PIXEL_HASHES").is_some());
    while let Some(frame) = decoder.receive_frame().map_err(|e| e.to_string())? {
        if hashed {
            let picture = frame.to_cpu().map_err(|e| e.to_string())?;
            let mut packed = Vec::new();
            picture.write_packed(&mut packed).map_err(|e| e.to_string())?;
            let digest = Md5::digest(&packed);
            let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
            frames.push(json!({"pts": frame.pts, "width": picture.width(),
                               "height": picture.height(), "md5": hex}));
        } else {
            std::hint::black_box(frame);
            frames.push(Value::Null);
        }
    }
    Ok(())'''


def cargo_environment(recipe):
    for key in os.environ:
        run.require(not (key in ('RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'RUSTC_WRAPPER',
                                'RUSTC_WORKSPACE_WRAPPER') or key.startswith(('CARGO_PROFILE_', 'CARGO_TARGET_'))),
                    'unrecorded Cargo or target override')
    for parent in (ROOT, *ROOT.parents, pathlib.Path.home()):
        for name in ('.cargo/config', '.cargo/config.toml'):
            run.require(not (parent / name).exists(), 'unrecorded hierarchical Cargo configuration')
    cargo_home = pathlib.Path(os.environ.get('CARGO_HOME', pathlib.Path.home() / '.cargo'))
    for name in ('config', 'config.toml'):
        run.require(not (cargo_home / name).exists(), 'unrecorded Cargo-home configuration')
    env = {key: os.environ[key] for key in ('PATH', 'HOME', 'CARGO_HOME', 'RUSTUP_HOME', 'TMPDIR') if key in os.environ}
    return env | {'CARGO_BUILD_JOBS': '2'} | recipe['native_profile_overrides']


def profile_contract(source):
    for parent in (source, *source.parents):
        for name in ('.cargo/config', '.cargo/config.toml'):
            run.require(not (parent / name).exists(), 'snapshot has unrecorded Cargo configuration')
    manifest = tomllib.loads((source / 'Cargo.toml').read_text())
    profile = manifest['profile']['dist']
    release = manifest['profile'].get('release', {})
    run.require(profile.get('inherits') == 'release' and profile.get('lto') == 'fat'
                and profile.get('codegen-units') == 1 and profile.get('opt-level', 3) == 3
                and not set(profile) - {'inherits', 'lto', 'codegen-units', 'opt-level', 'debug', 'strip'}
                and not set(release) - {'opt-level', 'debug', 'strip'} and release.get('opt-level', 3) == 3,
                'snapshot optimization profile differs')


def command(argv, **kwargs):
    return subprocess.run([str(x) for x in argv], check=True, timeout=1800, **kwargs)


def native_snapshot(work, arm, revision, registration, env, check_only=False):
    source = work / arm
    source.mkdir()
    archive = work / (arm + '.tar')
    command(['git', 'archive', '--format=tar', '-o', archive, revision], cwd=ROOT, env=env)
    with tarfile.open(archive) as bundle:
        bundle.extractall(source, filter='data')
    profile_contract(source)
    adapter = source / 'tools/decode-oracle/src/main.rs'
    adapter.write_text(ceiling.replace_body(adapter.read_text(), 'drain', ADAPTER))
    features = ['coreav-decode-oracle/native-vp9']
    if arm == 'merged-safe-dist':
        features.append(registration['native_safe_feature'])
    invocation = ['cargo', '+' + registration['rust_toolchain'], 'check' if check_only else 'build',
                  '--locked', '--profile', 'dist', '-p', 'coreav-decode-oracle', '-p', 'coreav-kernels-vp9',
                  '--features', ','.join(features)]
    record = dict(commit=revision, archive_sha256=run.digest(archive),
                  lock_sha256=run.digest(source / 'Cargo.lock'),
                  adapter_sha256=hashlib.sha256(ADAPTER.encode()).hexdigest(),
                  adapter_file_sha256=run.digest(adapter), command=invocation, features=features, profile='dist')
    command(invocation, cwd=source, env=env)
    return source, record


def build(args):
    run.require(sys.platform == 'linux' and platform.machine() == 'x86_64', 'build requires hosted Linux x86-64')
    registration = run.plan(ROOT)
    receipt = run.merge_receipt(registration, args.candidate)
    head = subprocess.run(['git', 'rev-parse', 'HEAD'], cwd=ROOT, check=True,
                          capture_output=True, timeout=60).stdout.decode().strip()
    run.require(head == args.candidate, 'tooling checkout differs from the merge commit')
    command(['git', 'diff', '--quiet', 'HEAD', '--'], cwd=ROOT)
    recipe = run.read(ROOT / RECIPE)
    run.require(re.fullmatch(r'[0-9a-f]{40}', recipe['reviewed_plan_commit']), 'invalid reviewed plan commit')
    command(['git', 'fetch', '--no-tags', 'origin', recipe['reviewed_plan_commit']], cwd=ROOT)
    reviewed = subprocess.run(['git', 'show', recipe['reviewed_plan_commit'] + ':' + str(run.PLAN_PATH)],
                              cwd=ROOT, check=True, capture_output=True, timeout=60).stdout
    run.require(reviewed == (ROOT / run.PLAN_PATH).read_bytes()
                and run.digest(ROOT / run.PLAN_PATH) == recipe['reviewed_plan_sha256'],
                'reviewed machine-readable plan changed')
    env = cargo_environment(recipe)
    compiler = subprocess.run(['rustc', '+' + registration['rust_toolchain'], '-Vv'],
                               check=True, capture_output=True, timeout=60, env=env).stdout.decode()
    run.require(compiler.startswith('rustc ' + registration['rust_toolchain'] + ' '), 'compiler differs')
    out = pathlib.Path(args.out).resolve()
    run.require(not out.exists(), 'build destination already exists')
    (out / 'bin').mkdir(parents=True)
    (out / 'scripts').mkdir()
    (out / run.PLAN_PATH).parent.mkdir(parents=True)
    shutil.copy2(ROOT / run.PLAN_PATH, out / run.PLAN_PATH)
    shutil.copy2(ROOT / RECIPE, out / RECIPE)
    for name in SCRIPTS:
        shutil.copy2(HERE / name, out / 'scripts' / name)
    provenance = dict(candidate=args.candidate, baseline=registration['baseline_commit'], merge_receipt=receipt,
                      reviewed_plan_commit=recipe['reviewed_plan_commit'], plan_sha256=run.digest(ROOT / run.PLAN_PATH),
                      recipe_sha256=run.digest(ROOT / RECIPE),
                      compiler=compiler, profile_overrides=recipe['native_profile_overrides'], native={},
                      scripts={name: run.digest(out / 'scripts' / name) for name in SCRIPTS})
    with tempfile.TemporaryDirectory(prefix='vp9-pixel-native-') as temporary:
        work = pathlib.Path(temporary)
        for arm in registration['profiles']['native_arms']:
            revision = registration['baseline_commit'] if arm == 'baseline-dist' else args.candidate
            source, provenance['native'][arm] = native_snapshot(work, arm, revision, registration, env)
            shutil.copy2(source / 'target/dist/decode-oracle', out / 'bin' / arm)
    with tempfile.TemporaryDirectory(prefix='vp9-pixel-oracle-') as temporary:
        oracle = pathlib.Path(temporary)
        command([HERE / 'install-oracle-ffmpeg.sh', oracle], env=env)
        run.require(not list(oracle_pin.verify_bin(oracle, ROOT)), 'pinned oracle verification failed')
        shutil.copy2(oracle / 'ffmpeg', out / 'bin/ffmpeg-pinned')
        shutil.copy2(oracle / 'ffprobe', out / 'bin/ffprobe')
    pin = oracle_pin.load(ROOT)
    revision = pin['BUILD'].rsplit('-g', 1)[-1]
    run.require(recipe['ffmpeg_source_commit'].startswith(revision), 'source revision differs from oracle pin')
    with tempfile.TemporaryDirectory(prefix='vp9-pixel-symbols-') as temporary:
        work = pathlib.Path(temporary)
        archive = work / 'source.tar.gz'
        with urllib.request.urlopen(f'https://codeload.github.com/FFmpeg/FFmpeg/tar.gz/{revision}', timeout=180) as response:
            with archive.open('wb') as stream:
                shutil.copyfileobj(response, stream)
        run.require(run.digest(archive) == recipe['ffmpeg_source_archive_sha256'], 'source archive differs')
        with tarfile.open(archive) as bundle:
            run.require(bundle.pax_headers.get('comment') == recipe['ffmpeg_source_commit'], 'full source commit differs')
        source = work / 'source'
        source.mkdir()
        log = work / 'build.log'
        try:
            with log.open('wb') as output:
                command(['tar', '-xzf', archive, '-C', source, '--strip-components=1'], stdout=output, stderr=output)
                configure = ['./configure', '--cc=cc', '--disable-everything', '--disable-autodetect', '--enable-decoder=vp9',
                             '--enable-parser=vp9', '--enable-demuxer=matroska', '--enable-muxer=null,framemd5',
                             '--enable-encoder=rawvideo,wrapped_avframe', '--enable-protocol=file,pipe',
                             '--enable-filter=null,format,scale', '--disable-stripping', '--disable-debug', '--disable-doc']
                command(configure, cwd=source, env=env, stdout=output, stderr=output)
                command(['make', '-j2'], cwd=source, env=env, stdout=output, stderr=output)
            shutil.copy2(source / 'ffmpeg_g', out / 'bin/ffmpeg-symbols')
        finally:
            shutil.rmtree(source)
            archive.unlink(missing_ok=True)
            log.unlink(missing_ok=True)
    sections = subprocess.run(['readelf', '-SW', out / 'bin/ffmpeg-symbols'], capture_output=True,
                               check=True, timeout=60, env=env).stdout
    run.require(b'.debug_' not in sections, 'symbol build contains debug sections')
    provenance.update(source_deleted_before_profiling=True, oracle_pin_sha256=run.digest(HERE / 'oracle-ffmpeg.env'),
                      symbol_source=dict(commit=recipe['ffmpeg_source_commit'],
                                         archive_sha256=recipe['ffmpeg_source_archive_sha256'],
                                         compiler=subprocess.run(['cc', '--version'], capture_output=True, check=True,
                                             timeout=60, env=env).stdout.decode().splitlines()[0],
                                         assembler=subprocess.run(['nasm', '-v'], capture_output=True, check=True,
                                             timeout=60, env=env).stdout.decode().strip(),
                                         configure=configure, debug_sections=False),
                      binaries={name: run.digest(out / 'bin' / name) for name in registration['arms'] + ['ffprobe']})
    run.write(out / 'builds.json', provenance)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--candidate', required=True)
    parser.add_argument('--out', required=True)
    args = parser.parse_args()
    try:
        build(args)
    except Exception as error:
        print(f'vp9-pixel-build incomplete ({type(error).__name__})', file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    sys.exit(main())

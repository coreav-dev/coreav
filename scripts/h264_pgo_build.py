#!/usr/bin/env python3
"""Build opt-in Linux x86-64 H.264 PGO artifacts without changing default builds."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
from h264_pgo_round import digest, execute, require, write

TOOLCHAIN = '1.99.1'
TARGET = 'x86_64-unknown-linux-gnu'
RUST_COMMIT = '58cf8f9c35b355199727627d07ae34a4e2846d67'
LLVM = '23.1.1'
HISTORICAL_SOURCE = '1821f1f005ff78f294a1b529bde5efe145b21e5c'
HISTORICAL_BINARY = 'e1a3ce41cf39fc77fbdded85f55eb627169065b8a8be3ae38ff1472e584ba3fa'
HISTORICAL_PROFILE = '0d12fc7b98c5f94e3b99122b5bc3797cc0ba0d2b8ae983a54041207ca723fb63'


def toolchain():
    require(platform.system() == 'Linux' and platform.machine() == 'x86_64', 'recipe requires Linux x86-64')
    for key in ('RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'LLVM_PROFILE_FILE', 'RUSTC_WRAPPER',
                'RUSTC_WORKSPACE_WRAPPER', 'CARGO_BUILD_TARGET', 'CARGO_TARGET_DIR', 'RUSTC', 'RUSTDOC'):
        require(not os.environ.get(key), f'unset {key} before using the recipe')
    require(not any(k.startswith('CARGO_TARGET_') and v for k, v in os.environ.items()),
            'unset target-specific Cargo overrides')
    home = Path(os.environ.get('CARGO_HOME', str(Path.home() / '.cargo')))
    require(not (home / 'config').exists() and not (home / 'config.toml').exists(), 'use a Cargo home without configuration overrides')
    version = subprocess.check_output(['rustc', '+' + TOOLCHAIN, '-vV'], text=True, timeout=30)
    require(f'commit-hash: {RUST_COMMIT}' in version and f'LLVM version: {LLVM}' in version and
            f'host: {TARGET}' in version, 'wrong compiler identity')
    sysroot = Path(subprocess.check_output(['rustc', '+' + TOOLCHAIN, '--print', 'sysroot'], text=True, timeout=30).strip())
    profdata = sysroot / 'lib/rustlib' / TARGET / 'bin/llvm-profdata'
    profile_version = subprocess.check_output([str(profdata), '--version'], text=True, timeout=30)
    require(LLVM in profile_version, 'install matching llvm-tools-preview')
    linker = shutil.which('cc')
    require(linker is not None, 'C linker driver is missing')
    return profdata, {'rustc': version, 'profdata': profile_version, 'profdata_sha256': digest(profdata),
                      'rustc_sha256': digest(sysroot / 'bin/rustc'), 'cargo_sha256': digest(sysroot / 'bin/cargo'),
                      'linker_sha256': digest(Path(linker).resolve()),
                      'linker_version': subprocess.check_output([linker, '--version'], text=True, timeout=30)}


def source_identity(source):
    require(not subprocess.check_output(['git', 'status', '--porcelain'], cwd=source, timeout=30), 'source checkout must be clean')
    def git(*args):
        return subprocess.check_output(['git', *args], cwd=source, text=True, timeout=30).strip()
    require(not any((p / '.cargo' / n).exists() for p in (source, *source.parents) for n in ('config', 'config.toml')),
            'repository Cargo overrides are not part of this recipe')
    return {'commit': git('rev-parse', 'HEAD'), 'tree': git('rev-parse', 'HEAD^{tree}'),
            'lock_sha256': digest(source / 'Cargo.lock')}


def compile_binary(source, destination, flags, historical=False, artifact='decode'):
    require(artifact in ('decode', 'cli') and not (historical and artifact != 'decode'), 'unsupported build artifact')
    destination.mkdir(parents=True, exist_ok=False)
    target = destination / 'target'
    command = ['cargo', '+' + TOOLCHAIN, 'build', '--locked', '--profile', 'dist']
    if artifact == 'decode':
        command += ['-p', 'coreav-h264', '--example', 'decode']
        suffix = 'dist/examples/decode'
        name = 'decode'
    else:
        command += ['-p', 'coreav-cli', '--bin', 'coreav', '--features', 'native-h264']
        suffix = 'dist/coreav'
        name = 'coreav'
    if not historical:
        command += ['--target', TARGET]
        flags = ['--remap-path-prefix=' + str(source) + '=/coreav-source', *flags]
    env = os.environ | {'CARGO_TARGET_DIR': str(target), 'CARGO_ENCODED_RUSTFLAGS': '\x1f'.join(flags),
                        'LLVM_PROFILE_FILE': str(destination / 'build-only-%m-%p.profraw')}
    with (destination / 'build.log').open('w') as log:
        subprocess.run(command, cwd=source, env=env, stdout=log, stderr=subprocess.STDOUT, check=True, timeout=900)
    binary = target / ('' if historical else TARGET) / suffix
    shutil.copy2(binary, destination / name)
    return {'command': command, 'flags': flags, 'sha256': digest(destination / name),
            'bytes': (destination / name).stat().st_size}


def training_roster(source):
    listing = json.loads((source / 'docs/measurements/h264-training/es.json').read_text())
    selected = []
    for cls in ('sd', '720p', '1080p', '4k'):
        ordered = sorted((name for name, row in listing.items() if row['class'] == cls), key=lambda name:
                         hashlib.sha256(('coreav-h264-pgo-2026-11-02\0' + listing[name]['es_sha256']).encode()).hexdigest())
        require(len(ordered) == 8, 'training manifest changed')
        selected.extend(ordered[:4])
    return {name: listing[name] for name in selected}


def collect(source, out, media, profdata, tools):
    require(not out.exists(), 'use a new output directory')
    out.mkdir(parents=True)
    identity = source_identity(source)
    rows = training_roster(source)
    require(all(digest(media / n) == r['es_sha256'] for n, r in rows.items()), 'training bytes changed')
    base = compile_binary(source, out / 'base', [])
    generate = compile_binary(source, out / 'instrumented', ['-Cprofile-generate=' + str(out / 'build-profiles')])
    records, profiles = [], []
    for name, row in rows.items():
        for threads in (1, 8):
            args = ['--threads', str(threads), '--frames', str(row['frames']), str(media / name)]
            reference = execute([str(out / 'base/decode'), *args], maximum=4 << 30)
            check_dir = out / 'checks' / name / str(threads)
            check_dir.mkdir(parents=True)
            previous = os.environ.get('LLVM_PROFILE_FILE')
            os.environ['LLVM_PROFILE_FILE'] = str(check_dir / '%m-%p.profraw')
            try:
                checked = execute([str(out / 'instrumented/decode'), *args], maximum=4 << 30)
            finally:
                if previous is None:
                    os.environ.pop('LLVM_PROFILE_FILE')
                else:
                    os.environ['LLVM_PROFILE_FILE'] = previous
            require((checked['bytes'], checked['sha256']) == (reference['bytes'], reference['sha256']) and reference['bytes'] > 0,
                    'instrumented output differs')
            directory = out / 'profiles' / name / str(threads)
            directory.mkdir(parents=True)
            with (directory / 'decode.log').open('w') as log:
                subprocess.run([str(out / 'instrumented/decode'), '--discard', *args], check=True, stdout=log, stderr=log,
                               env=os.environ | {'LLVM_PROFILE_FILE': str(directory / '%m-%p.profraw')}, timeout=180)
            require(not (directory / 'decode.log').stat().st_size, 'unexpected training diagnostic')
            raw = sorted(directory.glob('*.profraw'))
            require(len(raw) == 1, 'each decoder invocation must produce exactly one profile')
            profiles.extend(raw)
            records.append({'input': name, 'input_sha256': row['es_sha256'], 'threads': threads,
                            'bytes': reference['bytes'], 'output_sha256': reference['sha256'],
                            'profile': str(raw[0].relative_to(out)), 'profile_sha256': digest(raw[0])})
            write(out / 'training-progress.json', records)
    merged = out / 'profile.profdata'
    subprocess.run([str(profdata), 'merge', '-o', str(merged), *map(str, profiles)], check=True, timeout=120)
    shown = subprocess.check_output([str(profdata), 'show', '--all-functions', str(merged)], text=True, timeout=120)
    require('build_script_build' not in shown, 'profile contains host build-script functions')
    optimized = compile_binary(source, out / 'optimized', ['-Cprofile-use=' + str(merged), '-Cllvm-args=-pgo-warn-missing-function'])
    independent = compile_binary(source, out / 'independent', ['-Cprofile-use=' + str(merged), '-Cllvm-args=-pgo-warn-missing-function'])
    require(optimized['sha256'] == independent['sha256'], 'independent rebuild differs')
    for name, row in rows.items():
        for threads in (1, 8):
            value = execute([str(out / 'optimized/decode'), '--threads', str(threads), '--frames', str(row['frames']), str(media / name)], maximum=4 << 30)
            reference = next(r for r in records if r['input'] == name and r['threads'] == threads)
            require((value['bytes'], value['sha256']) == (reference['bytes'], reference['output_sha256']), 'PGO output differs')
    cli_base = compile_binary(source, out / 'cli-base', [], artifact='cli')
    cli_pgo = compile_binary(source, out / 'cli-optimized', ['-Cprofile-use=' + str(merged),
                              '-Cllvm-args=-pgo-warn-missing-function'], artifact='cli')
    cli_independent = compile_binary(source, out / 'cli-independent', ['-Cprofile-use=' + str(merged),
                                      '-Cllvm-args=-pgo-warn-missing-function'], artifact='cli')
    require(cli_pgo['sha256'] == cli_independent['sha256'], 'independent diagnostic CLI rebuild differs')
    write(out / 'build.json', {'schema': 1, 'source': identity, 'tools': tools, 'training': records,
          'profile_sha256': digest(merged), 'base': base, 'generate': generate,
          'pgo': optimized, 'independent': independent, 'independent_match': True,
          'recipe_sha256': digest(Path(__file__)), 'cli': {'features': ['native-h264'], 'base': cli_base,
          'pgo': cli_pgo, 'independent': cli_independent, 'independent_match': True}})


def reproduce(source, out, profile, tools):
    require(source_identity(source)['commit'] == HISTORICAL_SOURCE, 'historical source must match measured commit')
    require(profile.is_file(), 'retained merged profile is required')
    require(digest(profile) == HISTORICAL_PROFILE, 'historical merged profile changed')
    out.mkdir(parents=True, exist_ok=False)
    artifact = compile_binary(source, out / 'reproduction', ['-Cprofile-use=' + str(profile), '-Clink-arg=-Wl,--emit-relocs'], historical=True)
    write(out / 'reproduction.json', {'source': source_identity(source), 'tools': tools,
          'profile_sha256': digest(profile), 'binary': artifact, 'expected_sha256': HISTORICAL_BINARY,
          'match': artifact['sha256'] == HISTORICAL_BINARY})
    require(artifact['sha256'] == HISTORICAL_BINARY, 'measured binary was not reproduced; retain linker and path evidence')


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument('action', choices=('collect', 'reproduce'))
    ap.add_argument('--source', type=Path, required=True)
    ap.add_argument('--out', type=Path, required=True)
    ap.add_argument('--media', type=Path)
    ap.add_argument('--profile', type=Path)
    args = ap.parse_args()
    source, out = args.source.resolve(), args.out.resolve()
    try:
        profdata, tools = toolchain()
        if args.action == 'collect':
            require(args.media is not None, 'collect needs --media')
            collect(source, out, args.media.resolve(), profdata, tools)
        else:
            require(args.profile is not None, 'reproduce needs --profile')
            reproduce(source, out, args.profile.resolve(), tools)
        return 0
    except (ValueError, OSError, subprocess.SubprocessError) as error:
        print(str(error), file=sys.stderr)
        return 2


if __name__ == '__main__':
    sys.exit(main())

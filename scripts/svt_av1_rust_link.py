"""Verify a rooted Rust link against a fresh native SVT-AV1 installation.

The installation and output paths must have no concurrent writer.
"""

import argparse
import json
import os
from pathlib import Path
import platform
import re
import runpy
import shutil
import sys
import tempfile


ROOT = Path(__file__).resolve().parents[1]
API_ROOTS = (
    'svt_av1_enc_init_handle', 'svt_av1_enc_set_parameter', 'svt_av1_enc_init',
    'svt_av1_enc_send_picture', 'svt_av1_enc_get_packet', 'svt_av1_enc_release_out_buffer',
    'svt_av1_enc_deinit', 'svt_av1_enc_deinit_handle', 'svt_av1_get_version',
)
MESSAGE = 'CoreAV SVT-AV1 Rust link probe'


class LinkError(ValueError):
    pass


def rust_pin():
    text = (ROOT / 'scripts/rust-dist.env').read_text(encoding='utf-8')
    versions = re.findall(r'^RUST_DIST_TOOLCHAIN=([0-9]+\.[0-9]+\.[0-9]+)$', text, re.MULTILINE)
    commits = re.findall(r'^RUST_DIST_COMMIT=([a-f0-9]{40})$', text, re.MULTILINE)
    if len(versions) != 1 or len(commits) != 1:
        raise LinkError('read one Rust version and commit from scripts/rust-dist.env')
    return versions[0], commits[0]


def check(prefix, target, output, runner=None):
    recipe = runpy.run_path(str(ROOT / 'scripts/encoders/svt-av1/build.py'))
    licenses = runpy.run_path(str(ROOT / 'scripts/svt_av1_licenses.py'))
    identity = licenses['check'](prefix, target)
    native = recipe['TARGETS'][target]
    if (platform.system(), platform.machine()) != (native.system, native.machine):
        raise LinkError('Rust link verification requires the exact native host')
    prefix = prefix.resolve()
    if not output.is_absolute() or os.path.lexists(output):
        raise LinkError('output must be absolute and absent')
    output = output.resolve()
    if not output.parent.is_dir() or prefix in output.parents:
        raise LinkError('output must have an existing parent outside the installation')
    manifest = json.loads((prefix / 'manifest.json').read_text(encoding='utf-8'))
    tools = {}
    required = ['rustup', native.cc, 'nm']
    required += ['readelf'] if native.system == 'Linux' else ['lipo', 'otool']
    for name in required:
        found = shutil.which(name)
        if found is None:
            raise LinkError(f'install the required Rust link tool: {name}')
        tools[name] = str(Path(found).resolve())
    version, commit = rust_pin()
    run = recipe['run'] if runner is None else runner
    environment = {key: os.environ[key] for key in ('PATH', 'HOME', 'RUSTUP_HOME', 'CARGO_HOME')
                   if key in os.environ}
    environment.update(LC_ALL='C', LANG='C', RUSTUP_TOOLCHAIN=version)
    if native.system == 'Darwin':
        environment['MACOSX_DEPLOYMENT_TARGET'] = native.deployment
    with recipe['cancellation'](), tempfile.TemporaryDirectory(
            prefix='.svt-rust-link-', dir=output.parent) as temporary:
        work = Path(temporary)
        scratch = work / 'tmp'
        scratch.mkdir()
        environment['TMPDIR'] = str(scratch)
        rustc = Path(run([tools['rustup'], 'which', '--toolchain', version, 'rustc'],
                         environment, timeout=30).strip())
        if not rustc.is_absolute() or not rustc.is_file():
            raise LinkError('install the pinned Rust compiler before linking')
        details = run([str(rustc), '-vV'], environment, timeout=30)
        if (f'release: {version}\n' not in details
                or f'commit-hash: {commit}\n' not in details):
            raise LinkError('Rust compiler differs from scripts/rust-dist.env')
        compiler = run([tools[native.cc], '--version'], environment, timeout=30).strip()
        if compiler != manifest.get('compiler_version'):
            raise LinkError('final C linker differs from the native archive compiler')
        stage = work / 'result'
        stage.mkdir()
        source = stage / 'probe.rs'
        source.write_text(f'fn main() {{ println!("{MESSAGE}"); }}\n', encoding='utf-8')
        probe = stage / 'svt-av1-rust-link'
        command = [str(rustc), '--edition=2024', '--crate-name=svt_rust_link',
                   '--target', target, '-Copt-level=3', '-Clto=fat', '-Ccodegen-units=1',
                   '-Cpanic=abort', f'-Clinker={tools[native.cc]}',
                   f'-Lnative={prefix / "lib"}', '-lstatic=SvtAv1Enc',
                   f'--remap-path-prefix={work}=/svt-rust-link']
        unwind_sha256 = None
        if native.system == 'Linux':
            target_libdir = Path(run([str(rustc), '--print', 'target-libdir', '--target', target],
                                    environment, timeout=30).strip())
            unwind = target_libdir / 'self-contained/libunwind.a'
            if not target_libdir.is_absolute() or not unwind.is_file() or unwind.is_symlink():
                raise LinkError('install the pinned Rust target with its bundled libunwind.a')
            runtime = work / 'runtime'
            runtime.mkdir()
            shutil.copyfile(unwind, runtime / 'libunwind.a')
            unwind_sha256 = recipe['sha256'](runtime / 'libunwind.a')
            command += ['-Clink-self-contained=no', '-Crelocation-model=static',
                        f'-Lnative={runtime}']
        for name in API_ROOTS:
            symbol = '_' + name if native.system == 'Darwin' else name
            command.append('-Clink-arg=-Wl,-u,' + symbol)
        command += ['-Clink-arg=-pthread', '-Clink-arg=-lm', str(source), '-o', str(probe)]
        run(command, environment, timeout=180)
        symbols = run([tools['nm'], '-a', str(probe)], environment, timeout=30)
        text_symbols = {parts[-1] for line in symbols.splitlines()
                        if len(parts := line.split()) >= 3 and parts[-2] in ('T', 't')}
        expected = {'_' + name if native.system == 'Darwin' else name for name in API_ROOTS}
        if not expected <= text_symbols:
            raise LinkError('Rust probe did not retain every SVT API root as executable code')
        if native.system == 'Linux':
            header = run([tools['readelf'], '-h', str(probe)], environment, timeout=30)
            if not re.search(r'^\s*Machine:\s+' + re.escape(native.elf_machine) + r'\s*$',
                             header, re.MULTILINE):
                raise LinkError('Rust probe has the wrong ELF machine')
            program = run([tools['readelf'], '-l', str(probe)], environment, timeout=30)
            dynamic = run([tools['readelf'], '-d', str(probe)], environment, timeout=30)
            if (not re.search(r'\bLOAD\b', program) or re.search(r'\bINTERP\b', program)
                    or re.search(r'\bNEEDED\b', dynamic)):
                raise LinkError('Rust probe is not a static Linux executable')
        else:
            arches = run([tools['lipo'], '-archs', str(probe)], environment, timeout=30).split()
            if arches != [native.machine]:
                raise LinkError('Rust probe has the wrong or multiple architectures')
            listing = run([tools['otool'], '-L', str(probe)], environment, timeout=30).splitlines()
            if not listing or listing[0] != str(probe) + ':' or len(listing) < 2:
                raise LinkError('Rust probe has no valid system dependency listing')
            for line in listing[1:]:
                dependency = line.strip().split(' (', 1)[0]
                if (not dependency.startswith(('/usr/lib/', '/System/Library/'))
                        or '..' in Path(dependency).parts):
                    raise LinkError('Rust probe loads a non-system library')
        if run([str(probe)], environment, timeout=10) != MESSAGE + '\n':
            raise LinkError('Rust link probe did not complete successfully')
        record = {
            'target': target, 'version': identity['version'],
            'archive_sha256': identity['archive_sha256'],
            'library_sha256': manifest['sha256']['lib/libSvtAv1Enc.a'],
            'rust_version': version, 'rust_commit': commit, 'linker': native.cc,
            'compiler_version': compiler, 'api_roots': list(API_ROOTS),
            'source_sha256': recipe['sha256'](source),
            'probe_sha256': recipe['sha256'](probe),
        }
        if unwind_sha256 is not None:
            record['rust_unwind_sha256'] = unwind_sha256
        (stage / 'manifest.json').write_text(json.dumps(record, indent=2, sort_keys=True) + '\n',
                                             encoding='utf-8')
        recipe['publish'](stage, output)
    return record


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--prefix', type=Path, required=True)
    parser.add_argument('--target', required=True)
    parser.add_argument('--output', type=Path, required=True)
    options = parser.parse_args(argv)
    try:
        record = check(options.prefix, options.target, options.output)
    except (OSError, ValueError, RuntimeError) as error:
        print(f'error: {error}', file=sys.stderr)
        return 1
    print(json.dumps(record, sort_keys=True))
    return 0


if __name__ == '__main__':
    sys.exit(main())

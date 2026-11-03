#!/usr/bin/env python3
"""Run the registered synthetic native cases; independent decoding is a later gate."""

import argparse
from dataclasses import asdict
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import time
import uuid


ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'scripts'))
import svt_av1_cases as cases
import svt_av1_licenses as licenses

SPEC = importlib.util.spec_from_file_location('svt_native_process',
                                            ROOT / 'scripts/encoders/svt-av1/build.py')
PROCESS = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = PROCESS
SPEC.loader.exec_module(PROCESS)


def digest(path):
    result = hashlib.sha256()
    with Path(path).open('rb') as source:
        for block in iter(lambda: source.read(1 << 20), b''):
            result.update(block)
    return result.hexdigest()


def context(prefix, target):
    licenses.check(prefix, target)
    dirty = subprocess.check_output(['git', 'status', '--porcelain'], cwd=ROOT, text=True, timeout=30)
    if dirty:
        raise ValueError('native execution requires a committed clean source tree')
    revision = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True, timeout=30).strip()
    if os.environ.get('GITHUB_SHA', revision) != revision:
        raise ValueError('native source revision differs from the current workflow')
    toolchain = dict(line.split('=', 1) for line in (ROOT / 'scripts/rust-dist.env')
                     .read_text().splitlines() if line and not line.startswith('#'))
    version = toolchain['RUST_DIST_TOOLCHAIN']
    programs = {
        name: Path(subprocess.check_output(['rustup', 'which', '--toolchain', version, name],
                                           text=True, timeout=30).strip()).resolve(strict=True)
        for name in ('cargo', 'rustc')
    }
    rustc = subprocess.check_output([str(programs['rustc']), '-vV'], text=True, timeout=30)
    fields = dict(line.split(': ', 1) for line in rustc.splitlines() if ': ' in line)
    if (fields.get('release') != toolchain['RUST_DIST_TOOLCHAIN']
            or fields.get('commit-hash') != toolchain['RUST_DIST_COMMIT']):
        raise ValueError('native cases require the distribution Rust toolchain')
    return {
        'revision': revision, 'target': target, 'rustc': rustc,
        'toolchain': version,
        **{f'{name}_path': str(path) for name, path in programs.items()},
        **{f'{name}_sha256': digest(path) for name, path in programs.items()},
        'native_prefix': str(prefix), 'native_manifest_sha256': digest(prefix / 'manifest.json'),
        'roster_sha256': digest(cases.ROSTER),
        'workflow_run': os.environ.get('GITHUB_RUN_ID'),
        'workflow_attempt': os.environ.get('GITHUB_RUN_ATTEMPT'),
    }


def _build(prefix, target, destination, runner):
    current = context(prefix, target)
    destination.mkdir(parents=True, exist_ok=False)
    target_directory = destination / f'target-{uuid.uuid4().hex}'
    if os.path.lexists(target_directory):
        raise ValueError('native build requires a newly absent target directory')
    configuration = {
        'SVT_AV1_PREFIX': str(prefix), 'RUSTC': current['rustc_path'],
        'RUSTUP_TOOLCHAIN': current['toolchain'], 'RUSTC_WRAPPER': '',
        'RUSTC_WORKSPACE_WRAPPER': '', 'RUSTFLAGS': '', 'CARGO_ENCODED_RUSTFLAGS': '',
        'CARGO_INCREMENTAL': '0',
        f'CARGO_TARGET_{target.replace("-", "_").upper()}_LINKER':
            'musl-gcc' if target.endswith('linux-musl') else 'clang',
    }
    environment = {key: value for key, value in os.environ.items()
                   if not key.startswith(('CARGO_', 'RUST', 'SVT_AV1_', 'COREAV_SVT_CASE_'))}
    environment.update(configuration)
    command = [current['cargo_path'], 'test', '--manifest-path',
               str(ROOT / 'crates/coreav-backend-svt-av1/Cargo.toml'), '--locked',
               '--no-default-features', '--features', 'native', '--lib', '--no-run', '--jobs', '1',
               '--target', target, '--target-dir', str(target_directory),
               '--message-format=json-render-diagnostics']
    receipt = {'identity': current, 'command': command, 'configuration': configuration,
               'target_directory': str(target_directory), 'complete': False}
    receipt_path = destination / 'build.json'
    process = {}
    try:
        if context(prefix, target) != current:
            raise ValueError('native build identity changed before Cargo')
        runner(command, environment, timeout=1200, record=process)
        if (process.get('exit_code') != 0 or process.get('timed_out') is not False
                or type(process.get('pid')) is not int):
            raise ValueError('native Cargo invocation did not complete successfully')
        if context(prefix, target) != current:
            raise ValueError('native build identity changed during Cargo')
        binary = _executable(process['stdout'], target_directory, target)
        receipt.update(binary=str(binary), binary_sha256=digest(binary), complete=True)
        return binary, receipt
    finally:
        stdout = process.pop('stdout', '')
        (destination / 'cargo-messages.jsonl').write_text(stdout)
        (destination / 'cargo-stderr.log').write_text(process.pop('stderr', ''))
        receipt.update(process=process, messages_sha256=digest(destination / 'cargo-messages.jsonl'))
        receipt_path.write_text(json.dumps(receipt, indent=2) + '\n')


def _executable(messages, target_directory, target):
    artifacts = []
    finished = []
    for line in messages.splitlines():
        record = json.loads(line)
        if record.get('reason') == 'build-finished':
            finished.append(record.get('success'))
        if (record.get('reason') == 'compiler-artifact'
                and record.get('target', {}).get('name') == 'coreav_backend_svt_av1'
                and record.get('profile', {}).get('test') is True
                and record.get('executable')):
            artifacts.append(record)
    if (finished != [True] or len(artifacts) != 1 or artifacts[0].get('fresh') is not False
            or artifacts[0].get('features') != ['native']
            or artifacts[0].get('target', {}).get('kind') != ['lib']
            or artifacts[0].get('target', {}).get('src_path')
                != str(ROOT / 'crates/coreav-backend-svt-av1/src/lib.rs')):
        raise ValueError('build record requires one freshly compiled target-native test executable')
    binary = Path(artifacts[0]['executable'])
    if (not binary.is_file() or binary.is_symlink()
            or binary.parent.resolve() != (target_directory / target / 'debug/deps').resolve()):
        raise ValueError('native executable is outside this invocation target directory')
    return binary.resolve()


def check_layout(case, value):
    if (not isinstance(value, dict) or set(value) != {'B', 'F', 'P', 'S', 'K', 'slot'}
            or any(type(number) is not int or number <= 0 for number in value.values())):
        raise ValueError('invalid native retained layout')
    if (value['B'] != case.retained_bytes or value['S'] != case.capacity
            or value['K'] != case.keyint or value['P'] <= case.picture_bytes + value['slot']
            or value['S'] != min(value['K'], (value['B'] - value['F']) // value['P'])):
        raise ValueError('native retained layout differs from its closed case')


def check_completion(case, value, layout):
    if not isinstance(value, dict) or value.get('outcome') != case.outcome:
        raise ValueError('native case outcome differs from the closed roster')
    if case.outcome in ('empty-input', 'retained-limit'):
        expected = {'outcome': case.outcome, 'open_attempts': 0, 'frames': 0}
        if value != expected or any(type(value[key]) is not int for key in ('open_attempts', 'frames')):
            raise ValueError('refused native case initialized a session or changed its record')
        return
    fields = {'outcome', 'frames', 'peak', 'header_drops', 'plane_drops', 'peak_sessions'}
    if case.outcome == 'complete':
        fields |= {'segments', 'packets', 'open_attempts'}
    if set(value) != fields or any(type(value[key]) is not int or value[key] < 0 for key in fields - {'outcome'}):
        raise ValueError('invalid native completion record')
    submitted = min(case.pictures, case.capacity)
    peak = layout['F'] + case.capacity * layout['slot'] + submitted * (layout['P'] - layout['slot'])
    if value['frames'] != case.pictures or value['peak'] != peak or value['peak_sessions'] != 1:
        raise ValueError('native case count, peak or live-session bound differs from the closed roster')
    if case.outcome == 'retained-until-exit':
        if value['header_drops'] != 0 or value['plane_drops'] != 0:
            raise ValueError('failed destruction released native input ownership')
        return
    segments = (case.pictures + case.capacity - 1) // case.capacity if case.outcome == 'complete' else 1
    if value['header_drops'] != case.pictures + segments or value['plane_drops'] != case.pictures:
        raise ValueError('native input or EOF ownership was not released exactly once')
    if case.outcome == 'complete' and (value['segments'] != segments or value['open_attempts'] != segments
                                      or value['packets'] < segments):
        raise ValueError('native segment execution differs from the closed roster')


def run(prefix, target, destination, runner=PROCESS.run):
    binary, build = _build(prefix, target, destination, runner)
    current = {**build['identity'], 'binary_sha256': build['binary_sha256']}
    roster = cases.load()
    report = {'stage': 'native-wrapper', 'identity': current, 'build': build,
              'complete': False, 'cases': []}
    report_path = destination / 'report.json'
    deadline = time.monotonic() + 1200
    for case in roster:
        directory = destination / case.id
        directory.mkdir()
        result = {'case': asdict(case), 'exit_code': None, 'timed_out': False, 'complete': False}
        report['cases'].append(result)
        try:
            if digest(binary) != current['binary_sha256']:
                raise ValueError('native executable changed between cases')
            raw = directory / 'input.bin'
            with raw.open('wb') as output:
                cases.write_input(output, case)
            result['input_sha256'] = digest(raw)
            environment = {
                **os.environ,
                'COREAV_SVT_CASE_MODE': case.mode, 'COREAV_SVT_CASE_OUTCOME': case.outcome,
                'COREAV_SVT_CASE_FRAMES': str(case.pictures),
                'COREAV_SVT_CASE_CAPACITY': str(case.capacity),
                'COREAV_SVT_CASE_DIRECTORY': str(directory),
            }
            remaining = min(10 if case.mode == 'deadline-control' else 180,
                            deadline - time.monotonic())
            if remaining <= 0:
                raise ValueError('whole native-case deadline exhausted')
            process = {}
            failure = None
            try:
                with raw.open('rb') as source:
                    runner([str(binary), 'native::tests::registered_native_case', '--exact',
                            '--nocapture', '--test-threads=1'], environment,
                           timeout=remaining, stdin=source, record=process)
            except PROCESS.BuildError as error:
                failure = error
            finally:
                (directory / 'stdout.log').write_text(process.pop('stdout', ''))
                (directory / 'stderr.log').write_text(process.pop('stderr', ''))
                result.update(process)
                pid = process.get('pid')
                if pid is not None:
                    try:
                        os.killpg(pid, 0)
                    except ProcessLookupError:
                        result['group_absent'] = True
                    else:
                        result['group_absent'] = False
            if case.mode == 'deadline-control':
                if failure is None or not result['timed_out'] or not result.get('group_absent'):
                    raise ValueError('deadline control did not time out and terminate its group')
                retained = licenses.read_json(directory / 'retained-before-deadline.json')
                if (set(retained) != {'pid', 'frames', 'peak', 'header_drops', 'plane_drops', 'live_sessions'}
                        or any(type(number) is not int or number < 0 for number in retained.values())
                        or retained['pid'] != result['pid'] or retained['frames'] != case.pictures
                        or retained['header_drops'] != 0 or retained['plane_drops'] != 0
                        or retained['live_sessions'] != 1
                        or not case.picture_bytes <= retained['peak'] <= case.retained_bytes):
                    raise ValueError('deadline control lacks retained-input ownership evidence')
                completion = {'outcome': case.outcome, 'frames': case.pictures,
                              'pre_termination': retained}
            else:
                if failure is not None:
                    raise failure
                completion = licenses.read_json(directory / 'complete.json')
            layout = None
            if case.capacity:
                layout = licenses.read_json(directory / 'layout.json')
                check_layout(case, layout)
                result['layout'] = layout
            if case.mode != 'deadline-control':
                check_completion(case, completion, layout)
            else:
                exact = layout['F'] + case.capacity * layout['slot'] + case.pictures * (layout['P'] - layout['slot'])
                if completion['pre_termination']['peak'] != exact:
                    raise ValueError('deadline control changed its exact retained peak')
            result['native'] = completion
            result['complete'] = True
        except (OSError, ValueError, PROCESS.BuildError) as error:
            result['error'] = str(error)
        finally:
            expected_deadline = (case.mode == 'deadline-control' and result['complete']
                                 and result['timed_out'] and result.get('group_absent'))
            if ((not expected_deadline and (result['exit_code'] != 0 or result['timed_out']))
                    or result['exit_code'] is None or not result.get('group_absent')):
                result['complete'] = False
                result.setdefault('error', 'child did not exit cleanly with an absent process group')
            report_path.write_text(json.dumps(report, indent=2) + '\n')
    report['complete'] = all(result['complete'] for result in report['cases'])
    try:
        if {**context(prefix, target), 'binary_sha256': digest(binary)} != current:
            raise ValueError('native build identity changed during the cases')
    except (OSError, ValueError) as error:
        report['complete'] = False
        report['error'] = str(error)
    report_path.write_text(json.dumps(report, indent=2) + '\n')
    if not report['complete']:
        raise ValueError('one or more native cases did not complete as registered')
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--prefix', type=Path, required=True)
    parser.add_argument('--target', choices=PROCESS.TARGETS, required=True)
    parser.add_argument('--output', type=Path, required=True)
    options = parser.parse_args()
    with PROCESS.cancellation():
        run(options.prefix.resolve(), options.target, options.output.resolve())


if __name__ == '__main__':
    main()

"""Bind compatibility artifacts to the registered candidate and current tooling."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
import registered_candidate

README = 'docs/measurements/compat-0.1.0/README.md'
FROZEN = (
    'scripts/oracle-ffmpeg.env',
    'scripts/install-oracle-ffmpeg.sh',
    'scripts/install-oracle-mediainfo.sh',
    'scripts/oracle-audio-args.txt',
    'tools/compat-corpus/commands-0.1.0',
    'tools/compat-corpus/round_files.py',
    'crates/coreav-cli/tests/ffmpeg.rs',
    'crates/coreav-isobmff/tests/common',
)
ARTIFACTS = {'coreav': 'candidate', 'r3-harness': 'observer'}
FILES = set(ARTIFACTS) | {'sources.json', 'sha256.txt'}
SCHEMA = 'compat-0.1.0-sources/1'
JUDGING_REGISTRATION = 'docs/measurements/compat-0.1.0/judging.json'
JUDGING_SCHEMA = 'compat-0.1.0-judging/1'
JUDGING = (
    '.github/workflows/compat-0.1.0.yml',
    'docs/known-divergences-clip.json',
    'docs/known-divergences-extract.json',
    'docs/known-divergences-hls.json',
    'docs/known-divergences-join.json',
    'docs/known-divergences-mkv.json',
    'docs/known-divergences-remux.json',
    'docs/known-divergences-split.json',
    'docs/known-divergences-ts.json',
    'docs/known-divergences.json',
    'scripts/clip_check.py',
    'scripts/compat_check.py',
    'scripts/compat_sources.py',
    'scripts/extract_check.py',
    'scripts/hls_check.py',
    'scripts/join_check.py',
    'scripts/mkv_remux_check.py',
    'scripts/oracle_audio.py',
    'scripts/oracle_kd.py',
    'scripts/oracle_kd_mkv.py',
    'scripts/oracle_kd_ts.py',
    'scripts/oracle_pin.py',
    'scripts/policy_ref.py',
    'scripts/registered_candidate.py',
    'scripts/remux_check.py',
    'scripts/split_check.py',
    'scripts/track_boxes.py',
    'scripts/writer_boxes.py',
    'tools/compat-corpus/copy_check.py',
    'tools/compat-corpus/ffmpeg9.json',
    'tools/compat-corpus/r3.py',
    'tools/compat-corpus/release_round.py',
    'tools/compat-corpus/run.py',
    'tools/oracle/writer_corpus.py',
    'tools/stage3-report/corpus.py',
    'tools/stage3-report/r2pool.py',
)


def git(root, *args):
    done = subprocess.run(['git', *args], cwd=root, capture_output=True, text=True, check=False)
    if done.returncode:
        raise ValueError('source history is unavailable; check out the full candidate and tooling history')
    return done.stdout.strip()


def ancestor(root, older, newer):
    done = subprocess.run(['git', 'merge-base', '--is-ancestor', older, newer], cwd=root,
                          capture_output=True, check=False)
    if done.returncode not in (0, 1):
        raise ValueError('source ancestry is unavailable; check out the full candidate and tooling history')
    return done.returncode == 0


def full_sha(value):
    if not isinstance(value, str) or re.fullmatch(r'[0-9a-f]{40}', value) is None:
        raise ValueError('source commits must be full lowercase SHAs, not mutable refs')
    return value


def committed_blob(root, commit, path):
    done = subprocess.run(['git', 'cat-file', 'blob', f'{commit}:{path}'], cwd=root,
                          capture_output=True, check=False)
    if done.returncode:
        raise ValueError(f'{path} is unavailable in the tooling commit; check out its complete source')
    return done.stdout


def register(root, tooling):
    full_sha(tooling)
    files = {path: hashlib.sha256(committed_blob(root, tooling, path)).hexdigest() for path in JUDGING}
    return json.dumps({'schema': JUDGING_SCHEMA, 'files': files}, indent=2, sort_keys=True) + '\n'


def judging(root, tooling):
    raw = committed_blob(root, tooling, JUDGING_REGISTRATION)
    try:
        registration = json.loads(raw)
    except ValueError as error:
        raise ValueError(f'{JUDGING_REGISTRATION} must contain valid judging JSON') from error
    if not isinstance(registration, dict) or set(registration) != {'schema', 'files'} or \
            registration['schema'] != JUDGING_SCHEMA or not isinstance(registration['files'], dict):
        raise ValueError(f'{JUDGING_REGISTRATION} must register the judging schema and file hashes')
    files = registration['files']
    if set(files) != set(JUDGING):
        changed = sorted(set(files) ^ set(JUDGING))
        raise ValueError('judging registration has missing or extra files: ' + ', '.join(changed))
    for path in JUDGING:
        if not isinstance(files[path], str) or re.fullmatch(r'[0-9a-f]{64}', files[path]) is None:
            raise ValueError(f'{path} needs a full lowercase SHA-256 in the judging registration')
        if files[path] != hashlib.sha256(committed_blob(root, tooling, path)).hexdigest():
            raise ValueError(f'{path} differs from its registered judging hash; review it, then run compat_sources.py register')
    return {'registration': JUDGING_REGISTRATION, 'registration_sha256': hashlib.sha256(raw).hexdigest(),
            'files': files}


def plan(root, event, ref, commit, tooling):
    full_sha(tooling)
    if git(root, 'rev-parse', 'HEAD') != tooling:
        raise ValueError('the tooling checkout must equal the dispatched source SHA')
    if git(root, 'status', '--porcelain', '--untracked-files=no'):
        raise ValueError('the tooling checkout must have no modified tracked files')
    readme = git(root, 'show', f'{tooling}:{README}')
    if event == 'workflow_dispatch':
        candidate = registered_candidate.allowed(readme, ref, commit)
    elif event == 'pull_request':
        candidate = registered_candidate.registered(readme)
    else:
        raise ValueError('compatibility sources require a dispatch or pull request event')
    floor = registered_candidate.tooling_floor(readme)
    if not ancestor(root, floor, tooling):
        raise ValueError('tooling must descend from the registered checker-deadline floor')
    on_main = ancestor(root, tooling, 'origin/main')
    if event == 'workflow_dispatch' and not on_main:
        raise ValueError('dispatched tooling must be a commit of main')
    if not ancestor(root, candidate, tooling):
        raise ValueError('tooling must descend from the registered release candidate')
    frozen = {}
    for path in FROZEN:
        expected = git(root, 'rev-parse', '--verify', f'{candidate}:{path}')
        actual = git(root, 'rev-parse', '--verify', f'{tooling}:{path}')
        if expected != actual:
            raise ValueError(f'{path} differs from the registered candidate; keep oracle, input and harness sources fixed')
        frozen[path] = actual
    return {'commit': candidate, 'tooling': tooling, 'floor': floor, 'on_main': on_main,
            'frozen': frozen, 'judging': judging(root, tooling)}


def digest(path):
    h = hashlib.sha256()
    with path.open('rb') as stream:
        for block in iter(lambda: stream.read(1 << 20), b''):
            h.update(block)
    return h.hexdigest()


def context():
    return {'id': os.environ['GITHUB_RUN_ID'], 'attempt': os.environ['GITHUB_RUN_ATTEMPT'],
            'event': os.environ['GITHUB_EVENT_NAME'], 'ref': os.environ['GITHUB_REF'], 'set': os.environ['SET']}


def record(root, candidate_dir, dist, commit, tooling, run, compiler):
    sources = plan(root, run['event'], run['ref'], commit, tooling)
    if sources['commit'] != commit or git(candidate_dir, 'rev-parse', 'HEAD') != commit:
        raise ValueError('both Rust artifacts must be built in the registered candidate checkout')
    for directory in (root, candidate_dir):
        if git(directory, 'status', '--porcelain', '--untracked-files=no'):
            raise ValueError('source checkouts must have no modified tracked files')
    if run['event'] == 'pull_request' and run['set'] != 'rehearsal':
        raise ValueError('pull requests may only rehearse on spent inputs')
    artifacts = {name: {'role': role, 'built_from': commit, 'sha256': digest(dist / name)}
                 for name, role in ARTIFACTS.items()}
    if artifacts['coreav']['sha256'] == artifacts['r3-harness']['sha256']:
        raise ValueError('the candidate binary and observer harness must be distinct artifacts')
    manifest = {'schema': SCHEMA, 'run': run,
                'candidate': {'commit': commit, 'registration': README},
                'tooling': {'commit': tooling, 'floor': sources['floor'], 'on_main': sources['on_main']},
                'frozen': sources['frozen'], 'judging': sources['judging'], 'artifacts': artifacts,
                'build': {'coreav': 'cargo build --locked --profile dist -p coreav-cli',
                          'r3-harness': 'cargo test --locked --release -p coreav-cli --test ffmpeg --no-run',
                          'harness_test': 'the_0_1_0_commands_on_listed_pairs', 'features': 'default'},
                'rustc': compiler.strip()}
    (dist / 'sha256.txt').write_text(''.join(f"{value['sha256']}  {name}\n" for name, value in artifacts.items()))
    (dist / 'sources.json').write_text(json.dumps(manifest, indent=2) + '\n')
    verify(root, dist, commit, tooling, run)
    return manifest


def verify(root, dist, commit, tooling, run):
    full_sha(commit)
    full_sha(tooling)
    if set(p.name for p in dist.iterdir()) != FILES or any(p.is_symlink() or not p.is_file() for p in dist.iterdir()):
        raise ValueError('the artifact bundle must contain exactly both Rust artifacts and their two provenance files')
    manifest = json.loads((dist / 'sources.json').read_text())
    if manifest['schema'] != SCHEMA or manifest['candidate']['commit'] != commit:
        raise ValueError('artifact provenance does not name the planned candidate')
    if manifest['tooling']['commit'] != tooling or git(root, 'rev-parse', 'HEAD') != tooling:
        raise ValueError('artifact provenance does not name this tooling checkout')
    if git(root, 'status', '--porcelain', '--untracked-files=no'):
        raise ValueError('the tooling checkout must have no modified tracked files')
    if any(manifest['run'][key] != run[key] for key in ('id', 'event', 'ref', 'set')):
        raise ValueError('artifact provenance is from another run or input set')
    readme = git(root, 'show', f'{tooling}:{README}')
    if manifest['candidate']['registration'] != README:
        raise ValueError('artifact provenance names the wrong registration')
    if run['event'] == 'workflow_dispatch':
        registered_candidate.allowed(readme, run['ref'], commit)
        if manifest['tooling']['on_main'] is not True:
            raise ValueError('dispatched artifacts require tooling from main')
    elif run['event'] != 'pull_request' or run['set'] != 'rehearsal':
        raise ValueError('pull requests may only rehearse on spent inputs')
    if registered_candidate.registered(readme) != commit:
        raise ValueError('artifact candidate differs from the registered release candidate')
    if manifest['tooling']['floor'] != registered_candidate.tooling_floor(readme):
        raise ValueError('artifact tooling floor differs from the registration')
    if set(manifest['frozen']) != set(FROZEN):
        raise ValueError('artifact provenance lacks the complete frozen source roster')
    for path in FROZEN:
        if manifest['frozen'][path] != git(root, 'rev-parse', '--verify', f'{tooling}:{path}'):
            raise ValueError(f'artifact provenance disagrees with tooling source {path}')
    if manifest.get('judging') != judging(root, tooling):
        raise ValueError('artifact provenance differs from the registered judging hashes')
    if set(manifest['artifacts']) != set(ARTIFACTS):
        raise ValueError('artifact provenance has the wrong artifact roles')
    checksums = {}
    for line in (dist / 'sha256.txt').read_text().splitlines():
        sha, name = line.split()
        if name in checksums:
            raise ValueError('duplicate artifact checksum')
        checksums[name] = sha
    if set(checksums) != set(ARTIFACTS):
        raise ValueError('artifact checksum roster differs from the planned bundle')
    for name, role in ARTIFACTS.items():
        artifact = manifest['artifacts'][name]
        if artifact['role'] != role or artifact['built_from'] != commit:
            raise ValueError(f'{name} was not built from the registered candidate with its planned role')
        if artifact['sha256'] != digest(dist / name) or artifact['sha256'] != checksums[name]:
            raise ValueError(f'{name} bytes differ from the recorded build artifact')
    if manifest['artifacts']['coreav']['sha256'] == manifest['artifacts']['r3-harness']['sha256']:
        raise ValueError('the candidate binary and observer harness must be distinct artifacts')
    return manifest


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='command', required=True)
    for name in ('plan', 'record', 'verify', 'register'):
        command = commands.add_parser(name)
        if name != 'register':
            command.add_argument('--commit', required=True)
        command.add_argument('--tooling', required=True)
        if name == 'plan':
            command.add_argument('--event', required=True)
            command.add_argument('--ref', required=True)
        elif name != 'register':
            command.add_argument('--dist', required=True, type=Path)
            if name == 'record':
                command.add_argument('--candidate-dir', required=True, type=Path)
    args = parser.parse_args()
    root = Path.cwd()
    try:
        if args.command == 'register':
            print(register(root, args.tooling), end='')
        elif args.command == 'plan':
            sources = plan(root, args.event, args.ref, args.commit, args.tooling)
            print('\n'.join(f'{key}={sources[key]}' for key in ('commit', 'tooling', 'floor')))
        elif args.command == 'record':
            compiler = subprocess.check_output(['rustc', '-Vv'], cwd=args.candidate_dir, text=True)
            record(root, args.candidate_dir, args.dist, args.commit, args.tooling, context(), compiler)
        else:
            verify(root, args.dist, args.commit, args.tooling, context())
    except (OSError, ValueError, KeyError, TypeError, subprocess.CalledProcessError) as error:
        print(f'::error::invalid compatibility sources: {error}', file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    sys.exit(main())

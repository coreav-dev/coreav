#!/usr/bin/env python3
"""Compare exactly the registered synthetic HEVC pictures on Linux."""
import argparse
import json
import pathlib
import sys
import tempfile

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import hevc_conformance as harness
import hevc_fixture as fixture
import hevc_intra as intra
import hevc_synthetic as synthetic

MEMORY = 256 << 20
BINS = 1 << 28
REASONS = {stage: reason for stage, reason in (
    ('platform', 'linux_required'), ('inspection', 'synthetic_inspection_failed'),
    ('membership', 'recipe_membership_invalid'), ('targets', 'picture_membership_invalid'),
    ('identity', 'source_identity_changed'), ('oracle', 'oracle_failed'),
    ('native', 'native_failed'), ('metadata', 'metadata_invalid'),
    ('comparison', 'pixels_invalid'), ('aggregate', 'picture_completion_invalid'))}


def run(args):
    result = dict(complete=False, stage='platform', results=[])
    try:
        harness.require(sys.platform == 'linux', 'linux_required')
        with tempfile.TemporaryDirectory(prefix='hevc-smoke-') as temporary:
            work = pathlib.Path(temporary)
            result['stage'] = 'inspection'
            qualified = harness.test_oracle.require(args.oracle_bin / 'ffmpeg', args.oracle_bin / 'ffprobe')
            census = synthetic.inspect(args.data, work / 'headers', args.parser, qualified, harness)
            result['stage'] = 'membership'
            recipes = fixture.recipes()
            expected_tests = ['SYNTHETIC_' + recipe['id'] for recipe in recipes]
            harness.require(len(recipes) == len(set(expected_tests)) == 4
                            and census.get('complete') is True
                            and [row['test'] for row in census['streams']] == expected_tests,
                            'recipe_membership_invalid')
            streams = []
            expected = {(test, index) for test in expected_tests for index in (0, 1)}
            result['stage'] = 'targets'
            for recipe, record in zip(recipes, census['streams']):
                directory = work / 'headers' / recipe['id']
                stream = intra.stream_targets(args.data / 'run-1' / (recipe['id'] + '.hevc'),
                    record, directory / 'native.stdout', directory, record['test'], 'synthetic')
                harness.require(stream is not None, 'picture_membership_invalid')
                streams.append(stream)
            identities = [(stream['test'], target['candidate']['picture_index'])
                          for stream in streams for target in stream['targets']]
            harness.require(len(identities) == len(set(identities)) == 8
                            and set(identities) == expected, 'picture_membership_invalid')
            for ordinal, stream in enumerate(streams):
                source = stream['source']
                result['stage'] = 'identity'
                harness.require(harness.digest(source) == stream['record']['sha256'], 'source_identity_changed')
                directory = work / 'pictures' / str(ordinal)
                result['stage'] = 'oracle'
                oracle = intra.oracle_frames(qualified.ffmpeg, source, directory, stream)
                harness.require(len(oracle) == 2, 'oracle_failed')
                for target in stream['targets']:
                    candidate = target['candidate']
                    index = candidate['picture_index']
                    output = directory / (str(index) + '.u16le')
                    result['stage'] = 'native'
                    code, stdout, _ = harness.child(intra.native_command(args.native, source, index,
                        output, MEMORY, BINS), directory, 'native-' + str(index), file_limit=MEMORY)
                    harness.require(code == 0 and stdout.stat().st_size < harness.TEXT_CAP, 'native_failed')
                    result['stage'] = 'metadata'
                    lines = stdout.read_text().splitlines()
                    harness.require(len(lines) == 1, 'metadata_invalid')
                    metadata = json.loads(lines[0], object_pairs_hook=harness.unique_object)
                    intra.validate_metadata(metadata, target, stream, source, MEMORY, BINS)
                    harness.require(output.stat().st_size < MEMORY, 'metadata_invalid')
                    result['stage'] = 'comparison'
                    comparison = intra.compare_picture(output, metadata, candidate,
                        oracle[candidate['reference']['output_index']])
                    result['results'].append(dict(test=stream['test'], picture_index=index,
                                                   complete=True, comparison=comparison))
                    output.unlink()
                result['stage'] = 'identity'
                harness.require(harness.digest(source) == stream['record']['sha256'], 'source_identity_changed')
            result['stage'] = 'aggregate'
            actual = [(row['test'], row['picture_index']) for row in result['results'] if row['complete']]
            harness.require(len(actual) == len(set(actual)) == 8 and set(actual) == expected,
                            'picture_completion_invalid')
        result.update(complete=True, stage='complete')
    except Exception:
        result['reason'] = REASONS[result['stage']]
    finally:
        harness.write_json(args.out / 'result.json', result)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ('data', 'out', 'parser', 'native', 'oracle-bin'):
        parser.add_argument('--' + name, type=pathlib.Path, required=True)
    result = run(parser.parse_args())
    print('hevc-smoke: ' + result['stage'] + ':' + result.get('reason', 'complete'))
    return 0 if result['complete'] else 1


if __name__ == '__main__':
    sys.exit(main())

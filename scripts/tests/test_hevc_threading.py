import copy
import pathlib
import sys
import tempfile
import types
import unittest
from unittest.mock import patch

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
import hevc_threading as threading
import hevc_decode


class ThreadingReference(unittest.TestCase):
    def test_census_counts_only_complete_native_syntax_and_uses_qualified_extraction(self):
        with tempfile.TemporaryDirectory() as temp:
            root = pathlib.Path(temp)
            source = root / 'source'
            source.write_bytes(b'fixture')
            oracle = types.SimpleNamespace(ffmpeg='/qualified-ffmpeg')
            commands = []
            def child(command, directory, label, **unused):
                commands.append(command)
                directory.mkdir(parents=True, exist_ok=True)
                if label == 'extract':
                    pathlib.Path(command[-1]).write_bytes(b'fixture')
                stdout = directory / (label + '.stdout')
                stdout.write_text('{"kind":"summary","complete":true,"tools":{"wpp":true,"tiles":false}}')
                return 0, stdout, directory / 'stderr'
            with patch.object(threading.conformance, 'child', side_effect=child), \
                    patch.object(threading.conformance, 'digest', return_value='b' * 64):
                result = threading.syntax_record(dict(sha256='a' * 64), source, root, oracle, '/native-parser')
            self.assertEqual((result['status'], result['wpp'], result['tiles']), ('inspected', True, False))
            self.assertEqual(commands[0][0], oracle.ffmpeg)
            self.assertEqual(commands[1][0], '/native-parser')
            self.assertFalse((root / ('a' * 64) / 'syntax.hevc').exists())

    def test_identity_ignores_only_resource_and_scheduling_counters(self):
        one = dict(output_md5='a' * 32, stats=dict(ctus=3, memory_bytes=100), parallel_pictures=0, max_workers=1)
        many = copy.deepcopy(one)
        many.update(parallel_pictures=1, max_workers=8)
        many['stats']['memory_bytes'] = 500
        self.assertEqual(threading.normalized(one), threading.normalized(many))
        for owner, key in [(many, 'output_md5'), (many['stats'], 'ctus')]:
            old = owner[key]
            owner[key] = 'changed'
            self.assertNotEqual(threading.normalized(one), threading.normalized(many))
            owner[key] = old

    def test_thread_count_reaches_every_conformance_control(self):
        args = types.SimpleNamespace(native='/native', max_memory_bytes=100, max_stream_bins=200, max_picture_bins=300, threads=8)
        self.assertEqual(hevc_decode.command(args, '/input'), ['/native', '/input', '100', '200', '300', '8'])
        self.assertEqual(hevc_decode.command(args, '/input', stream_bins=0), ['/native', '/input', '100', '0', '300', '8'])
        del args.threads
        self.assertEqual(hevc_decode.command(args, '/input'), ['/native', '/input', '100', '200', '300'])

    def test_signature_requires_completion_and_supplies_threads(self):
        with patch.object(threading.reference, 'command', return_value=(b'{"type":"summary","complete":true}', {})) as child:
            threading.signature('/native', '/input', 4)
            self.assertEqual(child.call_args.args[0], ['/native', '/input', 'check', 4])
        with patch.object(threading.reference, 'command', return_value=(b'{"complete":false}', {})):
            with self.assertRaisesRegex(ValueError, 'incomplete_native'):
                threading.signature('/native', '/input', 4)

    def test_output_mismatch_prevents_any_timing(self):
        args = types.SimpleNamespace(native='/candidate', main='/main')
        with patch.object(threading.conformance, 'digest', return_value='a' * 64), \
             patch.object(threading.reference, 'verify', return_value=dict(frames=2)), \
             patch.object(threading, 'signature', side_effect=[[dict(output_md5='a')], [dict(output_md5='b')]]), \
             patch.object(threading.reference, 'command') as child:
            with self.assertRaisesRegex(ValueError, 'thread_output_mismatch'):
                threading.measure(args, object(), pathlib.Path('/input'), 'd00')
            child.assert_not_called()

    def test_ratio_direction_and_every_requested_count(self):
        calls = []
        for threads in threading.THREADS:
            calls += [dict(arm=f'main_{threads}', wall=12), dict(arm=f'candidate_{threads}', wall=6 / threads)]
        calls += [dict(arm='ffmpeg_one', wall=2)]
        summary = threading.summarize([dict(status='timed', depth=8, calls=calls)])['8']
        self.assertEqual(summary['main_over_candidate']['4'], 8)
        self.assertEqual(summary['candidate_one_over_threads']['4'], 4)
        self.assertEqual(summary['candidate_over_ffmpeg_one']['1'], 3)


if __name__ == '__main__':
    unittest.main()

import importlib.util
import json
import pathlib
import subprocess
import tempfile
from types import SimpleNamespace
import unittest
from unittest import mock

PATH = pathlib.Path(__file__).resolve().parents[1] / 'vp9_pixel_ceiling.py'
SPEC = importlib.util.spec_from_file_location('vp9_pixel_ceiling', PATH)
runner = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(runner)


class Sources(unittest.TestCase):
    def test_checkpoint_uses_only_the_first_development_input_of_each_class(self):
        plan, rows = runner.selection()
        self.assertEqual(rows, [plan['inputs'][i] for i in (0, 2, 4)])
        self.assertEqual([r['class'] for r in rows], ['720p', '1080p', '4K'])

    def test_scratch_removes_only_the_requested_function_body(self):
        text = 'fn first() {\n    if true { call(); }\n}\nfn second() { keep(); }\n'
        self.assertEqual(runner.replace_body(text, 'first', ''),
                         'fn first() {\n\n}\nfn second() { keep(); }\n')
        with self.assertRaises(ValueError):
            runner.replace_body(text, 'absent', '')

    def test_both_adapters_drop_frames_and_only_the_ceiling_cuts_out_pixels(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = pathlib.Path(temporary)
            paths = ['tools/decode-oracle/src/main.rs', 'crates/coreav-kernels-vp9/src/inter.rs',
                     'crates/coreav-kernels-vp9/src/filter.rs', 'crates/coreav-vp9/src/picture.rs']
            originals = dict(zip(paths, [
                'fn drain(decoder: &mut dyn Decoder, frames: &mut Vec<Value>) -> Result<(), String> {\n'
                '    picture.write_packed(&mut packed);\n}\nfn next() { untouched(); }\n',
                'pub fn predict_inter<P: Pixel>(dst: D, src: S) {\n    repeated();\n}\n'
                '#[cfg(test)]\nmod tests;\n',
                'fn wide<P: Pixel>(edge: E, log2_size: u32) {\n    repeated();\n}\n'
                '#[cfg(test)]\nmod tests;\n',
                '            let mut samples = memory.empty(len)?;\n'
                '            samples.extend(plane.samples.iter().copied());\n'
                '            Ok(P::samples(samples))\n',
            ]))
            for relative in paths:
                for arm in runner.ARMS:
                    path = root / arm / relative
                    path.parent.mkdir(parents=True, exist_ok=True)
                    path.write_text(originals[relative])
            for arm in runner.ARMS:
                runner.patch_source(root / arm, arm == 'ceiling')
            baseline = (root / 'baseline' / paths[0]).read_text()
            ceiling = (root / 'ceiling' / paths[0]).read_text()
            self.assertEqual(baseline, ceiling)
            self.assertNotIn('write_packed', baseline)
            self.assertIn('decoder.receive_frame()', baseline)
            for relative, name in zip(paths[1:3], ['predict_inter', 'wide']):
                self.assertEqual((root / 'baseline' / relative).read_text(), originals[relative])
                cutout = (root / 'ceiling' / relative).read_text()
                self.assertIn(f'fn {name}', cutout)
                self.assertIn('mod tests;', cutout)
            self.assertIn('samples.resize(len, P::default())', (root / 'ceiling' / paths[3]).read_text())
            self.assertEqual((root / 'baseline' / paths[3]).read_text(), originals[paths[3]])

    def test_children_cannot_inherit_credentials_or_cargo_overrides(self):
        with mock.patch.dict(runner.os.environ, {'R2_SECRET_ACCESS_KEY': 'secret-marker',
                             'CAPY_API_KEY': 'secret-marker', 'RUSTFLAGS': '-Ctarget-cpu=native',
                             'CARGO_PROFILE_DIST_LTO': 'off'}):
            env = runner.environment()
        self.assertFalse(any(k in env for k in ('R2_SECRET_ACCESS_KEY', 'CAPY_API_KEY',
                                               'RUSTFLAGS', 'CARGO_PROFILE_DIST_LTO')))
        self.assertEqual(env['CARGO_BUILD_JOBS'], '2')


class Counter(unittest.TestCase):
    def test_counter_requires_one_positive_instruction_only_summary(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = pathlib.Path(temporary) / 'counts'
            for text in ('events: Ir\nsummary: 123\n', 'events: Ir\nsummary: 0\n',
                         'events: Ir Dr\nsummary: 123 456\n',
                         'events: Ir\nsummary: 123\nsummary: 456\n', 'events: Ir\n'):
                with self.subTest(text=text):
                    path.write_text(text)
                    if text == 'events: Ir\nsummary: 123\n':
                        self.assertEqual(runner.instructions(path), 123)
                    else:
                        with self.assertRaises(ValueError):
                            runner.instructions(path)


class Capture(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        root = pathlib.Path(self.temporary.name)
        self.scratch, self.media = root / 'scratch', root / 'media'
        self.scratch.mkdir()
        self.media.mkdir()
        self.rows = [dict(sha256='', width=w, height=h, **{'class': name})
                     for name, w, h in [('720p', 1280, 720), ('1080p', 1920, 1080), ('4K', 3840, 2160)]]
        self.plan = dict(baseline_commit='a' * 40)
        listing = []
        for i, row in enumerate(self.rows):
            path = self.media / f'{i}.webm'
            path.write_bytes(bytes([i]))
            row['sha256'] = runner.digest(path)
            listing.append(dict(path=str(path), sha256=row['sha256']))
        runner.write(self.media / 'files.json', listing)
        provenance = dict(plan_sha256=runner.digest(runner.PLAN), baseline_commit='a' * 40,
                          driver_sha256=runner.digest(PATH), indexes=list(runner.INDEXES),
                          packets=runner.PACKETS, arms={})
        for arm in runner.ARMS:
            source = self.scratch / arm
            binary = source / 'target/dist/decode-oracle'
            binary.parent.mkdir(parents=True)
            binary.write_bytes(b'binary')
            file = source / 'source.rs'
            file.write_text('source')
            provenance['arms'][arm] = {'source.rs': runner.digest(file)}
        runner.write(self.scratch / 'sources.json', provenance)
        self.args = SimpleNamespace(out=str(self.scratch), media=str(self.media),
                                    private=str(root / 'private'), summary=str(root / 'summary.json'))
        self.mutation = lambda arm, document: None
        self.calls = []

    def child(self, command, **kwargs):
        if command == ['valgrind', '--version']:
            return subprocess.CompletedProcess(command, 0, stdout=b'valgrind-test\n')
        self.calls.append(command)
        self.assertNotIn('R2_SECRET_ACCESS_KEY', kwargs['env'])
        path = pathlib.Path(command[-5])
        i = int(path.stem)
        row = self.rows[i]
        arm = pathlib.Path(command[-6]).parents[2].name
        document = dict(codec='vp9', decoder='coreav vp9, scalar profile 0', fed=runner.PACKETS,
                        frames=[dict(pts=0, width=row['width'], height=row['height'])], cutoff=8, stream_index=0)
        self.mutation(arm, document)
        kwargs['stdout'].write(json.dumps(document).encode())
        counter = next(c.split('=', 1)[1] for c in command if c.startswith('--callgrind-out-file='))
        pathlib.Path(counter).write_text('events: Ir\nsummary: 123\n')
        return subprocess.CompletedProcess(command, 0)

    def capture(self):
        with mock.patch.object(runner.sys, 'platform', 'linux'), \
                mock.patch.object(runner.platform, 'machine', return_value='x86_64'), \
                mock.patch.object(runner, 'selection', return_value=(self.plan, self.rows)), \
                mock.patch.object(runner.subprocess, 'run', side_effect=self.child):
            runner.capture(self.args)

    def test_complete_roster_has_counts_but_no_elapsed_metrics_or_input_identity(self):
        self.capture()
        summary = runner.read(self.args.summary)
        self.assertTrue(summary['complete'])
        self.assertEqual(len(self.calls), 6)
        self.assertEqual(len(summary['classes']), 3)
        self.assertFalse(summary['pixel_exact'])
        text = json.dumps(summary)
        for forbidden in ('elapsed', 'speedup', 'wall', str(self.media), self.rows[0]['sha256']):
            self.assertNotIn(forbidden, text)

    def test_wrong_dimensions_leave_an_incomplete_capture(self):
        self.mutation = lambda arm, document: document['frames'][0].update(width=1)
        with self.assertRaisesRegex(ValueError, 'work'):
            self.capture()
        self.assertFalse(runner.read(self.args.summary)['complete'])

    def test_missing_packets_or_frames_cannot_become_a_complete_capture(self):
        self.mutation = lambda arm, document: document.update(fed=0, frames=[])
        with self.assertRaisesRegex(ValueError, 'work'):
            self.capture()
        self.assertFalse(runner.read(self.args.summary)['complete'])

    def test_changed_frame_work_between_arms_is_refused(self):
        def mutate(arm, document):
            if arm == 'ceiling':
                document['frames'][0]['pts'] = 1
        self.mutation = mutate
        with self.assertRaisesRegex(ValueError, 'differs'):
            self.capture()

    def test_reordered_or_changed_input_is_refused_before_decoder_execution(self):
        listing = runner.read(self.media / 'files.json')
        runner.write(self.media / 'files.json', list(reversed(listing)))
        with self.assertRaisesRegex(ValueError, 'roster'):
            self.capture()
        self.assertEqual(self.calls, [])

    def test_changed_source_is_refused_before_decoder_execution(self):
        (self.scratch / 'ceiling/source.rs').write_text('changed')
        with self.assertRaisesRegex(ValueError, 'source'):
            self.capture()
        self.assertEqual(self.calls, [])


if __name__ == '__main__':
    unittest.main()

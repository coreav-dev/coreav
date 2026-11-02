import copy
import hashlib
import json
import math
import os
import pathlib
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
import h264_g1 as g1
import h264_g1_conformance as conformance


def manifest():
    slots = ((1, 2, 3), (2, 3, 4), (3, 4, 1), (4, 1, 2), (1, 2, 3), (2, 3, 4), (3, 4, 1), (4, 1, 2))
    clips = []
    for cls, groups in enumerate(slots, 1):
        group = []
        for resolution in groups:
            n = len(clips) + 1
            w, h = {1: (16, 16), 2: (640, 480), 3: (1280, 960), 4: (1920, 1080)}[resolution]
            clip = dict(label=f'G1-{n:02}', parent=f'parent-{n}', sha256=f'{n:064x}', original_sha256=f'{n + 100:064x}',
                        width=w, height=h, frames=60, bytes=w*h*3//2*60, licence_approved=True, licence='CC0',
                        preparation=['frozen'], source_declarations={}, original_geometry=[w, h], original_timing=None, original_bytes=123,
                        first_frame=0, selected_stream=0, resolution_group=resolution, **{'class': cls}, split='held-out')
            group.append(clip)
            clips.append(clip)
        min(group, key=lambda c: hashlib.sha256(('coreav-h264-g1-v1:' + c['sha256']).encode()).digest())['split'] = 'training'
    tool = dict(sha256='a'*64, version='frozen', build='frozen command', compiler='frozen compiler', archive_sha256='b'*64)
    return dict(protocol='coreav-h264-g1-v1', qps=list(g1.QPS), frames=60, timing=[30, 1], colour=[False, 1, 1, 1, 0], clips=clips,
                ordered_planes_sha256=g1.sha([c['sha256'] for c in clips]),
                tools={n: dict(tool) for n in ('ffmpeg', 'ffprobe', 'jm', 'x264', 'metric_ffmpeg', 'ssimulacra2')},
                commands={n: ['{metric_ffmpeg}', 'frozen'] for n in ('convert', 'ssimulacra2', 'xpsnr')},
                metric_fixture=dict(frames=60, width=16, height=16, finite=True, reference_sha256='c'*64, reconstruction_sha256='e'*64, png_sha256=['d'*64]*60))


def cells(m, ratio=1.):
    result = []
    for clip in m['clips']:
        for arm in ('coreav', 'x264'):
            for i, qp in enumerate(g1.QPS):
                result.append(dict(label=clip['label'], arm=arm, qp=qp, frames=60, conforming=True,
                                   bytes=round((5000 - i*900) * (ratio if arm == 'coreav' else 1)),
                                   ssimulacra2=90 - i*10, ssimulacra2_frames=[90-i*10]*60,
                                   xpsnr_y=50 - i*5, xpsnr_cb=51-i*5, xpsnr_cr=52-i*5))
    return result


class G1(unittest.TestCase):
    def test_pre_registered_curves_identical_and_constant_ratio(self):
        for quality in ([0., 1., 2., 3., 4.], [-5., 0., 7., 19., 20.]):
            points = [(q, math.exp(q/10 + 3)) for q in quality]
            self.assertAlmostEqual(g1.bd_rate(points, points), 0., places=10)
            for ratio in (0.5, 1.3, 2.):
                ours = [(q, r*ratio) for q, r in points]
                self.assertAlmostEqual(g1.bd_rate(ours, points), (ratio-1)*100, places=9)
            c = g1.curve(points)
            expected = 3*(quality[-1]-quality[0]) + (quality[-1]**2-quality[0]**2)/20
            self.assertAlmostEqual(g1.integrate(c, quality[0], quality[-1]), expected, places=9)

    def test_invalid_curves_fail_instead_of_repair(self):
        good = [(i, 100*(i+1)) for i in range(5)]
        for bad in (good[:-1], [(0, 100)]*5, [(i, 100) for i in range(5)], [(i, -100) for i in range(5)], [(i, float('nan')) for i in range(5)]):
            with self.assertRaises(ValueError):
                g1.bd_rate(bad, good)
        with self.assertRaises(ValueError):
            g1.bd_rate(good, [(q+3, r) for q, r in good])

    def test_both_metrics_and_all_sixteen_held_out_clips_are_required(self):
        m = manifest()
        result = g1.report(m, cells(m))
        self.assertTrue(result['pass'])
        self.assertEqual(result['cells'], 240)
        self.assertEqual(len(result['per_clip']), 24)
        result = g1.report(m, cells(m, 1.4))
        self.assertFalse(result['pass'])
        self.assertFalse(result['reconsider'])
        self.assertAlmostEqual(result['held_out_medians']['xpsnr_y'], 40.)
        self.assertTrue(g1.report(m, cells(m, 1.7))['reconsider'])
        for changed in (cells(m)[:-1], cells(m)+cells(m)[:1]):
            with self.assertRaises(ValueError):
                g1.report(m, changed)
        for key, value in [('conforming', False), ('frames', 59), ('xpsnr_y', float('nan'))]:
            changed = cells(m)
            changed[0][key] = value
            with self.assertRaises(ValueError):
                g1.report(m, changed)

    def test_freeze_missing_unmerged_or_digest_changed_refuses_before_work(self):
        with patch.object(g1, 'git') as git:
            git.side_effect = subprocess.CalledProcessError(1, ['git'])
            with self.assertRaises(subprocess.CalledProcessError):
                g1.load_freeze('a'*40, 'absent.json', 'b'*64)
            self.assertEqual(git.call_count, 1)
        data = json.dumps(manifest()).encode()
        with patch.object(g1, 'git', side_effect=[b'', data]):
            with self.assertRaisesRegex(ValueError, 'digest'):
                g1.load_freeze('a'*40, 'manifest.json', 'b'*64)
        with patch.object(g1, 'git', side_effect=[b'', data]):
            self.assertEqual(g1.load_freeze('a'*40, 'manifest.json', hashlib.sha256(data).hexdigest())['frames'], 60)

    def test_missing_licence_slots_split_and_tool_pin_are_rejected(self):
        m = manifest()
        g1.validate(m)
        for mutate in (lambda m: m['clips'].pop(), lambda m: m['clips'][0].update(licence_approved=False),
                       lambda m: m['clips'][0].update(split='training' if m['clips'][0]['split'] == 'held-out' else 'held-out'),
                       lambda m: m['tools']['jm'].pop('sha256'), lambda m: m['metric_fixture'].update(frames=59)):
            bad = copy.deepcopy(m)
            mutate(bad)
            with self.assertRaises((ValueError, KeyError)):
                g1.validate(bad)

    def test_complete_plane_comparison_rejects_chroma_mutation_and_bad_lengths(self):
        with tempfile.TemporaryDirectory() as temp:
            a, b = pathlib.Path(temp)/'a', pathlib.Path(temp)/'b'
            a.write_bytes(bytes(range(24)))
            b.write_bytes(a.read_bytes())
            conformance.same_planes(a, b, 24)
            for value in (bytes(range(23)), bytes(range(25)), bytes(range(23))+b'\0', bytes(range(12, 24))+bytes(range(12))):
                b.write_bytes(value)
                with self.assertRaises(ValueError):
                    conformance.same_planes(a, b, 24)

    def test_command_failures_are_not_conformance_success(self):
        with tempfile.TemporaryDirectory() as temp, patch.object(conformance, 'run', side_effect=subprocess.CalledProcessError(1, ['oracle'])):
            with self.assertRaises(subprocess.CalledProcessError):
                conformance.compare(pathlib.Path('driver'), pathlib.Path('ffmpeg'), pathlib.Path('jm'), pathlib.Path('input'), pathlib.Path(temp)/'cell', 16, 16, 22, 3)

    def test_sei_removal_preserves_original_delimiters_and_other_bytes(self):
        sps = b'\0\0\0\1\x67abc'
        sei = b'\0\0\1\x06\x05uuid\x80'
        idr = b'\0\0\1\x65def'
        self.assertEqual(g1.strip_identifying_sei(sps+sei+idr), sps+idr)
        with self.assertRaises(ValueError):
            g1.strip_identifying_sei(sps+sei.replace(b'\x05', b'\x01')+idr)

    def test_aggregate_mutation_and_missing_metric_detail_fail(self):
        m = manifest()
        for mutate in (lambda c: c.update(ssimulacra2=c['ssimulacra2']+1),
                       lambda c: c['ssimulacra2_frames'].pop(), lambda c: c.pop('xpsnr_cb')):
            changed = cells(m)
            mutate(changed[0])
            with self.assertRaises((ValueError, KeyError)):
                g1.report(m, changed)
        with tempfile.TemporaryDirectory() as temp:
            directory = pathlib.Path(temp)
            for i in range(60):
                (directory/f'ssimulacra2-{i:03}.json').write_text(json.dumps(dict(exit_code=0, stdout='-5.0', stderr='')))
            (directory/'xpsnr.json').write_text(json.dumps(dict(exit_code=0, stdout='XPSNR y: 40 u: 41 v: 42', stderr='')))
            result = g1.replay_scores(directory)
            self.assertEqual(result['ssimulacra2'], -5.)
            self.assertEqual(result['xpsnr_cb'], 41.)
            changed = dict(result, binding=g1.sha({}), artifacts={p.name: conformance.digest(p) for p in directory.iterdir()}, frames=60, conforming=True, bytes=100, arm='coreav', qp=22)
            changed['ssimulacra2_frames'] = [100.] * 60
            changed['ssimulacra2'] = 100.
            with self.assertRaisesRegex(ValueError, 'raw process output'):
                g1.verify_cell(changed, directory, {}, {})
            (directory/'ssimulacra2-000.json').unlink()
            with self.assertRaises(ValueError):
                g1.replay_scores(directory)

    def test_zero_exit_with_coreav_decode_diagnostics_is_failure(self):
        result = subprocess.CompletedProcess(['decoder'], 0, stderr=b'decode: truncated NAL\n')
        with tempfile.TemporaryDirectory() as temp, patch.object(g1.subprocess, 'run', return_value=result):
            with self.assertRaisesRegex(ValueError, 'oracle diagnostics'):
                g1.decode_anchor(pathlib.Path('decoder'), pathlib.Path('stream'), pathlib.Path(temp))

    def test_jm_archive_fetch_requires_pinned_and_root_verified_intermediate(self):
        installer = pathlib.Path(__file__).resolve().parents[1] / 'install-jm-decoder.sh'
        with tempfile.TemporaryDirectory() as temp:
            root = pathlib.Path(temp)
            log = root/'calls'
            roots = root/'roots.pem'
            roots.write_text('existing trusted roots\n')
            commands = {
                'curl': '''#!/bin/bash
echo "curl $*" >> "$CALLS"
if [[ "$*" == *jm19.0.zip* ]]; then exit 42; fi
while [[ "$1" != -o ]]; do shift; done
printf certificate > "$2"
''',
                'openssl': '''#!/bin/bash
echo "openssl $*" >> "$CALLS"
if [[ "$1" == verify ]]; then exit "${VERIFY_EXIT:-0}"; fi
while [[ "$1" != -out ]]; do shift; done
printf der-certificate > "$2"
''',
                'sha256sum': '''#!/bin/bash
echo "sha256sum $*" >> "$CALLS"
cat >> "$CALLS"
exit "${HASH_EXIT:-0}"
'''}
            for name, text in commands.items():
                path = root/name
                path.write_text(text)
                path.chmod(0o755)
            env = dict(os.environ, PATH=str(root)+os.pathsep+os.environ['PATH'], CALLS=str(log), SSL_CERT_FILE=str(roots))
            for key, code in [(None, 42), ('VERIFY_EXIT', 31), ('HASH_EXIT', 32)]:
                log.write_text('')
                current = dict(env)
                if key:
                    current[key] = str(code)
                run = subprocess.run(['bash', str(installer), str(root/'bin')], env=current, capture_output=True, timeout=10)
                self.assertEqual(run.returncode, code, run.stderr.decode())
                calls = log.read_text()
                self.assertNotIn('--insecure', calls)
                if key is None:
                    self.assertIn('openssl verify -CAfile '+str(roots), calls)
                    self.assertIn('--cacert ', calls)
                    self.assertLess(calls.index('sha256sum'), calls.index('openssl verify'))
                    self.assertLess(calls.index('openssl verify'), calls.index('jm19.0.zip'))
                else:
                    self.assertNotIn('jm19.0.zip', calls)

    def test_jm_recipe_uses_only_the_decoder_build_directory(self):
        installer = pathlib.Path(__file__).resolve().parents[1] / 'install-jm-decoder.sh'
        text = installer.read_text()
        self.assertIn("-path '*/ldecod/Makefile'", text)
        self.assertIn('make -C "${BUILD%/*}" -j2 CC=', text)
        self.assertNotIn('-j2 ldecod ', text)


if __name__ == '__main__':
    unittest.main()

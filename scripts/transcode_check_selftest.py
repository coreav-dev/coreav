#!/usr/bin/env python3
"""Self-test of the pure parts of scripts/transcode_check.py and scripts/transcode_speed.py: the salted order and
the budgeted selection, the exact timestamp comparisons, the PSNR log parser, the bootstrap, the round order, the
speed run's top-up and its gate. No FFmpeg, no corpus, no CoreAV.

    python3 -I scripts/transcode_check_selftest.py
"""
import argparse
import hashlib
import json
import os
import pathlib
import sys
import tarfile
import tempfile
import unittest
from fractions import Fraction
from unittest import mock

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import transcode_check as tc  # noqa: E402
import transcode_speed as ts  # noqa: E402


def entry(i):
    return {'sha256': hashlib.sha256(str(i).encode()).hexdigest(), 'ext': 'mp4', 'size': 1, 'key': f'k{i}'}


class Selection(unittest.TestCase):
    def test_salted_order(self):
        sha = '0' * 64
        self.assertEqual(tc.salted(sha), hashlib.sha256(('coreav-transcode-v1:' + sha).encode()).hexdigest())
        self.assertNotEqual(tc.salted(sha), tc.salted(sha, ts.SALT))
        es = [entry(i) for i in range(50)]
        a = [e['sha256'] for e in sorted(es, key=lambda e: tc.salted(e['sha256']))]
        b = [e['sha256'] for e in sorted(reversed(es), key=lambda e: tc.salted(e['sha256']))]
        self.assertEqual(a, b)

    def probes(self, specs):
        """specs: {i: (codec, seconds, pixel_frames)}."""
        table = {entry(i)['sha256']: s for i, s in specs.items()}

        def probe_of(e):
            codec, seconds, pf = table[e['sha256']]
            if codec is None:
                return {'status': 'probe_failed'}
            return {'status': 'ok', 'codec': codec, 'seconds': seconds, 'pixel_frames': pf, 'height': 720}
        return probe_of

    def test_budget_skips_and_goes_on(self):
        specs = {i: ('h264', 10, 40) for i in range(10)}
        es = [entry(i) for i in range(10)]
        order = sorted(es, key=lambda e: tc.salted(e['sha256']))
        # The third file in order is too big for what is left; a later small one still fits.
        big = order[2]['sha256']
        specs = {i: ('h264', 10, 90 if entry(i)['sha256'] == big else 40) for i in range(10)}
        chosen, apart, rest = tc.select_class(es, self.probes(specs), 'h264', max_files=3, budget=130)
        self.assertEqual([e['sha256'] for e, _ in chosen], [order[0]['sha256'], order[1]['sha256'],
                                                              order[3]['sha256']])
        self.assertEqual(apart['over budget'], 1)
        self.assertEqual([e['sha256'] for e, _ in rest], [big])  # probing stops at max_files

    def test_reasons_and_other_classes(self):
        specs = {0: ('h264', 121, 1), 1: ('hevc', 10, 1), 2: ('mpeg4', 10, 1), 3: (None, 0, 0), 4: ('h264', 120, 1),
                 5: ('h264', 0.5, 1)}
        es = [entry(i) for i in specs]
        chosen, apart, _ = tc.select_class(es, self.probes(specs), 'h264')
        self.assertEqual(len(chosen), 2)            # 120 s is in, 121 s is out
        self.assertEqual(apart['video longer than 120 s'], 1)
        self.assertEqual(apart['not_allowlisted:mpeg4'], 1)
        self.assertEqual(apart['probe_failed'], 1)
        self.assertNotIn('hevc', str(apart))        # counted in its own class
        chosen, apart, _ = tc.select_class(es, self.probes(specs), 'h264', min_seconds=1)
        self.assertEqual(len(chosen), 1)
        self.assertEqual(apart['video shorter than 1 s'], 1)

    def test_probe_rest_collects_every_eligible_file(self):
        specs = {i: ('vp9', 10, 10) for i in range(8)}
        es = [entry(i) for i in range(8)]
        chosen, _, rest = tc.select_class(es, self.probes(specs), 'vp9', max_files=3, budget=1e9, probe_rest=True)
        self.assertEqual((len(chosen), len(rest)), (3, 5))


class Timestamps(unittest.TestCase):
    def test_same_time_base_is_exact(self):
        r = tc.compare_frames('1/12800', [512, 1024, 1536], '1/12800', [0, 512, 1024])
        self.assertTrue(r['ok'])
        r = tc.compare_frames('1/12800', [512, 1024, 1536], '1/12800', [0, 512, 1025])
        self.assertFalse(r['ok'])

    def test_repeated_times_are_expected_moved(self):
        # WebM blocks whose millisecond times repeat: each repeat one tick after the frame before, while
        # that is below the next source time.
        self.assertEqual(tc.moved_repeats([9580, 9580, 9580, 9929]), ([9580, 9581, 9582, 9929], 2, 0))
        self.assertEqual(tc.moved_repeats([0, 1, 1, 2, 3, 3, 4]), ([0, 1, 2, 3, 4], 0, 2))
        self.assertEqual(tc.moved_repeats([0, 10, 10, 10]), ([0, 10, 11, 12], 2, 0))
        # A time below the one before is not moved: the counts or the times differ.
        self.assertEqual(tc.moved_repeats([0, 5, 3]), ([0, 5, 3], 0, 0))
        r = tc.compare_frames('1/1000', [7863, 9580, 9580, 9929], '1/1000', [0, 1717, 1718, 2066])
        self.assertTrue(r['ok'])
        self.assertEqual((r['moved'], r['not_expected']), (1, 0))
        r = tc.compare_frames('1/1000', [7863, 9580, 9580, 9929], '1/1000', [0, 1717, 1717, 2066])
        self.assertFalse(r['ok'])

    def test_other_time_base_within_half_a_tick(self):
        # Source 1/1000, output 1/90000: half a source tick is 45 output ticks.
        src = [0, 33, 67, 100]
        ok = [0, 33 * 90 + 45, 67 * 90 - 45, 9000]
        self.assertTrue(tc.compare_frames('1/1000', src, '1/90000', ok)['ok'])
        bad = [0, 33 * 90 + 46, 67 * 90, 9000]
        r = tc.compare_frames('1/1000', src, '1/90000', bad)
        self.assertFalse(r['ok'])
        self.assertEqual(r['worst'], '23/45000')
        self.assertEqual(r['worst_frame'], 1)

    def test_counts_and_missing(self):
        self.assertEqual(tc.compare_frames('1/25', [0, 1], '1/25', [0])['reason'], 'frame counts differ')
        self.assertFalse(tc.compare_frames('1/25', [0, None], '1/25', [0, 1])['ok'])
        self.assertFalse(tc.compare_frames('1/25', [], '1/25', [])['ok'])

    def test_av_offset(self):
        f = Fraction
        self.assertTrue(tc.compare_offset(f(1, 2), f(0), f(1, 2) + f(1, 1000), f(0))['ok'])
        self.assertFalse(tc.compare_offset(f(1, 2), f(0), f(1, 2) + f(1001, 1000000), f(0))['ok'])
        self.assertFalse(tc.compare_offset(None, f(0), f(0), f(0))['ok'])

    def test_audio(self):
        src = ('1/44100', [{'pts': -1024, 'dts': -1024, 'duration': 1024, 'data_hash': 'MD5:a'},
                           {'pts': 0, 'dts': 0, 'duration': 1024, 'data_hash': 'MD5:b'}])
        out = ('1/44100', [dict(p) for p in src[1]])
        remux = ('1/88200', [{'pts': -2048, 'dts': -2048, 'duration': 2048}, {'pts': 0, 'dts': 0, 'duration': 2048}])
        self.assertTrue(tc.compare_audio(src, out, remux)['ok'])
        out[1][1]['data_hash'] = 'MD5:c'
        self.assertEqual(tc.compare_audio(src, out, remux)['reason'], 'packet data differs')
        out[1][1]['data_hash'] = 'MD5:b'
        remux[1][1]['duration'] = 2047
        r = tc.compare_audio(src, out, remux)
        self.assertEqual((r['reason'], r['packet']), ('duration differs from coreav remux', 1))
        self.assertFalse(tc.compare_audio(src, out, None)['ok'])


class NotCarried(unittest.TestCase):
    def test_only_codec_refusals_count(self):
        u = 'unsupported_feature'
        for message in ('unsupported: MP4 has no standard sample entry for Vorbis',
                        'unsupported: PCM audio is not copied to MP4',
                        'unsupported: AC-3 and E-AC-3 are not copied to MP4 by this remux',
                        'unsupported: audio other than AAC, MP3 or Opus in their MP4 sample entries'):
            self.assertTrue(tc.carried_none({'kind': u, 'message': message}), message)
        # A refusal for any other reason, or another kind, is a failure of the remux.
        self.assertFalse(tc.carried_none({'kind': u, 'message': 'unsupported: the edit list has more than one edit'}))
        self.assertFalse(tc.carried_none({'kind': u, 'message': 'no stream 3'}))
        self.assertFalse(tc.carried_none({'kind': 'invalid_structure', 'message': 'no standard sample entry'}))


class Parsing(unittest.TestCase):
    LOG = '\n'.join([
        '[Parsed_psnr_4 @ 0x1] PSNR y:42.026779 u:39.307836 v:39.058789 average:40.859132 min:39.818440 max:45.17',
        '[out#0/null @ 0x2]   Output stream #0:0 (video): 40 frames encoded; 40 packets muxed (17600 bytes); ',
        '[out#1/null @ 0x3]   Output stream #1:0 (video): 50 frames encoded; 50 packets muxed (22000 bytes); ',
        '[out#2/null @ 0x4]   Output stream #2:0 (video): 40 frames encoded; 40 packets muxed (17600 bytes); '])

    def test_psnr(self):
        self.assertEqual(tc.parse_psnr(self.LOG), (40.859132, [40, 50, 40]))
        self.assertEqual(tc.parse_psnr('PSNR y:inf u:inf v:inf average:inf min:inf max:inf')[0], float('inf'))
        self.assertEqual(tc.parse_psnr('nothing'), (None, [None, None, None]))

    def test_error_object(self):
        r = mock.Mock(stderr=b'warning\n{"schema":"coreav.error/1","kind":"unsupported"}\n', returncode=1)
        self.assertEqual(tc.error_of(r)['kind'], 'unsupported')
        r = mock.Mock(stderr=b'panic', returncode=101)
        self.assertEqual(tc.error_of(r), {'raw': 'panic', 'exit': 101})

    def test_arms(self):
        p = argparse.ArgumentParser()
        arms = tc.parse_arms(['x264=--encoder x264 --x264 "/a b/x264"'], p)
        self.assertEqual(arms, [('x264', ['--encoder', 'x264', '--x264', '/a b/x264'])])
        with self.assertRaises(SystemExit), mock.patch('sys.stderr'):
            tc.parse_arms(['baseline=--encoder ffmpeg'], p)


class Statistics(unittest.TestCase):
    def test_bootstrap(self):
        self.assertEqual(tc.bootstrap([1.0] * 7), (1.0, 1.0, 1.0))
        vals = [0.9, 1.0, 1.02, 1.03, 1.1, 1.2]
        a, b = tc.bootstrap(vals), tc.bootstrap(list(vals))
        self.assertEqual(a, b)
        self.assertLessEqual(a[1], a[0])
        self.assertLessEqual(a[0], a[2])
        self.assertEqual(tc.bootstrap([]), (None, None, None))

    def test_rotation(self):
        names = ['a', 'b', 'baseline']
        self.assertEqual([ts.rotation(names, r) for r in range(3)],
                         [['a', 'b', 'baseline'], ['b', 'baseline', 'a'], ['baseline', 'a', 'b']])


class SpeedRun(unittest.TestCase):
    """The top-up and the gate of transcode_speed.run, with the timing replaced."""

    def run_with(self, statuses, gates, files=2, reserve=6, not_uploaded=0):
        d = tempfile.mkdtemp()
        rows = lambda n, tag: [{'sha256': f'{tag}{i:063d}', 'ext': 'mp4', 'pixel_frames': 1, 'seconds': 2}
                               for i in range(n)]
        sel = {'params': {}, 'classes': {'vp8': {'files': rows(files, 'f'), 'reserve': rows(reserve, 'r'),
                                                 'apart': {}, 'reserve_not_uploaded': not_uploaded}}}
        pathlib.Path(d, 'sel.json').write_text(json.dumps(sel))
        for f in sel['classes']['vp8']['files'] + sel['classes']['vp8']['reserve']:
            pathlib.Path(d, f'{f["sha256"]}.mp4').write_bytes(b'')
        it = iter(statuses)

        def fake_time(args, f, path, tmp):
            s = next(it)
            ratio = {'timed': 1.0, 'arm_failed': float('inf')}.get(s)
            r = {'sha256': f['sha256'], 'status': 'timed' if ratio else s, 'ratios': {'a': ratio} if ratio else {},
                 'median_seconds': {'a': 1.0}, 'information_seconds': 1.0}
            if s == 'arm_failed':
                r['failed'] = {'a': {'exit': 101}}
            return r
        g = iter(gates)
        args = argparse.Namespace(selection=os.path.join(d, 'sel.json'), files=d, out=os.path.join(d, 'out'),
                                  tmp=os.path.join(d, 'tmp'), classes=['vp8'], arms=[('a', [])], ffmpeg='ffmpeg',
                                  ffprobe='ffprobe', coreav='coreav', gate='g', decode_oracle=None,
                                  preflight=None, allow_non_tmpfs=False)
        with mock.patch.object(ts, 'time_file', fake_time), mock.patch.object(ts, 'preflight', lambda a, t: None), \
                mock.patch.object(ts, 'gate', lambda a, t, w: {'which': w, 'times': [], 'median': next(g)}), \
                mock.patch.object(ts, 'host', lambda a: {'tmp_filesystem': 'tmpfs'}), mock.patch('builtins.print'):
            rc = ts.run(args)
        rep = json.loads(pathlib.Path(d, 'out', 'summary.json').read_text())
        return rc, rep

    def test_topup_until_five_timed(self):
        rc, rep = self.run_with(['timed', 'refused', 'timed', 'baseline_failed', 'timed', 'timed', 'timed',
                                 'timed'], [1.0, 1.03])
        self.assertEqual(rc, 0)
        v = rep['arms']['a']['vp8']
        self.assertEqual((v['timed'], v['topup']), (5, 4))  # 1 of the 2 selected, 4 of 5 reserve
        self.assertEqual(v['apart'], {'refused: None': 1, 'baseline_failed: None': 1})

    def test_failed_arm_is_judged_at_infinity(self):
        # A failure of the arm is not replaced by the top-up: it is a timed file whose ratio is +inf.
        rc, rep = self.run_with(['timed', 'arm_failed', 'arm_failed', 'arm_failed', 'timed', 'timed'],
                                [1.0, 1.0], files=2, reserve=6)
        v = rep['arms']['a']['vp8']
        self.assertEqual((v['timed'], v['topup'], v['failed']), (5, 3, 3))
        self.assertEqual(v['median_ratio'], float('inf'))
        self.assertFalse(v['met'])
        self.assertEqual(rc, 1)

    def test_reserve_used_up(self):
        rc, rep = self.run_with(['timed', 'refused', 'timed'], [1.0, 1.0], files=2, reserve=1)
        v = rep['arms']['a']['vp8']
        self.assertEqual(v['timed'], 2)
        self.assertTrue(v['reserve_used_up'])
        self.assertNotIn('topup_not_honoured', v)
        rc, rep = self.run_with(['timed', 'refused', 'timed'], [1.0, 1.0], files=2, reserve=1, not_uploaded=4)
        self.assertEqual(rep['arms']['a']['vp8']['topup_not_honoured'], {'eligible_files_not_here': 4})
        self.assertEqual(rc, 1)

    def test_gate_drift_voids(self):
        rc, rep = self.run_with(['timed'] * 5, [1.0, 1.051], files=5, reserve=0)
        self.assertEqual(rc, 3)
        self.assertTrue(rep['void'])


class Layout(unittest.TestCase):
    def test_encoded_stream_keeps_its_place(self):
        summary = {'video': {'stream_index': 2}, 'audio': [{'stream_index': 1}, {'stream_index': 4}],
                   'other': [{'stream_index': 0, 'kind': 'video'}, {'stream_index': 3, 'kind': 'subtitle'}]}
        self.assertEqual(tc.output_layout(summary), {'video': 2, 'audio': [1, 4], 'videos': 2, 'streams': 5})
        self.assertEqual(tc.output_layout({'video': {'stream_index': 0}, 'audio': []}),
                         {'video': 0, 'audio': [], 'videos': 1, 'streams': 1})
        self.assertEqual(tc.baseline_layout([3, 5]), {'video': 0, 'audio': [1, 2], 'videos': 1, 'streams': 3})

    def test_attached_picture_is_no_track(self):
        # A copied `udta` with a cover image: ffprobe lists the image as a second video stream.
        layout = {'video': 0, 'audio': [], 'videos': 1, 'streams': 1}
        h264 = {'index': 0, 'codec_type': 'video', 'codec_name': 'h264', 'disposition': {'attached_pic': 0}}
        cover = {'index': 1, 'codec_type': 'video', 'codec_name': 'mjpeg', 'disposition': {'attached_pic': 1}}
        self.assertTrue(tc.streams_read([h264, cover], layout, 0))
        self.assertTrue(tc.streams_read([h264], layout, 0))
        # A second video track is still one too many, and a cover image still has to be read.
        self.assertFalse(tc.streams_read([h264, dict(cover, disposition={'attached_pic': 0})], layout, 0))
        self.assertFalse(tc.streams_read([h264, dict(cover, codec_name=None)], layout, 0))
        # Audio beside the cover image keeps its place.
        aac = {'index': 2, 'codec_type': 'audio', 'codec_name': 'aac', 'disposition': {'attached_pic': 0}}
        with_audio = {'video': 0, 'audio': [2], 'videos': 1, 'streams': 2}
        self.assertTrue(tc.streams_read([h264, cover, aac], with_audio, 1))
        self.assertFalse(tc.streams_read([h264, cover, dict(aac, index=1), dict(cover, index=2)], with_audio, 1))
        # The encoded stream is never the cover image.
        self.assertFalse(tc.streams_read([dict(h264, disposition={'attached_pic': 1})], layout, 0))

    def test_misconfigured_arm_is_no_refusal(self):
        self.assertTrue(tc.misconfigured({'message': 'unsupported: no x264 program: install one, or name it'}))
        self.assertTrue(tc.misconfigured({'message': 'no encoder program: install x264 or ffmpeg with libx264'}))
        self.assertFalse(tc.misconfigured({'message': 'the encoder takes planar 4:2:0, 4:2:2 or 4:4:4 at 8 or 10 '
                                                      'bits, not Gray'}))


class Summary(unittest.TestCase):
    def test_missing_results_give_no_verdict(self):
        d = tempfile.mkdtemp()
        r = {'config': 'default', 'class': 'h264', 'file': 'a', 'complete': True,
             'arms': {'x': {'status': 'refused', 'reason': 'unsupported_feature', 'error': {'message': 'm'}}}}
        tc._write(tc.result_path(d, 'default', 'a'), r)
        tc._write(tc.result_path(d, 'default', 'b'), {'config': 'default', 'class': 'h264', 'complete': False})
        rep = tc.summarize(d, False, {'default': {'a': 'h264', 'b': 'h264', 'c': 'h264'}}, ['x'])
        self.assertEqual(rep['missing'], {'default': {'h264': 2, 'all': 2}})
        v = rep['configs']['default']['x']['h264']
        self.assertEqual(v['missing'], 2)
        self.assertIsNone(v['B2']['met'])
        with mock.patch('builtins.print'):
            self.assertEqual(tc.verdict_status(rep), 1)

    def test_b9_evidence_has_no_absolute_paths(self):
        d = pathlib.Path(tempfile.mkdtemp())
        fixtures, out, tools = d / 'fixtures', d / 'out', d / 'tools'
        for p in (fixtures, tools):
            p.mkdir()
        (fixtures / 'a.mp4').write_bytes(b'a')
        stderr = f'coreav ffmpeg: {fixtures}/a.mp4: truncated structure at offset 1'
        r = {'config': 'x01', 'class': 'h264', 'file': 'a.mp4', 'complete': True, 'command': '-i IN',
             'baseline': {'status': 'ok', 'exit': 0},
             'arms': {'x264': {'status': 'refused', 'exit': 1, 'reason': 'rejected: ' + stderr, 'stderr': stderr}}}
        tc._write(tc.result_path(out, 'x01', 'a.mp4'), r)
        tc._write(out / 'run.json', {'arms': {'x264': [['COREAV_X264', f'{tools}/x264']]}})
        tc._write(out / 'summary.json', {})
        args = argparse.Namespace(fixtures=str(fixtures), out=str(out), tmp=None, coreav=f'{tools}/coreav',
                                  ffmpeg=f'{tools}/ffmpeg', ffprobe=f'{tools}/ffprobe',
                                  arms=[('x264', [('COREAV_X264', f'{tools}/x264')])])
        tc.export_evidence(args, out, {'tools': {}, 'ffmpeg_version': []})
        ev = out / 'evidence'
        rows = [json.loads(line) for line in (ev / 'calls.jsonl').read_text().splitlines()]
        self.assertEqual([(x['arm'], x['status']) for x in rows], [('baseline', 'ok'), ('x264', 'refused')])
        self.assertEqual(rows[1]['reason'], 'rejected: coreav ffmpeg: a.mp4: truncated structure at offset 1')
        manifest = json.loads((ev / 'manifest.json').read_text())
        self.assertEqual(manifest['arms'], {'x264': ['COREAV_X264=x264']})
        self.assertEqual(list(manifest['fixtures']), ['a.mp4'])
        with tarfile.open(ev / 'results.tar.gz') as tar:
            text = ''.join(tar.extractfile(m).read().decode() for m in tar.getmembers())
        self.assertNotIn(str(d), text)
        self.assertNotIn(str(d), (ev / 'calls.jsonl').read_text() + (ev / 'manifest.json').read_text())

    def test_b9_run_without_a_baseline_misses(self):
        # B9 compares a command that ran with FFmpeg running it: a failed baseline leaves Q1 and Q2 missed.
        ok = {'ok': True}
        r = {'config': 'x01', 'class': 'h264', 'file': 'a.mp4', 'command': '-i IN', 'has_audio': False,
             'baseline': {'status': 'failed', 'exit': 1},
             'arms': {'x264': {'status': 'ok', 'B1': ok, 'B2': ok,
                               'Q1': {'ok': False, 'reason': 'no baseline or no output size'},
                               'Q2': {'ok': False, 'reason': 'no baseline or no output size'}}}}
        v = tc.class_verdict([r], 'x264', 1.0, True, compat=True)
        self.assertEqual((v['B1']['met'], v['Q1']['met'], v['Q2']['met']), (True, False, False))
        self.assertEqual(v['apart'], {})


if __name__ == '__main__':
    unittest.main()

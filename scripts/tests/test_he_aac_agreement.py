"""The HE-AAC harness (scripts/he_aac_agreement.py) without media: the
set, the signalling it reads, and the verdicts of each bar on made-up rows."""
import argparse
import contextlib
import io
import json
import pathlib
import struct
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

ROOT = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts'))
import decode_audio_check as dc  # noqa: E402
import he_aac_agreement as h  # noqa: E402


def row(**kw):
    """A decoder test row of a clean stereo input of 10 packets."""
    r = {'status': 'decoded', 'packets': 10, 'frame': 2048, 'length': 20480, 'damaged': 0, 'rate': 48000,
         'channels': 2, 'mask': 3, 'pce': False, 'reference_length': 20000, 'reference_values': 40000,
         'noise_frames': 0, 'intensity_all_ms_frames': 0, 'sha256': 'a',
         'per_channel': [{'rms': 1e-7, 'max': 1e-6, 'noise_db': 0.0, 'noise_blocks': 0, 'noise_block_db': None}] * 2}
    r.update(kw)
    return r


def ref(**kw):
    """The oracle's statement of the same input."""
    o = {'returncode': 0, 'warnings': False, 'stderr': '', 'rate': 48000, 'channels': 2, 'mask': 3}
    o.update(kw.pop('oracle', {}))
    return {'key': 'gen-he-tones-48000-stereo', 'set': 'generated', 'stated_samples': 20000, 'oracle': o, **kw}


class Set(unittest.TestCase):
    def test_the_generated_set_is_the_one_the_design_note_names(self):
        plan = h.encodes()
        groups = {}
        for e in plan:
            groups[e['group']] = groups.get(e['group'], 0) + 1
        self.assertEqual(groups, {'he-aac': 24, 'he-aac-v2': 12, 'rates': 8, 'layouts': 1})
        self.assertEqual(len({e['name'] for e in plan}), 45)
        self.assertTrue(all(e['layout'] == 'stereo' for e in plan if e['codec'] == 'aacp'))

    def test_the_corpus_rule_keeps_only_files_not_held_out_in_read_containers(self):
        self.assertTrue(h.candidate({'holdout': False, 'ext': 'm4a'}))
        self.assertTrue(h.candidate({'holdout': False, 'ext': 'mkv'}))
        for r in ({'holdout': True, 'ext': 'mp4'}, {'ext': 'mp4'}, {'holdout': False, 'ext': 'ts'},
                  {'holdout': False, 'ext': 'aac'}, {'holdout': False, 'ext': 'webm'}):
            self.assertFalse(h.candidate(r), r)


class Signalling(unittest.TestCase):
    """Configurations of the conformance package and of Apple's encoder."""

    def check(self, hex_asc, **want):
        got = h.signalling(bytes.fromhex(hex_asc))
        self.assertEqual({k: got[k] for k in want}, want, hex_asc)
        return got

    def test_implicit(self):
        self.check('1310', sbr='none', core_rate=24000, frame=1024, downsampled=False)
        self.check('140c', sbr='none', core_rate=16000, frame=960)

    def test_backward_compatible(self):
        self.check('131056e598', sbr='compatible', core_rate=24000, extension_rate=48000, ps=False)
        self.check('130856e59d4880', sbr='compatible', extension_rate=48000, ps=True)

    def test_backward_compatible_after_a_program_configuration(self):
        self.check('140006040000000056e5a8', sbr='compatible', channel_configuration=0, extension_rate=32000)
        self.check('1300058c01000108800056e598', sbr='compatible', channel_configuration=0, extension_rate=48000)

    def test_hierarchical(self):
        self.check('2b118800', sbr='hierarchical', object_type=5, core_rate=24000, extension_rate=48000,
                   downsampled=False)
        self.check('2b030802c200001000', sbr='hierarchical', core_rate=24000, extension_rate=24000, downsampled=True)

    def test_downsampled_sbr_and_960_have_no_implicit_variant(self):
        self.assertFalse(h.has_implicit_variant(self.check('141056e5c0', downsampled=True)))
        self.assertFalse(h.has_implicit_variant(h.signalling(bytes.fromhex('140c'))))
        self.assertTrue(h.has_implicit_variant(h.signalling(bytes.fromhex('131056e598'))))


class Files(unittest.TestCase):
    def test_wav_format_reads_masks_and_defaults(self):
        plain = b'RIFF\0\0\0\0WAVE' + b'fmt ' + struct.pack('<IHHIIHH', 16, 1, 2, 44100, 0, 4, 16)
        ext = b'RIFF\0\0\0\0WAVE' + b'fmt ' + struct.pack('<IHHIIHHHHI', 40, 0xfffe, 6, 48000, 0, 24, 32, 22, 32,
                                                          0x3f) + b'\0' * 16
        self.assertEqual(h.wav_format(plain), (44100, 2, 3))
        self.assertEqual(h.wav_format(ext), (48000, 6, 0x3f))

    def test_references_follow_the_package_names(self):
        with tempfile.TemporaryDirectory() as tmp:
            conf = pathlib.Path(tmp)
            folder = conf / 'referencesWav' / 'add-opt'
            folder.mkdir(parents=True)
            for name in ('al_sbr_hq_e_32_1', 'al_sbr_lp_e_32_1', 'al_sbr_hq_cm_48_5_f00', 'al_sbr_hq_cm_48_5_f01',
                         'al_sbr_hq_cm_48_5.1_f00', 'al_sbr_ps_03_ur', 'al_sbr_ps_03_bl'):
                (folder / f'{name}.wav').write_bytes(b'')
            names = lambda s: [p.stem for p in h.references(conf, s)]  # noqa: E731
            self.assertEqual(names('al_sbr_e_32_1'), ['al_sbr_hq_e_32_1'])
            self.assertEqual(names('al_sbr_cm_48_5'), ['al_sbr_hq_cm_48_5_f00', 'al_sbr_hq_cm_48_5_f01'])
            self.assertEqual(names('al_sbr_ps_03'), ['al_sbr_ps_03_ur'])
            self.assertEqual(names('al_sbr_ps_03_new'), ['al_sbr_ps_03_ur'])
            self.assertEqual(names('al960_sbr_e_32_1'), [])


def item(key, variant, of=None, container='mp4', set='generated', **kw):
    """One input of set.json; its path is its key unless given."""
    return {**ref(), 'key': key, 'set': set, 'path': key, 'variant': variant, 'of': of, 'references': [],
            'container': container, **kw}


def done(args, code=0, out=b'', err=b''):
    return subprocess.CompletedProcess([str(a) for a in args], code, out, err)


def wav(samples, channels=2, rate=48000):
    data = struct.pack(f'<{len(samples)}f', *samples)
    fmt = struct.pack('<HHIIHH', 3, channels, rate, rate * 4 * channels, 4 * channels, 32)
    return (b'RIFF' + struct.pack('<I', 4 + 8 + len(fmt) + 8 + len(data)) + b'WAVE' + b'fmt ' + struct.pack('<I', len(fmt))
            + fmt + b'data' + struct.pack('<I', len(data)) + data)


class Conversions(unittest.TestCase):
    """A source, encode or derived file that ffmpeg or Apple's encoder does
    not make stops the run at its preparation, before any CoreAV command
    ("HE-AAC sets", "Conversions"); it never drops out of a bar."""

    def test_variants_reports_a_partial_failure(self):
        sig = h.signalling(bytes.fromhex('131056e598'))
        refuse = lambda args, **kw: done(args, 1 if str(args[-1]).endswith('.implicit.m4a') else 0, err=b'refused')  # noqa: E731
        with tempfile.TemporaryDirectory() as tmp, mock.patch.object(h, 'run', refuse):
            made, failed = h.variants(argparse.Namespace(ffmpeg='ffmpeg'), 'in.m4a', sig, pathlib.Path(tmp), 'k')
        self.assertEqual(sorted(made), ['mka'])
        self.assertEqual(failed, {'implicit': 'refused'})
        with tempfile.TemporaryDirectory() as tmp, mock.patch.object(h, 'run', lambda args, **kw: done(args)):
            _, failed = h.variants(argparse.Namespace(ffmpeg='ffmpeg'), 'in.m4a', None, pathlib.Path(tmp), 'k')
        self.assertEqual(failed, {'implicit': 'no AudioSpecificConfig'})

    def test_prepare_stops_when_a_variant_is_not_made(self):
        stated = {'codec': 'aac', 'profile': 'HE-AAC', 'packets': 10, 'stated_samples': 20480, 'asc': '131056e598'}
        made = {'returncode': 0, 'warnings': False, 'stderr': '', 'rate': 48000, 'channels': 2, 'mask': 3,
                'samples': 20480}
        refuse = lambda args, **kw: done(args, 1 if str(args[-1]).endswith('.implicit.m4a') else 0,  # noqa: E731
                                         out=b'ffmpeg version x\n', err=b'refused')
        with tempfile.TemporaryDirectory() as tmp:
            tmp = pathlib.Path(tmp)
            (tmp / 'conf' / 'compressedMp4').mkdir(parents=True)
            (tmp / 'conf' / 'referencesWav' / 'add-opt').mkdir(parents=True)
            (tmp / 'conf' / 'compressedMp4' / 'al_sbr_e_32_2.mp4').write_bytes(b'')
            (tmp / 'gen').mkdir()
            (tmp / 'gen' / 'encoded.json').write_text(json.dumps({'runner': {}, 'encodes': []}))
            with mock.patch.object(h, 'run', refuse), mock.patch.object(h, 'probe', lambda f, p: dict(stated)), \
                    mock.patch.object(h, 'oracle', lambda a, p, o: dict(made)), \
                    self.assertRaises(SystemExit) as stop:
                h.prepare(argparse.Namespace(ffmpeg='ffmpeg', ffprobe='ffprobe', work=tmp / 'w',
                                             conformance=tmp / 'conf', generated=tmp / 'gen'))
            meta = json.loads((tmp / 'w' / 'set.json').read_text())
            self.assertFalse((tmp / 'w' / 'agreement.tsv').exists())
        key = 'conf-al_sbr_e_32_2'
        self.assertIn('stops at its preparation', str(stop.exception))
        self.assertEqual(meta['not_made'], {key + '.implicit': 'refused'})
        self.assertEqual(sorted(r['key'] for r in meta['inputs']), [key, key + '.mka'])

    def test_encode_stops_when_the_encoder_refuses(self):
        def afconvert(args, **kw):
            args = [str(a) for a in args]
            if args[0] == 'afconvert' and 'aacp' not in args:
                pathlib.Path(args[-1]).write_bytes(b'm4a')
                return done(args)
            return done(args, 1 if args[0] == 'afconvert' else 0, out=b'macOS 15', err=b'fmt?')
        plan = [e for e in h.encodes() if e['name'] in ('he-tones-48000-stereo', 'he2-tones-48000-stereo')]
        with tempfile.TemporaryDirectory() as tmp:
            tmp = pathlib.Path(tmp)
            (tmp / 'encodes.json').write_text(json.dumps({'encodes': plan, 'sources': {}}))
            with mock.patch.object(h, 'run', afconvert), contextlib.redirect_stdout(io.StringIO()), \
                    self.assertRaises(SystemExit) as stop:
                h.encode(argparse.Namespace(dir=tmp))
            encoded = json.loads((tmp / 'encoded.json').read_text())
        self.assertIn('he2-tones-48000-stereo', str(stop.exception))
        self.assertEqual([e['status'] for e in encoded['encodes']], ['encoded', 'not encoded'])


class Judge(unittest.TestCase):
    """The judge on a made-up run: which inputs each bar judges, and what bars
    2 and 5 report, from the commands it runs."""

    def run_judge(self, inputs, decoded, drop_result=False):
        calls = []

        def fake(args, timeout=None, **kw):
            args = [str(a) for a in args]
            calls.append(args)
            if 'decode_audio_check.py' in ' '.join(args):
                files = args[args.index('--out') + 2:]
                results = [{'status': 'ok', 'model': True, 'length': True, 'timeline': True, 'ffmpeg': 'equal'}
                           for _ in files][drop_result:]
                pathlib.Path(args[args.index('--out') + 1]).write_text(json.dumps({'results': results}))
                return done(args)
            if args[1] == 'decode-audio' and '--threads' in args:
                return done(args, out=b'same')
            if args[1] == 'decode-audio':
                return done(args, out=wav([0.5, -0.5] * 30))
            return done(args, out=struct.pack('<40f', *([0.5, -0.5] * 20)))

        with tempfile.TemporaryDirectory() as tmp:
            work = pathlib.Path(tmp)
            (work / 'set.json').write_text(json.dumps({'ffmpeg': 'f', 'encoder': {}, 'not_encoded': [],
                                                       'inputs': inputs}))
            (work / 'decoder.jsonl').write_text(''.join(json.dumps({'input': k, **v}) + '\n' for k, v in decoded.items()))
            a = argparse.Namespace(coreav='coreav', ffmpeg='ffmpeg', ffprobe='ffprobe', work=work, out=work / 'o.json')
            with mock.patch.object(h, 'run', fake), contextlib.redirect_stdout(io.StringIO()):
                code = h.judge(a)
            return code, json.loads(a.out.read_text()), calls

    def inputs(self):
        return [item('conf-x', 'mp4', set='conformance'), item('conf-x.mka', 'mka', 'conf-x', container='matroska',
                                                                set='conformance'),
                item('g', 'mp4'), item('g.mka', 'mka', 'g', container='matroska'), item('g.implicit', 'implicit', 'g'),
                item('c', 'matroska', container='matroska', set='corpus'), item('c.copy', 'copy', 'c', set='corpus'),
                item('m', 'mp4', set='corpus')]

    def test_bars_3_and_6_judge_every_input_the_bars_name(self):
        inputs = self.inputs()
        code, out, calls = self.run_judge(inputs, {r['path']: row(level_blocks=[]) for r in inputs})
        check = next(c for c in calls if 'decode_audio_check.py' in ' '.join(c))
        self.assertEqual(check[check.index('--out') + 2:], ['g', 'g.implicit', 'c.copy', 'm'])
        self.assertEqual(check[check.index('--tolerance') + 1:check.index('--tolerance') + 3],
                         [str(h.MAX_RMS), str(h.MAX_ABS)])
        self.assertEqual([r['input'] for r in out['bar3']['results']], ['g', 'g.implicit', 'c.copy', 'm'])
        self.assertEqual([r['input'] for r in out['bar6']['results']], ['g', 'g.mka', 'g.implicit', 'c', 'c.copy', 'm'])
        self.assertEqual((code, out['verdict']), (1, 'missed'))

    def test_a_missing_result_fails_bar_3(self):
        inputs = self.inputs()
        _, out, _ = self.run_judge(inputs, {r['path']: row(level_blocks=[]) for r in inputs}, drop_result=True)
        self.assertEqual([(r['input'], r['pass'], r.get('reason')) for r in out['bar3']['results']],
                         [(k, False, 'no result') for k in ('g', 'g.implicit', 'c.copy', 'm')])

    def test_the_corpus_copies_gate_but_stay_out_of_the_reported_numbers(self):
        inputs = self.inputs()
        decoded = {r['path']: row(level_blocks=[]) for r in inputs}
        wide = [{'rms': 1e-5, 'max': 4e-4, 'noise_db': 0.0, 'noise_blocks': 5}] * 2
        decoded['c.copy'] = row(level_blocks=[], per_channel=wide)
        _, out, _ = self.run_judge(inputs, decoded)
        self.assertEqual([r['verdict'] for r in out['bar2']['results'] if r['input'] == 'c.copy'], ['pass'])
        self.assertEqual((out['bar2']['largest_difference'], out['bar2']['largest_rms']), (1e-6, 1e-7))
        self.assertEqual(out['bar2']['largest_noise_share'], 0.0)
        self.assertEqual(out['bar3']['ffmpeg_default'], {'equal': 3, 'covered': 0, 'uncovered': 0})
        counts = {bar: (out[bar]['counted'], out[bar]['passed'], out[bar].get('corpus_mp4_copies'))
                  for bar in ('bar2', 'bar3', 'bar6')}
        self.assertEqual(counts, {'bar2': (7, 7, {'counted': 1, 'passed': 1}),
                                  'bar3': (3, 3, {'counted': 1, 'passed': 1}),
                                  'bar6': (5, 5, {'counted': 1, 'passed': 1})})
        decoded['c.copy'] = row(level_blocks=[], damaged=1)
        _, out, _ = self.run_judge(inputs, decoded)
        self.assertEqual([r['verdict'] for r in out['bar2']['results'] if r['input'] == 'c.copy'], ['FAIL: damaged frames'])
        self.assertEqual((out['bar2']['counted'], out['bar2']['passed']), (7, 7))
        self.assertEqual(out['bar2']['corpus_mp4_copies'], {'counted': 1, 'passed': 0})

    def test_a_failing_copy_alone_misses_the_bar(self):
        full = {f'bar{n}': {'counted': 2, 'passed': 2} for n in range(1, 7)}
        self.assertEqual(h.verdict(full | {'bar6': {'counted': 2, 'passed': 2,
                                                    'corpus_mp4_copies': {'counted': 1, 'passed': 1}}}), 'met')
        self.assertEqual(h.verdict(full | {'bar6': {'counted': 2, 'passed': 2,
                                                    'corpus_mp4_copies': {'counted': 1, 'passed': 0}}}), 'missed')

    def test_bar5_reports_decode_audio_against_ffmpeg_with_both_lengths(self):
        inputs = self.inputs()
        _, out, calls = self.run_judge(inputs, {r['path']: row(level_blocks=[]) for r in inputs})
        for path in ('conf-x.mka', 'g.mka', 'c'):
            self.assertIn(['coreav', 'decode-audio', '--format', 'f32', path, '-'], calls)
            self.assertIn(['ffmpeg', '-v', 'error', '-threads:a', '1', '-i', path, '-map', '0:a:0', '-f', 'f32le', '-'],
                          calls)
        reported = {r['input']: r['reported'] for r in out['bar5']['results']}
        self.assertEqual(sorted(reported), ['c', 'conf-x.mka', 'g.mka'])
        for r in reported.values():
            self.assertEqual((r['coreav_length'], r['ffmpeg_length']), (30, 20))
            self.assertEqual([c['rms'] for c in r['per_channel']], [0.0, 0.0])
        self.assertIn('per_channel', out['bar2']['results'][0])
        self.assertIn('largest_noise_share', out['bar2'])

    def test_level_blocks_are_compared_by_level_only(self):
        mine = struct.pack('<8f', 1, 1, 1, 1, 0.5, 0.5, 0.5, 0.5)
        theirs = struct.pack('<8f', 1, 1, 1, 0.9, 0.25, 0.5, 0.25, 0.5)
        got = h.channel_differences(mine, theirs, 2, 2, {1})
        self.assertEqual([c['max'] for c in got], [0.0, 0.10000002384185791])
        self.assertAlmostEqual(got[0]['level_db'], 10 * __import__('math').log10(0.5 / 0.125))
        self.assertAlmostEqual(got[1]['level_db'], 0.0)


class AudioCheck(unittest.TestCase):
    """scripts/decode_audio_check.py's --tolerance holds each channel to the
    HE-AAC tolerance; without it every channel is pooled as before."""

    def six(self, diff, frames=1000):
        return struct.pack('<6f', *[0.5] * 6) * frames, struct.pack('<6f', 0.5 + diff, *[0.5] * 5) * frames

    def test_one_channel_over_the_bar_fails_when_the_pool_would_pass(self):
        a, b = self.six(3e-5)
        with mock.patch.object(dc, 'TOLERANCE', (h.MAX_RMS, h.MAX_ABS)):
            pooled, per = dc.Compare(True), dc.Compare(True, 6)
            pooled.add(a, b)
            per.add(a, b)
            self.assertTrue(pooled.ok())
            self.assertFalse(per.ok())
            ok = dc.Compare(True, 6)
            ok.add(*self.six(1e-5))
            self.assertTrue(ok.ok())

    def reference(self, out):
        full = struct.pack('<6f', *[0.5] * 6) * 1000
        return {'plan': None, 'full': full, 'frame': 24, 'lossy': True, 'ffmpeg': out, 'edges': list(range(0, 1001, 100)),
                'edits': [], 'first_dts': 0, 'rate': 1000, 'timescale': 1000, 'movie_timescale': 1000,
                'fragmented': False, 'total': 1000, 'ffmpeg_failed': False}

    def evaluate(self, out, ffmpeg):
        ref = self.reference(ffmpeg)
        return dc.verdicts(dc.in_memory(out, ref['full'], ffmpeg, ref), ref, dc.known_divergences(),
                           dc.timeline(ref['plan'], ref['total']))

    def test_the_override_applies_per_channel_and_the_default_stays_pooled(self):
        full, off = self.six(3e-5)
        self.assertEqual(self.evaluate(full, off)['ffmpeg'], 'equal')
        argv = ['decode_audio_check.py', '--coreav', 'coreav', '--tolerance', str(h.MAX_RMS), str(h.MAX_ABS), 'x.m4a']
        with mock.patch.object(dc, 'TOLERANCE', dc.TOLERANCE), mock.patch.object(dc, 'PER_CHANNEL', False), \
                mock.patch.object(sys, 'argv', argv), mock.patch.object(dc, 'check', lambda *x: {'status': 'not read'}), \
                contextlib.redirect_stdout(io.StringIO()):
            dc.main()
            self.assertEqual((dc.TOLERANCE, dc.PER_CHANNEL), ((h.MAX_RMS, h.MAX_ABS), True))
            self.assertEqual(self.evaluate(full, off)['ffmpeg'], 'uncovered')
            self.assertEqual(self.evaluate(full, full)['ffmpeg'], 'equal')

    def test_the_length_result(self):
        full, _ = self.six(0)
        self.assertTrue(self.evaluate(full, full)['length'])
        self.assertFalse(self.evaluate(full[:-24], full)['length'])
        self.assertFalse(self.evaluate(full + full[:24], full)['length'])


class Bars(unittest.TestCase):
    def test_bar2_passes_a_clean_input(self):
        self.assertEqual(h.bar2_verdict(row(), ref()), 'pass')

    def test_bar2_fails_samples_lengths_and_format(self):
        self.assertIn('samples', h.bar2_verdict(row(per_channel=[{'rms': 1e-7, 'max': 1e-3, 'noise_db': 0,
                                                                   'noise_blocks': 0}]), ref()))
        self.assertIn('lengths', h.bar2_verdict(row(length=20000), ref()))
        self.assertIn('ffmpeg length', h.bar2_verdict(row(), ref(stated_samples=30000)))
        self.assertIn('format', h.bar2_verdict(row(channels=1, reference_values=20000), ref()))
        self.assertIn('format', h.bar2_verdict(row(mask=4), ref()))
        self.assertEqual(h.bar2_verdict(row(mask=0, pce=True), ref()), 'pass')
        self.assertIn('damaged', h.bar2_verdict(row(damaged=1), ref()))
        self.assertEqual(h.bar2_verdict(None, ref()), 'missing')

    def test_tolerance_edges(self):
        edge = [{'rms': h.MAX_RMS, 'max': h.MAX_ABS, 'noise_db': 0, 'noise_blocks': 0}]
        self.assertEqual(h.bar2_verdict(row(per_channel=edge), ref()), 'pass')
        over = [{'rms': h.MAX_RMS * 1.001, 'max': 0, 'noise_db': 0, 'noise_blocks': 0}]
        self.assertIn('samples', h.bar2_verdict(row(per_channel=over), ref()))
        self.assertEqual((h.MAX_ABS, h.REF_ABS), (2.0 ** -11, 2.0 ** -12))

    def test_a_named_stream_is_waived_from_the_ffmpeg_comparison_only(self):
        bad = [{'rms': 1, 'max': 1, 'noise_db': 0, 'noise_blocks': 0}]
        for key in ('conf-al_sbr_i_32_1_new', 'conf-al960_sbr_ps_04.mka', 'conf-al_sbr_cm_48_5.implicit'):
            named = ref(key=key, set='conformance', oracle={'warnings': True}, stated_samples=1)
            self.assertEqual(h.bar2_verdict(row(per_channel=bad), named), 'pass (waived: samples not compared)')
            self.assertIn('damaged', h.bar2_verdict(row(per_channel=bad, damaged=2), named))
            self.assertIn('lengths', h.bar2_verdict(row(per_channel=bad, length=1), named))
            self.assertIn('format', h.bar2_verdict(row(per_channel=bad, rate=24000), named))
        self.assertEqual(len(h.WAIVED), 11)

    def test_only_mp4_inputs_get_the_mp4_readers_option(self):
        for path, opts in (('a.mka', []), ('a.mkv', []), ('a.m4a', ['-ignore_editlist', '1']),
                           ('a.mp4', ['-ignore_editlist', '1']), ('a.3gp', ['-ignore_editlist', '1'])):
            cmd = h.bar2_decode('ffmpeg', path, 'out')
            self.assertEqual(cmd[cmd.index('-y') + 1:cmd.index('-y') + 1 + len(opts)], opts, path)
            self.assertEqual('-ignore_editlist' in cmd, bool(opts), path)
            self.assertIn('+skip_manual', cmd)
            self.assertLess(cmd.index('+skip_manual'), cmd.index('-i'))

    def test_both_manifest_shapes_give_the_same_candidates(self):
        rows = [{'sha256': 'b', 'ext': 'm4a', 'key': 'x/b.m4a', 'holdout': False},
                {'sha256': 'a', 'ext': 'mkv', 'key': 'corpus/c/a.mkv', 'holdout': False},
                {'sha256': 'c', 'ext': 'aac', 'key': 'c.aac', 'holdout': False},
                {'sha256': 'd', 'ext': 'mp4', 'key': 'd.mp4', 'holdout': True}]
        want = [('b', 'm4a', 'corpus/c/x/b.m4a'), ('a', 'mkv', 'corpus/c/a.mkv')]
        self.assertEqual(h.manifest_candidates('c', rows), want)
        self.assertEqual(h.manifest_candidates('c', {'files': rows}), want)

    def test_a_container_class_the_selection_empties_fails_the_run(self):
        ok = [('mp4', True, True, True), ('matroska', True, True, False), ('matroska', True, True, True)]
        self.assertEqual(h.dropped_classes(ok), [])
        self.assertEqual(h.dropped_classes([('mp4', True, True, True), ('matroska', True, False, False)]), [])
        self.assertEqual(h.dropped_classes([('mp4', True, True, True), ('matroska', True, True, False),
                                            ('matroska', True, True, False)]), ['matroska'])
        self.assertEqual(h.dropped_classes([('mp4', False, False, False), ('matroska', True, True, True)]), ['mp4'])

    def test_no_other_input_is_waived(self):
        bad = [{'rms': 1e-2, 'max': 1e-1, 'noise_db': 0, 'noise_blocks': 0}]
        on = {'coreav': [1e-7, 1e-6], 'ffmpeg': [1e-2, 1e-1]}
        for r in (ref(oracle={'warnings': True}), ref(key='corpus-c001', set='corpus', oracle={'warnings': True}),
                  ref(key='conf-al_sbr_ps_04', set='conformance'), ref(key='conf-al_sbr_ps_04_new', set='conformance'),
                  ref(key='gen-al_sbr_cm_48_5', set='generated')):
            self.assertTrue(h.bar2_verdict(row(per_channel=bad, reference=on), r).startswith('FAIL'), r['key'])
        self.assertIn('ffmpeg error', h.bar2_verdict(row(), ref(oracle={'warnings': True})))

    def test_noise_level_rule(self):
        loud = [{'rms': 1, 'max': 1, 'noise_db': 0.5, 'noise_blocks': 200}]
        self.assertIn('noise level', h.bar2_verdict(row(per_channel=loud, noise_frames=3), ref()))
        few = [{'rms': 1e-7, 'max': 1e-6, 'noise_db': 0.5, 'noise_blocks': 20}]
        self.assertEqual(h.bar2_verdict(row(per_channel=few, noise_frames=3), ref()), 'pass')

    def test_bar1(self):
        self.assertEqual(h.bar1_verdict(row(reference={'coreav': [h.REF_RMS, h.REF_ABS]})), 'pass')
        self.assertIn('FAIL', h.bar1_verdict(row(reference={'coreav': [1e-7, h.REF_ABS * 2]})))
        self.assertIn('FAIL', h.bar1_verdict(row()))
        self.assertIn('FAIL', h.bar1_verdict({'status': 'refused', 'reason': 'HE-AAC (SBR)'}))

    def test_bar3_gates_length_and_timeline_always(self):
        self.assertTrue(h.bar3_pass({'status': 'ok', 'model': True, 'length': True, 'timeline': True}, True))
        self.assertFalse(h.bar3_pass({'status': 'ok', 'model': False, 'length': True, 'timeline': True}, True))
        self.assertTrue(h.bar3_pass({'status': 'ok', 'model': False, 'length': True, 'timeline': True}, False))
        self.assertFalse(h.bar3_pass({'status': 'ok', 'model': True, 'length': False, 'timeline': True}, False))
        self.assertFalse(h.bar3_pass({'status': 'ok', 'model': True, 'length': True, 'timeline': False}, False))

    def meta(self):
        return {'inputs': [
            item('g', 'mp4'), item('g.mka', 'mka', 'g', container='matroska'), item('g.implicit', 'implicit', 'g'),
            item('c', 'matroska', container='matroska', set='corpus'), item('c.copy', 'copy', 'c', set='corpus'),
            item('d', 'matroska', container='matroska', set='corpus')]}

    def test_signalling_and_matroska_pairs(self):
        meta = self.meta()
        decoded = {k: row() for k in ('g', 'g.mka', 'g.implicit', 'c', 'c.copy', 'd')}
        bar1, bar2, bar4, bar5 = h.judge_rows(meta, decoded)
        self.assertEqual(bar1, [])
        self.assertEqual([r['input'] for r in bar2], ['g', 'g.mka', 'g.implicit', 'c', 'c.copy', 'd'])
        self.assertEqual(bar4, [{'input': 'g.implicit', 'pass': True}])
        self.assertEqual([(r['input'], r['pass']) for r in bar5], [('g.mka', True), ('c', True), ('d', False)])
        self.assertEqual(bar5[-1]['reason'], 'no MP4 pair')
        decoded['g.implicit'] = row(sha256='b')
        decoded['c'] = row(rate=24000)
        _, _, bar4, bar5 = h.judge_rows(meta, decoded)
        self.assertFalse(bar4[0]['pass'])
        self.assertEqual([r['pass'] for r in bar5], [True, False, False])

    def test_implicit_variant_must_keep_ffmpegs_format(self):
        meta = self.meta()
        meta['inputs'][2]['oracle']['rate'] = 24000
        decoded = {k: row() for k in ('g', 'g.mka', 'g.implicit', 'c', 'c.copy', 'd')}
        _, _, bar4, _ = h.judge_rows(meta, decoded)
        self.assertFalse(bar4[0]['pass'])

    def test_bar2_reports_each_channel_and_the_share_in_noise_blocks(self):
        channels = [{'rms': 1e-7, 'max': 1e-6, 'noise_db': 0.01, 'noise_blocks': 3, 'noise_block_db': [0.0, 0.02, 0.01]},
                    {'rms': 2e-7, 'max': 3e-6, 'noise_db': 0.02, 'noise_blocks': 3, 'noise_block_db': [0.0, 0.01, 0.02]}]
        got = h.bar2_row(row(per_channel=channels, noise_frames=1), item('g', 'mp4'))
        self.assertEqual(got['per_channel'], channels)
        self.assertAlmostEqual(got['noise_share'], 0.3)
        self.assertEqual((got['rms'], got['max']), (2e-7, 3e-6))

    def test_an_empty_bar_is_a_miss(self):
        full = {f'bar{n}': {'counted': 2, 'passed': 2} for n in range(1, 7)}
        self.assertEqual(h.verdict(full), 'met')
        self.assertEqual(h.verdict(full | {'bar4': {'counted': 0, 'passed': 0}}), 'missed')
        self.assertEqual(h.verdict(full | {'bar6': {'counted': 2, 'passed': 1}}), 'missed')


if __name__ == '__main__':
    unittest.main()

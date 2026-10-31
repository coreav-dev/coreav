"""The HE-AAC harness (scripts/he_aac_agreement.py) without media: the
set, the signalling it reads, and the verdicts of each bar on made-up rows."""
import pathlib
import struct
import sys
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'scripts'))
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
        def item(key, variant, of=None, path=None, container='mp4', **kw):
            return {**ref(), 'key': key, 'set': 'generated', 'path': path or key, 'variant': variant, 'of': of,
                    'references': [], 'container': container, **kw}
        return {'inputs': [
            item('g', 'mp4'), item('g.mka', 'mka', 'g', container='matroska'), item('g.implicit', 'implicit', 'g'),
            item('c', 'matroska', container='matroska'), item('c.copy', 'copy', 'c'),
            item('d', 'matroska', container='matroska', copy_failed='error')]}

    def test_signalling_and_matroska_pairs(self):
        meta = self.meta()
        decoded = {k: row() for k in ('g', 'g.mka', 'g.implicit', 'c', 'c.copy', 'd')}
        bar1, bar2, bar4, bar5 = h.judge_rows(meta, decoded)
        self.assertEqual(bar1, [])
        self.assertEqual([r['input'] for r in bar2], ['g', 'g.mka', 'g.implicit', 'c', 'd'])
        self.assertEqual(bar4, [{'input': 'g.implicit', 'pass': True}])
        self.assertEqual([(r['input'], r['pass']) for r in bar5], [('g.mka', True), ('c', True), ('d', False)])
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

    def test_an_empty_bar_is_a_miss(self):
        full = {f'bar{n}': {'counted': 2, 'passed': 2} for n in range(1, 7)}
        self.assertEqual(h.verdict(full), 'met')
        self.assertEqual(h.verdict(full | {'bar4': {'counted': 0, 'passed': 0}}), 'missed')
        self.assertEqual(h.verdict(full | {'bar6': {'counted': 2, 'passed': 1}}), 'missed')


if __name__ == '__main__':
    unittest.main()

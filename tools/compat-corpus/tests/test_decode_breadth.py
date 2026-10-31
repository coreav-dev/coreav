"""decode_breadth.py: V1's codes, host limits, records and summary, with stub
ffprobe, ffmpeg and decode-oracle executables that the tests write (the fixture
run on the real binaries is test_decode_breadth_fixtures.py). No media."""
import argparse
import contextlib
import hashlib
import io
import json
import os
import pathlib
import signal
import stat
import sys
import tempfile
import threading
import time
import unittest
from unittest import mock

HERE = pathlib.Path(__file__).resolve().parent.parent
ROOT = HERE.parent.parent
sys.path.insert(0, str(HERE))

import decode_breadth  # noqa: E402

STUB = '''#!%(python)s
import json, os, subprocess, sys, time

tool = os.path.basename(sys.argv[0])
argv = sys.argv[1:]
path = argv[argv.index('-i') + 1] if tool == 'ffmpeg' else argv[-1]
spec = json.load(open(path + '.spec')).get({'ffprobe': 'probe', 'ffmpeg': 'reference', 'decode-oracle': 'coreav'}[tool], {})
with open(path + '.calls', 'a') as log:
    gconv = os.environ.get('GCONV_PATH')
    log.write(json.dumps({'tool': tool, 'argv': argv, 'env': sorted(os.environ),
                          'gconv': gconv, 'gconv_files': os.listdir(gconv) if gconv else None}) + chr(10))
if spec.get('child'):
    open(path + '.child', 'w').write(str(subprocess.Popen(['sleep', '30']).pid))
if spec.get('early_sleep'):
    time.sleep(spec['early_sleep'])
if tool == 'ffprobe':
    print(json.dumps({'streams': spec.get('streams', []), 'format': {'format_name': spec.get('format_name')}}))
elif tool == 'ffmpeg':
    print('#format: frame checksums')
    for i, md5 in enumerate(spec.get('md5', [])):
        print('0, %%d, %%d, 1, 100, %%s' %% (i, i, md5))
elif 'object' in spec:
    print(json.dumps(spec['object']))
sys.stdout.flush()
sys.stderr.write(spec.get('stderr', ''))
sys.stderr.flush()
if spec.get('sleep'):
    time.sleep(spec['sleep'])
if spec.get('signal'):
    os.kill(os.getpid(), spec['signal'])
sys.exit(spec.get('exit', 0))
'''

H264 = {'index': 0, 'codec_type': 'video', 'codec_name': 'h264', 'width': 64, 'height': 48,
        'pix_fmt': 'yuv420p', 'field_order': 'progressive'}
MP4 = 'mov,mp4,m4a,3gp,3g2,mj2'
MKV = 'matroska,webm'
MD5S = ['a', 'b', 'c', 'd']


def sha(name):
    return hashlib.sha256(name.encode()).hexdigest()


def ours(frames=MD5S, stream_index=0, outcome='ok', codec='h264', backend='native'):
    return {'stream_index': stream_index, 'codec': codec, 'backend': backend, 'decoder': 'stub',
            'frames': [{'pts': i, 'md5': m} for i, m in enumerate(frames)], 'outcome': outcome}


def failure(stage, kind, message, sent=3):
    return {'stage': stage, 'kind': kind, 'message': message, 'packets_sent': sent}


class Harness(unittest.TestCase):
    def setUp(self):
        self.dir = pathlib.Path(tempfile.mkdtemp(prefix='decode-breadth-'))
        self.addCleanup(__import__('shutil').rmtree, self.dir, True)
        for tool in ('ffprobe', 'ffmpeg', 'decode-oracle'):
            path = self.dir / tool
            path.write_text(STUB % {'python': sys.executable})
            path.chmod(path.stat().st_mode | stat.S_IXUSR)
        self.entries = []

    def add(self, name, stream=None, format_name=MP4, ext='mp4', probe=None, reference=None, coreav=None):
        """A file with stub behaviour: a probe listing one stream, a
        reference of MD5S, and CoreAV agreeing, unless told otherwise."""
        path = self.dir / name
        path.write_bytes(b'')
        stream = H264 if stream is None else stream
        spec = {'probe': {'streams': [stream], 'format_name': format_name, **(probe or {})},
                'reference': {'md5': MD5S, **(reference or {})},
                'coreav': {'object': ours(codec=stream.get('codec_name')), **(coreav or {})}}
        pathlib.Path(str(path) + '.spec').write_text(json.dumps(spec))
        self.entries.append({'sha256': sha(name), 'key': 'k/' + name, 'ext': ext, 'path': str(path)})
        return name

    def options(self, **kw):
        values = dict(decode_oracle=str(self.dir / 'decode-oracle'), ffmpeg=str(self.dir / 'ffmpeg'),
                      ffprobe=str(self.dir / 'ffprobe'), files=str(self.dir / 'files.json'),
                      records=str(self.dir / 'records.jsonl'), shard=None, fetch=None, deadline=None,
                      limit_minutes=1.0, rerun_host=False, jobs=1)
        values.update(kw)
        return argparse.Namespace(**values)

    def run_files(self, fetcher=None, **kw):
        pathlib.Path(self.dir / 'files.json').write_text(json.dumps(self.entries))
        with contextlib.redirect_stdout(io.StringIO()):
            return decode_breadth.run(self.options(**kw), fetcher)

    def records(self, name='records.jsonl'):
        return {r['sha256']: r for r in decode_breadth.load([self.dir / name])}

    def record(self, name):
        return self.records()[sha(name)]

    def calls(self, name):
        text = (self.dir / (name + '.calls')).read_text()
        return [json.loads(line) for line in text.splitlines()]

    def verdict(self, name, **kw):
        self.run_files(**kw)
        r = self.record(name)
        return r['code'], r['sub']


class NoReference(Harness):
    def test_no_stream(self):
        self.add('probe_fails', probe={'exit': 1})
        self.add('audio_only', stream={'index': 0, 'codec_type': 'audio', 'codec_name': 'aac'})
        self.add('picture_only', stream={**H264, 'disposition': {'attached_pic': 1}})
        self.add('no_streams')
        pathlib.Path(self.dir / 'no_streams.spec').write_text(json.dumps({'probe': {'streams': []}}))
        self.run_files()
        for name in ('probe_fails', 'audio_only', 'picture_only', 'no_streams'):
            r = self.record(name)
            self.assertEqual((r['code'], r['counted'], r['group'], r['host_limit']), ('no_stream', False, None, None), name)
            self.assertNotIn('reference', r)
        self.assertFalse((self.dir / 'audio_only.calls').read_text().count('"ffmpeg"'))

    def test_reference_failed_and_empty(self):
        self.add('fails', reference={'exit': 1, 'md5': MD5S})
        self.add('crashes', reference={'signal': signal.SIGSEGV})
        self.add('empty', reference={'md5': []})
        self.run_files()
        self.assertEqual(self.record('fails')['code'], 'reference_failed')
        self.assertEqual(self.record('crashes')['code'], 'reference_failed')
        self.assertEqual(self.record('empty')['code'], 'reference_empty')

    def test_reference_unfinished_only_on_the_rerun_host(self):
        self.add('slow', reference={'sleep': 15})
        self.run_files(limit_minutes=3 / 60)
        r = self.record('slow')
        self.assertEqual((r['code'], r['host_limit'], r['host_limit_side']), (None, 'deadline', 'reference'))
        self.assertEqual((r['reference']['killed'], r['reference']['limit_s']), ('limit', 3.0))
        pathlib.Path(self.dir / 'records.jsonl').unlink()
        self.run_files(limit_minutes=3 / 60, rerun_host=True)
        r = self.record('slow')
        self.assertEqual((r['code'], r['host_limit'], r['reference']['killed']), ('reference_unfinished', None, 'limit'))


class Counted(Harness):
    def test_agreement_counts(self):
        self.add('good')
        self.run_files()
        r = self.record('good')
        self.assertEqual((r['counted'], r['code'], r['sub'], r['group'], r['stream_index']), (True, None, None, 'h264', 0))
        self.assertEqual((r['reference']['frames'], r['coreav']['frames'], r['coreav']['backend']), (4, 4, 'native'))
        self.assertEqual(r['frames'], {'reference': 4, 'coreav': 4, 'first_differing': None})

    def test_a_codec_outside_the_build_counts_when_it_agrees(self):
        self.add('hevc_ok', stream={**H264, 'codec_name': 'hevc'})
        self.run_files()
        r = self.record('hevc_ok')
        self.assertEqual((r['group'], r['counted']), ('other', True))

    def test_the_reference_command(self):
        base = ['-nostdin', '-v', 'error', '-noautorotate']
        tail = ['-flags', 'unaligned', '-reinit_filter', '0', '-i']
        end = ['-fps_mode', 'passthrough']
        last = ['-noautoscale', '-f', 'framemd5', '-']
        cases = {
            'h264.mp4': (H264, 'mp4', base + ['-ignore_editlist', '1'] + tail),
            'h264.mkv': (H264, 'mkv', base + tail),
            'av1.mkv': ({**H264, 'codec_name': 'av1'}, 'mkv', base + ['-c:v', 'libdav1d'] + tail),
            'vp9.webm': ({**H264, 'codec_name': 'vp9'}, 'webm', base + ['-c:v', 'vp9'] + tail),
            'vp8.webm': ({**H264, 'codec_name': 'vp8'}, 'webm', base + ['-c:v', 'vp8'] + tail),
            'hevc.mov': ({**H264, 'codec_name': 'hevc'}, 'mov', base + ['-ignore_editlist', '1'] + tail),
        }
        for name, (stream, ext, head) in cases.items():
            self.add(name, stream={**stream, 'index': 2}, ext=ext, reference={'md5': MD5S})
        self.add('alpha.mkv', stream={**H264, 'codec_name': 'vp9', 'pix_fmt': 'yuva420p'}, ext='mkv')
        self.run_files()
        for name, (_, ext, head) in cases.items():
            [call] = [c for c in self.calls(name) if c['tool'] == 'ffmpeg']
            self.assertEqual(call['argv'], head + [str(self.dir / name), '-map', '0:2'] + end + last, name)
        [call] = [c for c in self.calls('alpha.mkv') if c['tool'] == 'ffmpeg']
        self.assertEqual(call['argv'][-11:], ['0:0', '-fps_mode', 'passthrough', '-pix_fmt', 'yuv420p', '-vf',
                                              'setparams=colorspace=unknown:color_primaries=unknown:color_trc=unknown'
                                              ':range=unknown'] + last)
        self.assertNotIn('-reinit_filter', call['argv'])

    def test_the_probe_command_lists_the_planned_entries(self):
        self.add('good')
        self.run_files()
        [call] = [c for c in self.calls('good') if c['tool'] == 'ffprobe']
        self.assertEqual(call['argv'][:5], ['-v', 'error', '-show_entries', decode_breadth.PROBE_ENTRIES, '-of'])
        for entry in ('index', 'codec_type', 'codec_name', 'width', 'height', 'pix_fmt', 'field_order',
                      'bits_per_raw_sample', 'attached_pic', 'format_name'):
            self.assertIn(entry, decode_breadth.PROBE_ENTRIES)

    def test_the_stream_is_the_largest_video_not_an_attached_picture(self):
        streams = [{**H264, 'index': 0, 'width': 10, 'height': 10}, {**H264, 'index': 1, 'width': 20, 'height': 20},
                   {**H264, 'index': 2, 'width': 90, 'height': 90, 'disposition': {'attached_pic': 1}},
                   {**H264, 'index': 3, 'width': 20, 'height': 20}]
        self.add('many')
        pathlib.Path(self.dir / 'many.spec').write_text(json.dumps({
            'probe': {'streams': streams, 'format_name': MP4}, 'reference': {'md5': MD5S},
            'coreav': {'object': ours(stream_index=1)}}))
        self.run_files()
        self.assertEqual((self.record('many')['stream_index'], self.record('many')['counted']), (1, True))


class NotRead(Harness):
    def read_error(self, name, kind, message, format_name=MP4):
        self.add(name, format_name=format_name, coreav={'exit': 1, 'object': {
            'stream_index': None, 'codec': None, 'backend': None, 'decoder': None, 'frames': [],
            'outcome': failure('read', kind, message, 0)}})
        return self.verdict(name)

    def test_subcodes_in_order(self):
        self.assertEqual(self.read_error('avi', 'unsupported_container', 'not a movie', 'avi'), ('not_read', 'container'))
        self.assertEqual(self.read_error('avi_rejected', 'truncated_structure', 'x', 'avi'), ('not_read', 'container'))
        self.assertEqual(self.read_error('truncated', 'truncated_structure', 'at 5', MKV), ('not_read', 'rejected'))
        self.assertEqual(self.read_error('invalid', 'invalid_structure', 'bad box', MP4), ('not_read', 'rejected'))
        self.assertEqual(self.read_error('encrypted', 'unsupported_feature', 'unsupported: encrypted video', MKV),
                         ('not_read', 'encrypted'))
        self.assertEqual(self.read_error('other', 'unsupported_feature', 'unsupported: encrypted videos', MKV),
                         ('not_read', 'other'))
        self.assertEqual(self.read_error('io', 'io', 'I/O error', MP4), ('not_read', 'other'))

    def test_the_text_of_an_other_is_kept(self):
        self.read_error('other', 'unsupported_feature', 'unsupported: something new')
        self.assertEqual(self.record('other')['text'], 'unsupported: something new')


class Precedence(Harness):
    def test_another_stream_comes_before_a_decode_error_and_a_codec_outside_the_build(self):
        self.add('ok_other_stream', coreav={'object': ours(stream_index=1)})
        self.add('failed_other_stream', stream={**H264, 'codec_name': 'hevc'}, coreav={'exit': 1, 'object': ours(
            stream_index=1, outcome=failure('packet', 'invalid_data', 'x'))})
        self.run_files()
        self.assertEqual(self.record('ok_other_stream')['code'], 'other_stream')
        self.assertEqual(self.record('failed_other_stream')['code'], 'other_stream')

    def test_a_codec_outside_the_build_comes_before_out_of_scope_and_errors(self):
        for name, outcome in (('a', failure('open', 'unsupported', 'no decoder')),
                              ('b', failure('packet', 'invalid_data', 'x'))):
            self.add(name, stream={**H264, 'codec_name': 'prores'}, coreav={'exit': 1, 'object': ours(outcome=outcome)})
        self.add('c', stream={**H264, 'codec_name': 'mpeg4'}, coreav={'object': ours(frames=['a'])})
        self.run_files()
        for name, codec in (('a', 'prores'), ('b', 'prores'), ('c', 'mpeg4')):
            self.assertEqual((self.record(name)['code'], self.record(name)['sub']), ('codec_not_in_build', codec))

    def test_a_kill_at_the_limit_is_a_timeout_whatever_the_tool_printed(self):
        self.add('printed', coreav={'exit': 1, 'object': ours(outcome=failure('packet', 'invalid_data', 'x')),
                                    'sleep': 15})
        self.add('printed_ok', coreav={'sleep': 15})
        self.run_files(limit_minutes=3 / 60, rerun_host=True)
        for name in ('printed', 'printed_ok'):
            r = self.record(name)
            self.assertEqual((r['code'], r['sub'], r['coreav']['killed'], r['host_limit']), ('timeout', None, 'limit', None))

    def test_an_error_before_the_reference_decides_nothing_without_a_reference(self):
        self.add('x', reference={'exit': 1}, coreav={'exit': 1, 'object': ours(outcome=failure('packet', 'limit', 'x'))})
        self.run_files()
        self.assertEqual(self.record('x')['code'], 'reference_failed')

    def test_a_side_the_runner_stopped_never_counts_even_when_its_output_looks_complete(self):
        """The tool prints a complete, agreeing object and exits 0, but a descendant keeps its output open past the
        limit: the runner stops the group, and the side is a timeout on the rerun host (a host limit elsewhere),
        never a counted file."""
        self.add('lingering', coreav={'child': True})
        self.add('lingering_reference', reference={'child': True})
        self.run_files(limit_minutes=3 / 60, rerun_host=True)
        r = self.record('lingering')
        self.assertEqual((r['coreav']['killed'], r['coreav']['exit']), ('limit', 0))
        self.assertEqual((r['code'], r['counted'], r['host_limit']), ('timeout', False, None))
        r = self.record('lingering_reference')
        self.assertEqual((r['reference']['killed'], r['reference']['exit']), ('limit', 0))
        self.assertEqual((r['code'], r['counted'], r['host_limit']), ('reference_unfinished', False, None))
        (self.dir / 'records.jsonl').unlink()
        self.run_files(limit_minutes=3 / 60)
        for name in ('lingering', 'lingering_reference'):
            r = self.record(name)
            self.assertEqual((r['code'], r['counted'], r['host_limit']), (None, False, 'deadline'), name)

    def test_a_stopped_side_keeps_the_codes_that_come_before_timeout(self):
        self.add('read_fails', coreav={'child': True, 'exit': 1,
                                       'object': ours(outcome=failure('read', 'unsupported', 'no video stream'))})
        self.add('elsewhere', coreav={'child': True, 'object': ours(stream_index=1)})
        self.run_files(limit_minutes=3 / 60, rerun_host=True)
        self.assertEqual(self.record('read_fails')['code'], 'not_read')
        self.assertEqual(self.record('elsewhere')['code'], 'other_stream')


class OutOfScope(Harness):
    def unsupported(self, name, stream, message='no decoder', stage='open'):
        self.add(name, stream={**H264, **stream}, coreav={'exit': 1, 'object': ours(
            outcome=failure(stage, 'unsupported', message))})
        return self.verdict(name)

    def test_h264_properties_in_order(self):
        cases = [
            ('ten', {'pix_fmt': 'yuv420p10le'}, '10bit'),
            ('ten_raw', {'bits_per_raw_sample': '10'}, '10bit'),
            ('ten_422', {'pix_fmt': 'yuv422p10le', 'field_order': 'tt'}, '10bit'),
            ('p422', {'pix_fmt': 'yuv422p'}, 'chroma'),
            ('p444', {'pix_fmt': 'yuv444p'}, 'chroma'),
            ('gray', {'pix_fmt': 'gray'}, 'chroma'),
            ('gray_interlaced', {'pix_fmt': 'gray', 'field_order': 'tb'}, 'chroma'),
            ('tff', {'field_order': 'tt'}, 'interlaced'),
            ('bff', {'field_order': 'bb'}, 'interlaced'),
            ('unknown_order', {'field_order': 'unknown'}, 'other'),
            ('plain', {}, 'other'),
            ('eight_raw', {'bits_per_raw_sample': '8', 'pix_fmt': 'yuvj420p'}, 'other'),
            ('nv12', {'pix_fmt': 'nv12'}, 'other'),
        ]
        for name, props, sub in cases:
            self.assertEqual(self.unsupported(name, props), ('out_of_scope', sub), name)

    def test_any_stage_counts(self):
        for stage in ('open', 'packet', 'eof'):
            self.assertEqual(self.unsupported('s_' + stage, {'field_order': 'tt'}, stage=stage), ('out_of_scope', 'interlaced'))

    def test_table_subcodes_of_the_other_groups(self):
        hbd = ('no decoder for vp9: nvdec: not available; native: this backend decodes H.264 only; dav1d: dav1d decodes '
               'AV1 only; libvpx: unsupported: worker: this libvpx decodes VP9 at 8 bits only (built without high '
               'bit depth); ffmpeg: not available')
        cases = [
            ('vp9_hbd', 'vp9', 'open', hbd, 'high_bit_depth'),
            ('vp9_440', 'vp9', 'packet', "worker: VP9 4:4:0 (profile 1 or 3) has no pixel format in CoreAV's frame model",
             'chroma_440'),
            ('vp9_440_eof', 'vp9', 'eof', "worker: VP9 4:4:0 (profile 1 or 3) has no pixel format in CoreAV's frame model",
             'chroma_440'),
            ('av1_bitstream', 'av1', 'packet', 'worker: dav1d decoding: Unsupported bitstream', 'unsupported_bitstream'),
            ('vp8_bitstream', 'vp8', 'eof', 'worker: libvpx decoding: Bitstream not supported by this decoder',
             'unsupported_bitstream'),
            ('vp9_feature', 'vp9', 'packet', 'worker: libvpx decoding: Bitstream required feature not supported by '
             'this decoder', 'unsupported_feature'),
            ('vp9_incapable', 'vp9', 'packet', 'worker: libvpx decoding: Codec does not implement requested capability',
             'incapable'),
            ('av1_unknown', 'av1', 'open', 'no decoder for av1: something new', 'other'),
            ('vp9_detail', 'vp9', 'packet', 'worker: libvpx decoding: Bitstream not supported by this decoder: detail',
             'other'),
            ('av1_wrong_stage', 'av1', 'open', 'worker: dav1d decoding: Unsupported bitstream', 'other'),
        ]
        for name, codec, stage, message, sub in cases:
            self.assertEqual(self.unsupported(name, {'codec_name': codec}, message, stage), ('out_of_scope', sub), name)

    def test_h264_ignores_the_table(self):
        self.assertEqual(self.unsupported('h', {}, 'worker: dav1d decoding: Unsupported bitstream', 'packet'),
                         ('out_of_scope', 'other'))


class DecodeError(Harness):
    def error(self, name, kind, message, stage='packet', **kw):
        self.add(name, coreav={'exit': 1, 'object': ours(outcome=failure(stage, kind, message)), **kw})
        return self.verdict(name)

    def test_subcodes_by_kind(self):
        self.assertEqual(self.error('limit', 'limit', 'decoded bytes exceed the limit'), ('decode_error', 'limit'))
        self.assertEqual(self.error('invalid', 'invalid_data', 'bad', 'eof'), ('decode_error', 'invalid_data'))
        self.assertEqual(self.error('backend', 'backend', 'worker: oops', 'open'), ('decode_error', 'backend'))
        self.assertEqual(self.error('io', 'io', 'pipe'), ('decode_error', 'other'))
        self.assertEqual(self.error('late', 'timeout', 'the decode deadline passed'), ('decode_error', 'other'))
        self.assertEqual(self.error('allow', 'codec_not_allowed', 'x', 'open'), ('decode_error', 'other'))

    def test_a_worker_that_exited_is_a_crash(self):
        self.assertEqual(self.error('exit', 'backend', 'worker exited'), ('decode_error', 'crash'))
        self.assertEqual(self.error('segv', 'backend', 'worker exited (worker signal: 11 (SIGSEGV))', 'eof'),
                         ('decode_error', 'crash'))
        self.assertEqual(self.error('stderr', 'backend', 'worker exited (worker signal: 11 (SIGSEGV); text)'),
                         ('decode_error', 'backend'))

    def test_the_tool_crashing_is_a_crash(self):
        self.add('signal', coreav={'signal': signal.SIGSEGV})
        self.add('abort', coreav={'object': None, 'exit': 134})
        self.add('no_object', coreav={'exit': 0, 'object': None})
        self.add('wrong_status', coreav={'exit': 1})
        self.add('wrong_status_error', coreav={'exit': 0, 'object': ours(outcome=failure('eof', 'limit', 'x'))})
        self.add('error_object', coreav={'exit': 1, 'object': {'error': 'usage'}})
        self.run_files()
        for name in ('signal', 'abort', 'no_object', 'wrong_status', 'wrong_status_error', 'error_object'):
            r = self.record(name)
            self.assertEqual((r['code'], r['sub'], r['host_limit']), ('decode_error', 'crash', None), name)

    def test_an_error_after_frames_keeps_its_code(self):
        self.add('partial', coreav={'exit': 1, 'object': ours(frames=['a', 'b'], outcome=failure('packet', 'limit', 'x'))})
        self.run_files()
        self.assertEqual(self.record('partial')['code'], 'decode_error')
        self.assertNotIn('frames', self.record('partial'))


class Mismatch(Harness):
    def test_frame_count_and_md5(self):
        self.add('fewer', coreav={'object': ours(frames=['a', 'b'])})
        self.add('none', coreav={'object': ours(frames=[])})
        self.add('more', coreav={'object': ours(frames=MD5S + ['e'])})
        self.add('changed', coreav={'object': ours(frames=['a', 'b', 'X', 'd'])})
        self.add('changed_first', coreav={'object': ours(frames=['X', 'b', 'c', 'd'])})
        self.add('count_and_changed', coreav={'object': ours(frames=['a', 'X', 'c'])})
        self.run_files()
        expect = {'fewer': ('frame_count', 4, 2, 2), 'none': ('frame_count', 4, 0, 0), 'more': ('frame_count', 4, 5, 4),
                  'changed': ('md5', 4, 4, 2), 'changed_first': ('md5', 4, 4, 0),
                  'count_and_changed': ('frame_count', 4, 3, 1)}
        for name, (code, ref, mine, first) in expect.items():
            r = self.record(name)
            self.assertEqual((r['code'], r['frames']['reference'], r['frames']['coreav'], r['frames']['first_differing']),
                             (code, ref, mine, first), name)
            self.assertFalse(r['counted'])


class HostLimits(Harness):
    def test_the_deadline_is_recorded_and_the_process_group_killed(self):
        self.add('hang', coreav={'sleep': 30, 'child': True})
        started = time.time()
        self.run_files(limit_minutes=3 / 60)
        self.assertLess(time.time() - started, 10)
        r = self.record('hang')
        self.assertEqual((r['host_limit'], r['host_limit_side'], r['code'], r['counted']), ('deadline', 'coreav', None, False))
        self.assertEqual((r['coreav']['killed'], r['coreav']['limit_s'], r['coreav']['exit'], r['coreav']['signal']),
                         ('limit', 3.0, None, signal.SIGKILL))
        child = int((self.dir / 'hang.child').read_text())
        for _ in range(50):
            try:
                os.kill(child, 0)
            except OSError:
                break
            time.sleep(0.1)
        else:
            self.fail('the descendant of the killed side survived')

    def test_a_kill_the_runner_did_not_send_is_the_memory_limit(self):
        self.add('probe_oom', probe={'signal': signal.SIGKILL})
        self.add('reference_oom', reference={'signal': signal.SIGKILL})
        self.add('coreav_oom', coreav={'signal': signal.SIGKILL})
        self.run_files()
        for name, side in (('probe_oom', 'probe'), ('reference_oom', 'reference'), ('coreav_oom', 'coreav')):
            r = self.record(name)
            self.assertEqual((r['host_limit'], r['host_limit_side'], r['code']), ('oom', side, None), name)
            self.assertIsNone(r[side if side != 'probe' else 'probe']['killed'])

    def test_a_lack_of_disk(self):
        self.add('full', coreav={'exit': 1, 'stderr': 'error: No space left on device (os error 28)\n'})
        self.add('full_ffmpeg', reference={'exit': 1, 'stderr': 'No space left on device\n'})
        self.run_files()
        self.assertEqual((self.record('full')['host_limit'], self.record('full')['host_limit_side']), ('enospc', 'coreav'))
        self.assertEqual(self.record('full_ffmpeg')['host_limit'], 'enospc')

    def test_a_result_that_mentions_a_full_disk_is_still_a_result(self):
        self.add('result', coreav={'exit': 1, 'object': ours(outcome=failure('packet', 'io', 'No space left on device'))})
        self.run_files()
        self.assertEqual((self.record('result')['host_limit'], self.record('result')['code']), (None, 'decode_error'))

    def test_a_probe_that_hits_the_limit_is_a_host_limit_on_the_rerun_host_too(self):
        self.add('slow_probe', probe={'sleep': 15})
        self.run_files(limit_minutes=3 / 60, rerun_host=True)
        r = self.record('slow_probe')
        self.assertEqual((r['host_limit'], r['host_limit_side'], r['code']), ('deadline', 'probe', None))

    def test_the_job_deadline_stops_a_side_and_starts_no_file(self):
        self.add('running', coreav={'sleep': 30})
        self.add('never_started')
        started = time.time()
        self.run_files(deadline=time.time() + 1.0, limit_minutes=10)
        self.assertLess(time.time() - started, 12)
        r = self.record('running')
        self.assertEqual((r['host_limit'], r['host_limit_side'], r['code'], r['coreav']['killed']),
                         ('job_end', 'coreav', None, 'job_end'))
        self.assertNotIn(sha('never_started'), self.records())
        self.assertFalse((self.dir / 'never_started.calls').exists())

    def test_the_job_deadline_is_a_host_limit_on_the_rerun_host(self):
        self.add('running', coreav={'sleep': 30})
        self.run_files(deadline=time.time() + 1.0, limit_minutes=10, rerun_host=True)
        self.assertEqual(self.record('running')['host_limit'], 'job_end')

    def test_a_deadline_already_past_runs_nothing(self):
        self.add('late')
        self.run_files(deadline=time.time() - 1)
        self.assertEqual(self.records(), {})


class Fetching(Harness):
    def fetcher(self, fail=()):
        def fetch(entry, directory):
            name = entry['path'].rsplit('/', 1)[-1]
            if name in fail:
                raise fail[name]
            path = directory / f"{entry['sha256']}.{entry['ext']}"
            for suffix in ('', '.spec'):
                pathlib.Path(str(path) + suffix).write_bytes(pathlib.Path(entry['path'] + suffix).read_bytes())
            return path
        return fetch

    def test_fetched_and_deleted_after_the_record(self):
        for i in range(3):
            self.add(f'f{i}')
        directory = self.dir / 'fetched'
        self.run_files(fetcher=self.fetcher(), fetch=str(directory))
        self.assertEqual(len(self.records()), 3)
        self.assertEqual([r['counted'] for r in self.records().values()], [True] * 3)
        leftovers = [p for p in directory.iterdir() if not p.name.endswith('.spec') and not p.name.endswith('.calls')]
        self.assertEqual(leftovers, [])

    def test_a_failed_fetch_is_a_host_limit(self):
        self.add('missing')
        self.add('full')
        self.add('fine')
        fail = {'missing': SystemExit('a fetched file does not have the SHA-256'),
                'full': OSError(28, 'No space left on device')}
        self.run_files(fetcher=self.fetcher(fail=fail), fetch=str(self.dir / 'fetched'))
        self.assertEqual((self.record('missing')['host_limit'], self.record('missing')['host_limit_side']), ('fetch', 'fetch'))
        self.assertEqual(self.record('full')['host_limit'], 'enospc')
        self.assertTrue(self.record('fine')['counted'])
        self.assertIsNone(self.record('missing')['code'])

    def test_at_most_jobs_files_are_on_disk(self):
        held = {'peak': 0}
        for i in range(6):
            self.add(f'f{i}', coreav={'early_sleep': 0.3})
        directory = self.dir / 'fetched'

        def watch():
            while not stop.is_set():
                files = [p for p in directory.glob('*.mp4')] if directory.exists() else []
                held['peak'] = max(held['peak'], len(files))
                time.sleep(0.01)
        stop = threading.Event()
        watcher = threading.Thread(target=watch)
        watcher.start()
        try:
            self.run_files(fetcher=self.fetcher(), fetch=str(directory), jobs=2)
        finally:
            stop.set()
            watcher.join()
        self.assertEqual(len(self.records()), 6)
        self.assertLessEqual(held['peak'], 2)
        self.assertGreaterEqual(held['peak'], 1)

    def test_the_isolated_entry_point_reaches_the_corpus_store(self):
        """The workflow and the rerun host run the module as python3 -I with --fetch: its imports resolve, and it
        stops only for the missing store credentials."""
        self.add('a')
        (self.dir / 'files.json').write_text(json.dumps(self.entries))
        env = {k: v for k, v in os.environ.items() if not k.startswith('R2_') and k != 'PYTHONPATH'}
        done = __import__('subprocess').run(
            [sys.executable, '-I', str(HERE / 'decode_breadth.py'), 'run', '--decode-oracle',
             str(self.dir / 'decode-oracle'), '--ffmpeg', str(self.dir / 'ffmpeg'), '--ffprobe', str(self.dir / 'ffprobe'),
             '--files', str(self.dir / 'files.json'), '--records', str(self.dir / 'records.jsonl'),
             '--fetch', str(self.dir / 'media')], capture_output=True, text=True, cwd=str(self.dir), env=env, timeout=60)
        self.assertNotIn('ModuleNotFoundError', done.stderr)
        self.assertNotEqual(done.returncode, 0)
        self.assertIn('R2_ENDPOINT', done.stderr)


class RecordsFile(Harness):
    def test_each_record_is_written_when_the_file_is_decided(self):
        self.add('first')
        self.add('second', coreav={'early_sleep': 2.0})
        pathlib.Path(self.dir / 'files.json').write_text(json.dumps(self.entries))
        thread = threading.Thread(target=self.run_files)
        thread.start()
        try:
            for _ in range(100):
                if (self.dir / 'records.jsonl').exists() and (self.dir / 'records.jsonl').read_text().count('\n') == 1:
                    break
                time.sleep(0.05)
            self.assertEqual(list(self.records()), [sha('first')])
            self.assertTrue(thread.is_alive())
        finally:
            thread.join()
        self.assertEqual(len(self.records()), 2)

    def test_a_file_with_a_line_is_not_run_again_and_a_torn_line_is_dropped(self):
        self.add('done')
        self.add('todo')
        self.run_files()
        text = (self.dir / 'records.jsonl').read_text().splitlines()
        (self.dir / 'records.jsonl').write_text(text[0] + '\n' + text[1][:20])
        first = sha('done') if json.loads(text[0])['sha256'] == sha('done') else sha('todo')
        for name in ('done', 'todo'):
            (self.dir / (name + '.calls')).unlink()
        self.run_files()
        records = self.records()
        self.assertEqual(len(records), 2)
        ran = [n for n in ('done', 'todo') if (self.dir / (n + '.calls')).exists()]
        self.assertEqual([sha(n) for n in ran], [s for s in (sha('done'), sha('todo')) if s != first])

    def test_shards_take_every_nth_from_the_ith(self):
        for i in range(5):
            self.add(f'f{i}')
        self.run_files(shard='1/2')
        self.assertEqual(sorted(self.records()), sorted(sha(f'f{i}') for i in (1, 3)))

    def test_the_sides_of_a_file_run_at_the_same_time(self):
        self.add('both', reference={'early_sleep': 3}, coreav={'early_sleep': 3})
        started = time.time()
        self.run_files()
        self.assertLess(time.time() - started, 5.5)
        self.assertTrue(self.record('both')['counted'])

    def test_programs_get_a_clean_environment(self):
        self.add('env')
        dirty = {'R2_SECRET_ACCESS_KEY': 'secret', 'R2_ENDPOINT': 'x', 'COREAV_STRICT_SAFE': '0', 'LD_PRELOAD': '/x'}
        with mock.patch.dict(os.environ, dirty):
            self.run_files()
        calls = {c['tool']: c for c in self.calls('env')}
        noise = {'LC_CTYPE', '__CF_USER_TEXT_ENCODING', 'PWD', 'SHLVL', '_'}
        env = {tool: set(call['env']) - noise for tool, call in calls.items()}
        self.assertEqual(env['ffprobe'], {'GCONV_PATH', 'HOME', 'PATH'})
        self.assertEqual(env['ffmpeg'], {'GCONV_PATH', 'HOME', 'PATH'})
        self.assertEqual(env['decode-oracle'], {'COREAV_STRICT_SAFE', 'GCONV_PATH', 'HOME', 'PATH'})
        for call in calls.values():
            self.assertEqual(call['gconv_files'], [])
        self.assertEqual(len({c['gconv'] for c in calls.values()}), 1)
        self.assertFalse(pathlib.Path(calls['ffprobe']['gconv']).exists())

    def test_the_decode_oracle_runs_the_stream_mode(self):
        self.add('mode')
        self.run_files()
        [call] = [c for c in self.calls('mode') if c['tool'] == 'decode-oracle']
        self.assertEqual(call['argv'], ['stream', str(self.dir / 'mode')])

    def test_a_program_that_is_not_there_stops_the_run(self):
        self.add('x')
        with self.assertRaises(SystemExit):
            self.run_files(ffmpeg=str(self.dir / 'absent'))


def record(sha_, group='h264', code=None, sub=None, counted=None, frames=None, host_limit=None, backend='native',
           codec=None):
    counted = code is None and host_limit is None if counted is None else counted
    return {'sha256': sha_, 'key': 'private/' + sha_, 'ext': 'mp4', 'group': group, 'codec': codec or group,
            'code': code, 'sub': sub, 'counted': counted, 'host_limit': host_limit, 'frames': frames,
            'coreav': {'backend': backend}}


class Scratch(unittest.TestCase):
    def setUp(self):
        self.dir = pathlib.Path(tempfile.mkdtemp(prefix='decode-breadth-'))
        self.addCleanup(__import__('shutil').rmtree, self.dir, True)

    def write(self, name, records):
        path = self.dir / name
        path.write_text(''.join(json.dumps(r) + '\n' for r in records))
        return str(path)


class Summary(Scratch):
    def summarize(self, github, rerun=(), expected=None):
        files = expected if expected is not None else sorted({r['sha256'] for r in list(github) + list(rerun)})
        (self.dir / 'files.json').write_text(json.dumps([{'sha256': s, 'key': 'k', 'ext': 'mp4'} for s in files]))
        opts = argparse.Namespace(records=[self.write('a.jsonl', github)], rerun=[self.write('b.jsonl', rerun)] if rerun else None,
                                  files=str(self.dir / 'files.json'), summary=str(self.dir / 'summary.json'))
        with contextlib.redirect_stdout(io.StringIO()):
            code = decode_breadth.summarize(opts)
        return code, json.loads((self.dir / 'summary.json').read_text())

    def test_the_result(self):
        frames = {'reference': 10, 'coreav': 10, 'first_differing': None}
        rows = [record('1', frames=frames), record('2', frames=frames), record('3', frames=frames),
                record('4', 'h264', 'out_of_scope', '10bit'), record('5', 'h264', 'out_of_scope', 'interlaced'),
                record('6', 'h264', 'decode_error', 'limit'), record('7', 'h264', 'md5', frames={'reference': 10, 'coreav': 10, 'first_differing': 3}),
                record('8', 'h264', 'frame_count', frames={'reference': 10, 'coreav': 7, 'first_differing': 7}),
                record('9', 'av1', frames=frames), record('10', 'av1', 'timeout'),
                record('11', 'vp9', 'out_of_scope', 'chroma_440'), record('12', 'vp8', 'not_read', 'container'),
                record('13', 'other', 'codec_not_in_build', 'hevc', codec='hevc'),
                record('14', 'other', 'codec_not_in_build', 'hevc', codec='hevc'),
                record('15', 'other', 'codec_not_in_build', 'prores', codec='prores'),
                record('16', None, 'no_stream', counted=False), record('17', 'h264', 'reference_failed', counted=False),
                record('18', 'av1', 'reference_empty', counted=False), record('19', 'vp9', 'reference_unfinished', counted=False)]
        code, s = self.summarize(rows)
        self.assertEqual(code, 0)
        self.assertEqual((s['files'], s['denominator'], s['counted'], s['void']), (19, 15, 4, False))
        self.assertEqual(s['share'], 4 / 15)
        self.assertFalse(s['meets_bar'])
        self.assertEqual(s['bar_percent'], 74)
        h = s['groups']['h264']
        self.assertEqual((h['denominator'], h['counted'], h['share']), (8, 3, 3 / 8))
        self.assertEqual(h['misses'], {'out_of_scope': {'files': 2, 'sub': {'10bit': 1, 'interlaced': 1}},
                                       'decode_error': {'files': 1, 'sub': {'limit': 1}},
                                       'md5': {'files': 1, 'sub': {}}, 'frame_count': {'files': 1, 'sub': {}}})
        self.assertEqual(s['groups']['av1']['misses'], {'timeout': {'files': 1, 'sub': {}}})
        self.assertEqual(s['groups']['vp9']['misses'], {'out_of_scope': {'files': 1, 'sub': {'chroma_440': 1}}})
        self.assertEqual(s['groups']['other']['misses'], {'codec_not_in_build': {'files': 3, 'sub': {'hevc': 2, 'prores': 1}}})
        self.assertEqual(s['other_codecs'], {'hevc': 2, 'prores': 1})
        self.assertEqual(s['no_reference'], {'h264': {'reference_failed': 1}, 'av1': {'reference_empty': 1},
                                             'vp9': {'reference_unfinished': 1}, 'none': {'no_stream': 1}})
        self.assertEqual(s['frames_compared'], 4 * 10 + 7 + 10)
        self.assertEqual(s['files_rerun'], 0)

    def test_the_bar_is_74_percent_of_the_denominator(self):
        rows = [record(str(i)) for i in range(74)] + [record(str(100 + i), code='md5') for i in range(26)]
        self.assertTrue(self.summarize(rows)[1]['meets_bar'])
        rows = [record(str(i)) for i in range(73)] + [record(str(100 + i), code='md5') for i in range(27)]
        self.assertFalse(self.summarize(rows)[1]['meets_bar'])

    def test_counts_only(self):
        rows = [record('abcdef0123', code='md5', frames={'reference': 1, 'coreav': 1, 'first_differing': 0}),
                record('fedcba9876', 'other', 'codec_not_in_build', 'hevc', codec='hevc')]
        text = json.dumps(self.summarize(rows)[1])
        for private in ('abcdef0123', 'fedcba9876', 'private/'):
            self.assertNotIn(private, text)

    def test_rerun_lines_replace_the_github_pass_lines(self):
        github = [record('1'), record('2', host_limit='deadline', code=None, counted=False),
                  record('3', host_limit='oom', counted=False)]
        rerun = [record('2', code='timeout'), record('3', frames={'reference': 5, 'coreav': 5, 'first_differing': None})]
        code, s = self.summarize(github, rerun)
        self.assertEqual((code, s['void'], s['denominator'], s['counted'], s['files_rerun']), (0, False, 3, 2, 2))
        self.assertEqual(s['groups']['h264']['misses'], {'timeout': {'files': 1, 'sub': {}}})

    def test_the_last_line_of_a_file_wins(self):
        code, s = self.summarize([record('1', host_limit='oom', counted=False), record('1')])
        self.assertEqual((code, s['counted']), (0, 1))

    def test_a_file_without_a_final_result_voids_the_run(self):
        code, s = self.summarize([record('1')], expected=['1', '2'])
        self.assertEqual((code, s['void'], s['void_reasons'], s['meets_bar']), (2, True, [{'reason': 'no_final_result', 'files': 1}], None))

    def test_a_host_limit_left_after_the_rerun_voids_the_run(self):
        code, s = self.summarize([record('1', host_limit='deadline', counted=False)],
                                 [record('1', host_limit='oom', counted=False)])
        self.assertEqual((code, s['void_reasons']), (2, [{'reason': 'no_final_result', 'files': 1}]))

    def test_the_ffmpeg_fallback_voids_the_run(self):
        code, s = self.summarize([record('1', backend='ffmpeg'), record('2')])
        self.assertEqual((code, s['void_reasons']), (2, [{'reason': 'ffmpeg_fallback_backend', 'files': 1}]))

    def test_a_line_outside_the_set_voids_the_run(self):
        code, s = self.summarize([record('1'), record('2')], expected=['1'])
        self.assertEqual((code, s['void_reasons'], s['denominator']), (2, [{'reason': 'file_outside_the_set', 'files': 1}], 1))


class PendingAndMismatches(Scratch):
    def entries(self):
        return [{'sha256': str(i), 'key': 'k' + str(i), 'ext': 'mp4', 'size': 1, 'family': 'mp4-mov'} for i in range(6)]

    def pending(self, records, shard=None):
        (self.dir / 'files.json').write_text(json.dumps(self.entries()))
        opts = argparse.Namespace(records=[self.write('a.jsonl', records)], files=str(self.dir / 'files.json'),
                                  shard=shard, out=str(self.dir / 'rerun.json'))
        with contextlib.redirect_stdout(io.StringIO()):
            decode_breadth.pending(opts)
        return json.loads((self.dir / 'rerun.json').read_text())

    def test_pending_is_a_host_limit_or_no_line_in_the_form_of_the_files(self):
        got = self.pending([record('0'), record('1', host_limit='oom', counted=False), record('2', code='md5'),
                            record('4', host_limit='fetch', counted=False)])
        self.assertEqual(got, [e for e in self.entries() if e['sha256'] in ('1', '3', '4', '5')])

    def test_pending_takes_a_shard(self):
        got = self.pending([], shard='1/3')
        self.assertEqual([e['sha256'] for e in got], ['1', '4'])

    def test_mismatches_lists_frame_count_and_md5_files(self):
        rows = [record('aa', code='md5', frames={'reference': 9, 'coreav': 9, 'first_differing': 4}),
                record('bb', code='frame_count', frames={'reference': 9, 'coreav': 8, 'first_differing': 8}),
                record('cc'), record('dd', code='decode_error', sub='limit')]
        opts = argparse.Namespace(records=[self.write('a.jsonl', rows)], rerun=None, out=str(self.dir / 'm.md'))
        with contextlib.redirect_stdout(io.StringIO()):
            decode_breadth.mismatches(opts)
        text = (self.dir / 'm.md').read_text()
        self.assertIn('| aa | private/aa | h264 | md5 | 9 | 9 | 4 |', text)
        self.assertIn('| bb | private/bb | h264 | frame_count | 9 | 8 | 8 |', text)
        self.assertNotIn('| cc |', text)
        self.assertNotIn('| dd |', text)


class ReasonTable(unittest.TestCase):
    def test_every_row_quotes_the_source_that_emits_it(self):
        rows = json.loads((HERE / 'decode_reasons.json').read_text())['rows']
        self.assertTrue(rows)
        for row in rows:
            source = (ROOT / row['source']).read_text()
            self.assertIn(row['literal'], source, row['message'])

    def test_keys_are_unique_and_the_codes_are_the_plan_s(self):
        rows = json.loads((HERE / 'decode_reasons.json').read_text())['rows']
        keys = [(r['stage'], r['kind'], r['message']) for r in rows]
        self.assertEqual(len(keys), len(set(keys)))
        for row in rows:
            self.assertIn(row['stage'], ('read', 'open', 'packet', 'eof'))
            self.assertIn(row['code'], ('not_read', 'out_of_scope', 'decode_error'))
        self.assertEqual(decode_breadth.reasons().get(('read', 'unsupported_feature', 'unsupported: encrypted video')),
                         ('not_read', 'encrypted'))


if __name__ == '__main__':
    unittest.main()

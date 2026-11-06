"""copy_check.py's reading of ffmpeg's stream mapping and of the
summaries (docs/design/compat-cli.md, "coreav ffmpeg")."""
import copy
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent.parent))

import copy_check  # noqa: E402


class Timing(unittest.TestCase):
    def check(self, record, value):
        self.assertEqual(record, {"job": "join"})
        return value

    def test_check_timing_preserves_the_result(self):
        result = object()
        with mock.patch.object(copy_check.time, "monotonic", side_effect=[10, 12.5]), \
                mock.patch.object(copy_check.os, "write") as write:
            self.assertIs(copy_check.timed_check(True, 123, self.check, {"job": "join"}, result), result)
        event = json.loads(write.call_args.args[1])
        self.assertEqual(event, {"event": "copy_check_call", "sequence": 123, "check": "check", "job": "join",
                                 "seconds": 2.5, "completed": True})

    def test_check_timing_preserves_an_exception(self):
        def failed(record):
            raise ValueError("invalid check")

        with mock.patch.object(copy_check.time, "monotonic", side_effect=[10, 11]), \
                mock.patch.object(copy_check.os, "write") as write:
            with self.assertRaisesRegex(ValueError, "invalid check"):
                copy_check.timed_check(True, 123, failed, {"job": "join"})
        event = json.loads(write.call_args.args[1])
        self.assertEqual(event["seconds"], 1)
        self.assertFalse(event["completed"])

    def test_timing_is_opt_in_for_other_copy_checker_callers(self):
        result = object()
        with mock.patch.object(copy_check.os, "write") as write:
            self.assertIs(copy_check.timed_check(False, 123, self.check, {"job": "join"}, result), result)
        write.assert_not_called()


class Mapping(unittest.TestCase):
    def test_mapping_lines(self):
        stderr = ("Stream mapping:\n"
                  "  Stream #0:2 -> #0:1 (copy)\n"
                  "  Stream #0:0 -> #0:0 (copy)\n"
                  "Press [q] to stop\n")
        mapped = sorted((int(o), int(i)) for i, o in copy_check.MAPPING.findall(stderr))
        self.assertEqual(mapped, [(0, 0), (1, 2)])

    def test_selected_streams_of_each_summary(self):
        record = lambda detail: {"detail": detail}  # noqa: E731
        self.assertEqual(copy_check.selected(record({"stream": {"stream_index": 3}})), [3])
        self.assertEqual(copy_check.selected(record({"streams": [{"stream_index": 0}, {"stream_index": 2}]})),
                         [0, 2])
        segments = {"segments": [{"streams": [{"stream_index": 1}]},
                                 {"streams": [{"stream_index": 1}, {"stream_index": 0}]}]}
        self.assertEqual(copy_check.selected(record(segments)), [1, 0])


class Concat(unittest.TestCase):
    def test_microseconds_round_halves_away_from_zero(self):
        F = copy_check.fractions.Fraction
        self.assertEqual(copy_check.micros(F(1, 2_000_000)), 1)
        self.assertEqual(copy_check.micros(F(-1, 2_000_000)), -1)
        self.assertEqual(copy_check.micros(F(1, 3)), 333_333)

    def test_each_input_follows_the_durations_before_it(self):
        F = copy_check.fractions.Fraction
        # A 0.3 s video with 1 s of audio, listed twice: the demuxer places
        # the second file at 1 s, a join at the video's end at 0.3 s.
        formats = [(0, 1_000_000), (0, 1_000_000)]
        self.assertEqual(copy_check.placement([F(0), F(1)], formats), [])
        self.assertEqual(copy_check.placement([F(0), F(3, 10)], formats),
                         ["timing: input 1 placed at 300000 us, ffmpeg concat 1000000 us"])
        # Each file's own start time is moved to where the file goes.
        self.assertEqual(copy_check.placement([F(5), F(8)], [(500_000, 2_000_000), (-500_000, 1_000_000)]), [])

    def test_a_placement_is_compared_in_the_video_ticks(self):
        F = copy_check.fractions.Fraction
        # 4.032 s is 51609.6 ticks of 1/12800: the video goes to 51610.
        formats = [(0, 4_032_000), (0, 4_032_000)]
        self.assertEqual(copy_check.placement([F(0), F(51610, 12800)], formats, 12800), [])
        self.assertEqual(copy_check.placement([F(0), F(51609, 12800)], formats, 12800),
                         ["timing: input 1 placed at 51609 ticks, ffmpeg concat 51610 ticks"])

    def test_the_presentation_end_by_the_boxes(self):
        F = copy_check.fractions.Fraction

        def track(handler, timescale, durations, cto, base=0):
            return {"handler": handler, "timescale": timescale, "edits": None,
                    "samples": {"durations": durations, "cto": cto, "sizes": [1] * len(durations),
                                "base_dts": base, "gap": False}}
        # Two frames of 512 ticks presented from 1024, and 1 s of audio.
        video = track("vide", 12_800, [512, 512], [1024, 1024])
        audio = track("soun", 1000, [500, 500], [0, 0])
        movie = {"fragmented": True, "tracks": [video, audio]}
        self.assertEqual(copy_check.presentation(movie), (F(1), F(2, 25), 2))
        longer = dict(video, samples=dict(video["samples"], durations=[512, 12_800]))
        self.assertEqual(copy_check.presentation(dict(movie, tracks=[longer, audio])), (F(28, 25), F(2, 25), 2))
        self.assertIsNone(copy_check.presentation(dict(movie, fragmented=False)))
        self.assertIsNone(copy_check.presentation(dict(movie, tracks=[dict(video, edits=[(1, 1024, 1)]), audio])))
        self.assertIsNone(copy_check.presentation(dict(movie, tracks=[audio])))

    def test_only_an_excess_of_the_first_composition_time_is_set_aside(self):
        F = copy_check.fractions.Fraction
        found = [(F(502, 25), F(2, 25), 500)] * 2
        self.assertEqual(copy_check.composition_gaps([(0, 20_160_000)] * 2, found), [F(2, 25)] * 2)
        self.assertEqual(copy_check.composition_gaps([(0, 20_080_000)] * 2, found), [0, 0])
        self.assertEqual(copy_check.composition_gaps([(0, 20_200_000), (0, 20_160_000)], [None, found[1]]),
                         [0, F(2, 25)])
        times = [(F(i), F(i)) for i in range(4)]
        self.assertEqual(copy_check.without_gaps(times, [F(1, 2), F(1, 2)], [2, 2]),
                         [(F(0), F(0)), (F(1), F(1)), (F(3, 2), F(3, 2)), (F(5, 2), F(5, 2))])

    def test_an_indexed_input_beside_an_ordinary_one(self):
        # The first input has a segment index and its video presents from
        # 0.08 s: ffmpeg gives it 1.08 s, its boxes 1 s, and places the
        # second, ordinary input (two frames from 0) that much later. The
        # join places it at 1 s, and ffmpeg's packets after the first input
        # are read 0.08 s earlier.
        def track(handler, timescale, durations, cto):
            return {"handler": handler, "timescale": timescale, "edits": None,
                    "samples": {"durations": durations, "cto": cto, "sizes": [1] * len(durations),
                                "base_dts": 0, "gap": False}}
        movie = {"fragmented": True, "tracks": [track("vide", 12_800, [512, 512], [1024, 1024]),
                                                track("soun", 1000, [500, 500], [0, 0])]}
        formats = {"indexed.mp4": (0, 1.08), "plain.mp4": (0, 1)}
        packets = {"indexed.mp4": [8, 12], "plain.mp4": [0, 4], "concat.mp4": [8, 12, 108, 112],
                   "join.mp4": [8, 12, 100, 104]}

        def run(args, **_):
            path = pathlib.Path(args[-1]).name
            if "format=start_time,duration" in args:
                start, duration = formats[path]
                out = {"format": {"start_time": str(start), "duration": str(duration)}}
            elif "stream=time_base:packet=pts,dts" in args:
                out = {"streams": [{"time_base": "1/100"}], "packets": [{"pts": t, "dts": t} for t in packets[path]]}
            else:
                out = {}
            return subprocess.CompletedProcess(args, 0, json.dumps(out).encode(), b"")

        record = {"detail": {"inputs": [{"path": "indexed.mp4", "offset": {"num": 0, "den": 1, "timescale": 12_800}},
                                        {"path": "plain.mp4", "offset": {"num": 1, "den": 1, "timescale": 12_800}}]},
                  "input": "list.txt", "args": ["-f", "concat", "-i", "INPUT", "-c", "copy", "OUTPUT"],
                  "output": "join.mp4"}
        boxes = {"indexed.mp4": [(b"ftyp",), (b"sidx",), (b"moof",)], "plain.mp4": [(b"ftyp",), (b"moov",)]}
        with mock.patch.object(copy_check.subprocess, "run", run), \
                mock.patch.object(copy_check.track_boxes, "top_level", lambda path: boxes[path]), \
                mock.patch.object(copy_check.track_boxes, "read_movie", lambda path: movie), \
                mock.patch.object(copy_check.writer_corpus, "ratio", lambda value: value):
            self.assertEqual(copy_check.concat_problems(record, "ffprobe", "ffmpeg"), [])
            packets["join.mp4"] = [8, 12, 108, 112]
            self.assertEqual(copy_check.concat_problems(record, "ffprobe", "ffmpeg"),
                             ["timing: video packet 2 of 4 at other times than ffmpeg concat's (4 packets)"])


class Range(unittest.TestCase):
    def test_times_in_ffmpeg_syntax(self):
        F = copy_check.fractions.Fraction
        self.assertEqual(copy_check.seconds("00:02:54.583"), F(174583, 1000))
        self.assertEqual(copy_check.seconds("03:10"), 190)
        self.assertEqual(copy_check.seconds("100ms"), F(1, 10))
        self.assertEqual(copy_check.seconds("1500us"), F(3, 2000))
        self.assertEqual(copy_check.seconds("2.5s"), F(5, 2))
        self.assertEqual(copy_check.seconds("120"), 120)

    def test_every_time_counts_from_the_input_start(self):
        # The pinned ffmpeg on an all-intra video at one frame a second
        # whose first frame is presented at 5 s.
        def at_5(command):
            return copy_check.requested(command.split(), 5)
        self.assertEqual(at_5("-ss 6 -i INPUT -t 1 -c copy OUTPUT"), (11, 12))
        self.assertEqual(at_5("-i INPUT -ss 6 -t 1 -c copy OUTPUT"), (11, 12))
        self.assertEqual(at_5("-ss 2 -i INPUT -ss 3 -t 1 -c copy OUTPUT"), (10, 11))
        self.assertEqual(at_5("-ss 6 -i INPUT -to 2 -c copy OUTPUT"), (11, 13))
        self.assertEqual(at_5("-ss 1 -i INPUT -to 3 -c copy OUTPUT"), (6, 9))
        self.assertEqual(at_5("-to 3 -i INPUT -c copy OUTPUT"), (5, 8))
        self.assertEqual(at_5("-i INPUT -ss 6 -c copy OUTPUT"), (11, None))


def seek_record(args=None):
    return {'job': 'clip', 'source': 'source.mp4', 'input': 'source.mp4', 'output': 'out.mp4',
            'args': args or ['-ss', '9', '-i', 'INPUT', '-t', '3', '-c', 'copy', 'OUTPUT']}


class InputSeekDispatch(unittest.TestCase):
    def test_both_finite_bound_spellings_and_copy_aliases(self):
        F = copy_check.fractions.Fraction
        self.assertEqual(copy_check.input_seek_args(seek_record()), (F(9), F(3)))
        self.assertEqual(copy_check.input_seek_args(seek_record(
            ['-y', '-ss', '00:00:09', '-i', 'INPUT', '-to', '3', '-vcodec', 'copy', '-acodec', 'copy', 'OUTPUT'])),
            (F(9), F(3)))

    def test_outside_argument_shapes_stay_legacy(self):
        for args in (
            ['-i', 'INPUT', '-ss', '9', '-t', '3', '-c', 'copy', 'OUTPUT'],
            ['-ss', '9', '-t', '3', '-i', 'INPUT', '-c', 'copy', 'OUTPUT'],
            ['-ss', '9', '-i', 'INPUT', '-c', 'copy', 'OUTPUT'],
            ['-ss', '9', '-i', 'INPUT', '-t', '3', '-c:a', 'copy', 'OUTPUT'],
            ['-ss', '9', '-i', 'INPUT', '-t', '3', '-c', 'copy', '-an', 'OUTPUT'],
            ['-ss', '9', '-i', 'INPUT', '-t', '3', '-to', '3', '-c', 'copy', 'OUTPUT'],
            ['-ss', '9', '-ss', '10', '-i', 'INPUT', '-t', '3', '-c', 'copy', 'OUTPUT'],
            ['-ss', 'bad', '-i', 'INPUT', '-t', '3', '-c', 'copy', 'OUTPUT'],
            ['-ss', '9', '-i', 'OTHER', '-t', '3', '-c', 'copy', 'OUTPUT'],
            ['-ss', '9', '-i', 'INPUT', '-t', '3', '-c', 'copy', '-map', '0', 'OUTPUT'],
        ):
            with self.subTest(args=args):
                self.assertIsNone(copy_check.input_seek_args(seek_record(args)))

    def test_source_facts_not_summary_or_fixture_name_choose_the_model(self):
        record = seek_record()
        record['detail'] = {'untrusted': 'not a selection authority'}
        for handlers, admitted in ((['vide', 'soun'], True), (['soun', 'vide'], True),
                                    (['vide'], False), (['soun'], False)):
            with self.subTest(handlers=handlers), mock.patch.object(copy_check.track_boxes, 'read_movie',
                    return_value={'tracks': [{'handler': h} for h in handlers]}):
                self.assertEqual(copy_check.input_seek_operation(record) is not None, admitted)

    def test_source_read_errors_cannot_pass_or_escape_to_legacy(self):
        with mock.patch.object(copy_check.track_boxes, 'read_movie', side_effect=OSError('unreadable')):
            self.assertIsNotNone(copy_check.input_seek_operation(seek_record()))

    def test_legacy_dispatch_and_input_seek_dispatch_are_separate(self):
        import clip_check
        record = seek_record(['-i', 'INPUT', '-ss', '9', '-t', '3', '-c', 'copy', 'OUTPUT'])
        tools = {'ffmpeg': 'ffmpeg', 'ffprobe': 'ffprobe', 'mediainfo': None}
        with mock.patch.object(copy_check.track_boxes, 'read_movie', return_value={}), \
                mock.patch.object(copy_check, 'streams', return_value=[]), \
                mock.patch.object(copy_check, 'entry', return_value={'legacy': True}), \
                mock.patch.object(copy_check, 'range_problems', return_value=[]), \
                mock.patch.object(clip_check, 'check', return_value=([], [], [], {})) as legacy:
            self.assertEqual(copy_check.oracle(record, tools), [])
            legacy.assert_called_once_with({'legacy': True}, 'ffprobe', 'ffmpeg', None)
        with mock.patch.object(copy_check, 'input_seek_operation', return_value=(9, 3)), \
                mock.patch.object(copy_check, 'input_seek_problems', return_value=['model failure']) as current, \
                mock.patch.object(clip_check, 'check') as legacy:
            self.assertEqual(copy_check.oracle(seek_record(), tools), ['model failure'])
            current.assert_called_once()
            legacy.assert_not_called()


class InputSeekPin(unittest.TestCase):
    def setUp(self):
        self.owner = tempfile.TemporaryDirectory()
        self.addCleanup(self.owner.cleanup)
        self.directory = pathlib.Path(self.owner.name)
        self.tools = {'ffmpeg': 'ffmpeg', 'ffprobe': 'ffprobe', 'mediainfo': None}
        self.snapshot = {'packets_and_frames': [{'type': 'frame', 'stream_index': 0}],
                         'streams': [{'codec_name': 'h264', 'width': 2, 'height': 2},
                                     {'codec_name': 'aac', 'profile': 'LC', 'channels': 1, 'nb_frames': '1'}]}
        self.calls = []
        self.failure = None

    def run_command(self, args, **kwargs):
        self.calls.append(args)
        target = pathlib.Path(args[-1])
        if '-f' not in args:
            target.write_bytes(b'pin')
            return subprocess.CompletedProcess(args, int(self.failure == 'copy exit'), b'',
                                               b'error' if self.failure == 'stderr' else b'')
        data = b'pixels' if 'rawvideo' in args else b'pcm!'
        if self.failure == target.name:
            data = data[:-1] + b'?'
        if self.failure == 'empty':
            data = b''
        if self.failure == 'extra prefix' and target.name == 'audio-output':
            data += b'extra'
        target.write_bytes(data)
        return subprocess.CompletedProcess(args, 0, b'', b'')

    def check_pin(self):
        with mock.patch.object(copy_check.subprocess, 'run', side_effect=self.run_command), \
                mock.patch.object(copy_check, 'input_seek_snapshot', return_value=self.snapshot):
            return copy_check.input_seek_pin(seek_record(), self.tools, 1, self.directory)

    def test_exact_copy_argv_and_complete_pixel_and_pcm_comparisons(self):
        self.assertEqual(self.check_pin(), [])
        expected = ['ffmpeg', '-nostdin', '-v', 'error', *copy_check.AUDIO_DECODE_INPUT,
                    '-ss', '9', '-i', 'source.mp4', '-t', '3', '-c', 'copy', str(self.directory / 'out.mp4')]
        self.assertEqual(self.calls[0], expected)
        self.assertEqual(len(self.calls), 5)

    def test_pixel_pcm_and_extra_prefix_changes_fail(self):
        for failure in ('video-output', 'audio-output', 'extra prefix'):
            self.failure = failure
            with self.subTest(failure=failure):
                self.assertTrue(self.check_pin())

    def test_pin_failure_empty_decode_and_resource_cap_fail(self):
        for failure in ('copy exit', 'stderr', 'empty'):
            self.failure = failure
            with self.subTest(failure=failure), self.assertRaises(ValueError):
                self.check_pin()
        self.failure = None
        with mock.patch.object(copy_check, 'INPUT_SEEK_MAX_RAW', 1), self.assertRaises(ValueError):
            self.check_pin()

    def test_complete_record_and_presented_frame_tie_cannot_be_skipped(self):
        changed = copy.deepcopy(self.snapshot)
        changed['streams'][0]['width'] = 3
        with mock.patch.object(copy_check.subprocess, 'run', side_effect=self.run_command), \
                mock.patch.object(copy_check, 'input_seek_snapshot', side_effect=[changed, self.snapshot]):
            self.assertTrue(copy_check.input_seek_pin(seek_record(), self.tools, 1, self.directory))
        with mock.patch.object(copy_check.subprocess, 'run', side_effect=self.run_command), \
                mock.patch.object(copy_check, 'input_seek_snapshot', return_value=self.snapshot), self.assertRaises(ValueError):
            copy_check.input_seek_pin(seek_record(), self.tools, 2, self.directory)

    def test_timeout_propagates_instead_of_becoming_success(self):
        with mock.patch.object(copy_check.subprocess, 'run', side_effect=subprocess.TimeoutExpired('ffmpeg', 1)), \
                self.assertRaises(subprocess.TimeoutExpired):
            copy_check.input_seek_pin(seek_record(), self.tools, 1, self.directory)


class InputSeekAudioEdit(unittest.TestCase):
    def test_an_edit_inside_the_first_audio_sample_does_not_invent_a_recovery_requirement(self):
        clock = 45_158_400
        video = {'handler': 'vide', 'timescale': 12288, 'mdhd_duration': 12288, 'tkhd_duration': clock,
                 'edits': [(clock, 0, 1)], 'samples': {'base_dts': 0, 'gap': False,
                 'sizes': [1] * 24, 'durations': [512] * 24, 'cto': [0] * 24, 'offsets': list(range(16, 40))}}
        audio_span = (44 * 1024 - 546) * 1024
        audio = {'handler': 'soun', 'timescale': 44100, 'mdhd_duration': 44 * 1024, 'tkhd_duration': audio_span,
                 'edits': [(audio_span, 546, 1)], 'samples': {'base_dts': 0, 'gap': False,
                 'sizes': [1] * 44, 'durations': [1024] * 44, 'cto': [0] * 44, 'offsets': list(range(40, 84))}}
        extra = {'descriptions': 1, 'indices': [1], 'rates': [65536], 'volume': 0}
        a_extra = dict(extra, volume=256, groups={b'sgpd': copy_check.INPUT_SEEK_ROLL,
            b'sbgp': b'\0' * 4 + b'roll' + copy_check.struct.pack('>III', 1, 44, 1)})
        written = {'brand': b'isom', 'fragmented': False, 'media': (16, 84), 'tracks': [extra, a_extra]}
        movie = {'tracks': [video, audio], 'movie_timescale': clock, 'movie_duration': audio_span}
        plan = [dict(kind='video', source_index=0, first=12, count=24, media_time=0),
                dict(kind='audio', source_index=1, first=22, count=44, media_time=546)]
        time = lambda ticks: {'ticks': {'num': ticks, 'den': 1}, 'timescale': 12288}
        record = seek_record()
        record['detail'] = {'streams': [dict(kind=r['kind'], stream_index=r['source_index'],
            first_sample=r['first'], samples=r['count']) for r in plan],
            'start': {'requested': {'num': 1, 'den': 2}, 'snapped': time(6144)},
            'end': {'requested': {'num': 3, 'den': 2}, 'written': time(18432)}}
        F = copy_check.fractions.Fraction
        with mock.patch.object(copy_check.writer_boxes, 'compare_samples', return_value=[]), \
                mock.patch.object(copy_check.writer_boxes, 'compare_entry', return_value=[]):
            self.assertEqual(copy_check.input_seek_iso(record, (F(1, 2), F(1)), movie, plan, movie, written), ([], 24))


@unittest.skipUnless(os.environ.get('COREAV_CURRENT_CI_ORACLE') == '1', 'requires the generated writer records and pinned oracle')
class InputSeekGenerated(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        import test_oracle
        pair = test_oracle.require()
        cls.tools = {'ffmpeg': str(pair.ffmpeg), 'ffprobe': str(pair.ffprobe), 'mediainfo': None}
        records = json.loads((pathlib.Path(os.environ['COREAV_FFMPEG_OUT']) / 'records.json').read_text())
        cls.records = [r for r in records if r['status'] == 'written' and copy_check.input_seek_operation(r) is not None]
        if not cls.records:
            raise AssertionError('no written generated input-seek records')

    def test_complete_generated_copies_pass_the_new_model(self):
        for record in self.records:
            with self.subTest(args=record['args'], source=record['source']):
                self.assertEqual(copy_check.oracle(record, self.tools), [])

    def test_summary_tampering_fails_on_actual_outputs(self):
        for original in self.records:
            for field in ('start', 'end', 'count', 'first', 'index'):
                record = copy.deepcopy(original)
                if field == 'start':
                    record['detail']['start']['snapped']['ticks']['num'] -= 1
                elif field == 'end':
                    record['detail']['end']['written']['ticks']['num'] += 1
                else:
                    key = {'count': 'samples', 'first': 'first_sample', 'index': 'stream_index'}[field]
                    record['detail']['streams'][0][key] += 1
                with self.subTest(source=record['source'], field=field):
                    self.assertTrue(copy_check.oracle(record, self.tools))

    def test_a_nonmatching_output_cannot_pass_through_the_input_seek_dispatch(self):
        if len(self.records) < 2:
            self.fail('two distinct generated input-seek outputs are required')
        for index, original in enumerate(self.records):
            record = copy.deepcopy(original)
            record['output'] = self.records[(index + 1) % len(self.records)]['output']
            with self.subTest(source=record['source']):
                self.assertIsNotNone(copy_check.input_seek_operation(record))
                self.assertTrue(copy_check.oracle(record, self.tools))

    def test_non_avc_or_non_aac_sources_fail_the_bounded_model(self):
        tb = copy_check.track_boxes
        original = self.records[0]
        before = pathlib.Path(original['source']).read_bytes()
        moov = next(b for b in tb.top_level(original['source']) if b[0] == b'moov')
        data = memoryview(tb.read_box(original['source'], *moov[1:], tb.MAX_MOOV))
        tracks = tb.children(data, (0, len(data)), b'trak')
        for index, kind in ((0, b'hev1'), (1, b'ac-3')):
            with self.subTest(codec=kind), tempfile.TemporaryDirectory() as tmp:
                stbl = tb.child(data, tb.child(data, tb.child(data, tracks[index], b'mdia'), b'minf'), b'stbl')
                stsd = tb.child(data, stbl, b'stsd')
                entry = next(tb.boxes(data, stsd[0] + 8, stsd[1]))
                changed = bytearray(before)
                at = moov[1] + moov[2] + entry[1] - 4
                changed[at:at + 4] = kind
                path = pathlib.Path(tmp) / 'changed-source.mp4'
                path.write_bytes(changed)
                record = copy.deepcopy(original)
                record['source'] = record['input'] = str(path)
                self.assertIsNotNone(copy_check.input_seek_operation(record))
                self.assertTrue(copy_check.oracle(record, self.tools))

    def test_serialized_iso_mutations_fail_without_changing_the_source(self):
        tb = copy_check.track_boxes
        for original in self.records:
            before = pathlib.Path(original['output']).read_bytes()
            layout = tb.top_level(original['output'])
            moov = next(b for b in layout if b[0] == b'moov')
            data = memoryview(tb.read_box(original['output'], *moov[1:], tb.MAX_MOOV))
            tracks = tb.children(data, (0, len(data)), b'trak')
            stbl = [tb.child(data, tb.child(data, tb.child(data, t, b'mdia'), b'minf'), b'stbl') for t in tracks]
            elst = tb.child(data, tb.child(data, tracks[0], b'edts'), b'elst')[0]
            ctts = tb.child(data, stbl[0], b'ctts')[0]
            stsz = tb.child(data, stbl[0], b'stsz')[0]
            stsc = tb.child(data, stbl[0], b'stsc')[0]
            stco = [tb.child(data, t, b'stco')[0] for t in stbl]
            sbgp = tb.child(data, stbl[1], b'sbgp')[0]
            sgpd = tb.child(data, stbl[1], b'sgpd')[0]
            stsd = tb.child(data, stbl[0], b'stsd')[0]
            tkhd = tb.child(data, tracks[1], b'tkhd')[0]
            delta = tb.u32(data, tb.child(data, stbl[0], b'stts')[0] + 12)
            mutations = [(elst + 12, 0), (elst + 8, tb.u32(data, elst + 8) + 1),
                         (elst + 16, 65537), (ctts + 12, delta + tb.u32(data, ctts + 20)),
                         (ctts, 2 << 24), (ctts + 12, 0xffffffff), (stsz + 8, tb.u32(data, stsz + 8) + 1),
                         (stsc + 16, 2), (stco[1] + 8, tb.u32(data, stco[0] + 8)),
                         (sbgp + 12, 1), (sgpd + 14, 0x0001fffe), (stsd + 4, 2), (tkhd + 36, 128 << 16)]
            for index, (at, value) in enumerate(mutations):
                with self.subTest(source=original['source'], mutation=index), tempfile.TemporaryDirectory() as tmp:
                    changed = bytearray(before)
                    absolute = moov[1] + moov[2] + at
                    changed[absolute:absolute + 4] = value.to_bytes(4, 'big')
                    path = pathlib.Path(tmp) / 'changed.mp4'
                    path.write_bytes(changed)
                    record = copy.deepcopy(original)
                    record['output'] = str(path)
                    self.assertTrue(copy_check.oracle(record, self.tools))
            for field in ('sample byte', 'trailing byte'):
                with self.subTest(source=original['source'], mutation=field), tempfile.TemporaryDirectory() as tmp:
                    changed = bytearray(before)
                    if field == 'sample byte':
                        mdat = next(b for b in layout if b[0] == b'mdat')
                        changed[mdat[1] + mdat[2]] ^= 1
                    else:
                        changed += b'!'
                    path = pathlib.Path(tmp) / 'changed.mp4'
                    path.write_bytes(changed)
                    record = copy.deepcopy(original)
                    record['output'] = str(path)
                    self.assertTrue(copy_check.oracle(record, self.tools))


if __name__ == "__main__":
    unittest.main()

"""tools/compat-corpus/classify.py: the rule that labels an invocation."""
import importlib.util
import pathlib
import shlex
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[3]
SPEC = importlib.util.spec_from_file_location(
    'classify', ROOT / 'tools' / 'compat-corpus' / 'classify.py')
classify = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(classify)


class Label(unittest.TestCase):
    def label(self, tool, command):
        return classify.label(tool, shlex.split(command))

    def assert_native(self, tool, command):
        self.assertEqual(self.label(tool, command), ('native', ''), command)

    def assert_out(self, tool, command, reason):
        self.assertEqual(self.label(tool, command), ('out-of-scope', reason), command)


class Ffprobe(Label):
    def test_the_common_forms_are_native(self):
        for command in (
            'in.mp4',
            '-v quiet -print_format json -show_format -show_streams in.mp4',
            '-v error -select_streams v:0 -show_entries stream=width,height -of csv=p=0 in.mkv',
            '-i in.ts -show_streams -hide_banner',
            '-show_packets -count_packets -of json in.webm',
        ):
            self.assert_native('ffprobe', command)

    def test_every_frame_query_needs_a_decoder_and_is_out_of_scope(self):
        self.assert_out('ffprobe', '-show_frames in.mp4', '-show_frames')
        self.assert_out('ffprobe', '-count_frames in.mp4', '-count_frames')
        self.assert_out('ffprobe', '-show_entries frame=pict_type in.mp4',
                        '-show_entries frame')
        self.assert_out('ffprobe', '-show_entries stream=nb_read_frames in.mp4',
                        '-show_entries nb_read_frames')

    def test_the_packet_and_stream_queries_beside_them_are_native(self):
        self.assert_native('ffprobe', '-count_packets -show_entries stream=nb_read_packets in.mp4')
        self.assert_native('ffprobe', '-show_entries format=duration:stream_tags=language in.mp4')

    def test_a_writer_step_one_does_not_print_is_out_of_scope(self):
        self.assert_out('ffprobe', '-of xml -show_streams in.mp4', '-of xml')

    def test_a_writer_setting_ffmpeg_itself_refuses(self):
        self.assert_out('ffprobe', '-of default=nw=1=nk=1 -show_streams in.mp4',
                        '-of default=nw=1=nk=1')
        self.assert_native('ffprobe', '-of default=nw=1:nk=1 -show_streams in.mp4')
        self.assert_native('ffprobe', '-output_format json -show_streams in.mp4')

    def test_a_container_coreav_does_not_read_is_out_of_scope(self):
        self.assert_out('ffprobe', '-show_streams in.avi', 'input container .avi')

    def test_a_filter_graph_is_not_a_file(self):
        self.assert_out('ffprobe', '-f lavfi -i movie=in.mp4 -show_frames', '-f lavfi')

    def test_a_stream_protocol_is_not_a_file(self):
        for url in ('rtsp://host/live', 'rtp://239.0.0.1:1234', 'udp://host:1234'):
            self.assert_out('ffprobe', f'-show_streams {url}', f'input {url}')

    def test_an_unknown_option_names_itself(self):
        self.assert_out('ffprobe', '-skip_frame nokey -show_frames in.mp4', '-skip_frame')

    def test_no_input_is_refused_not_dropped(self):
        self.assert_out('ffprobe', '-show_streams', 'no input file')

    def test_a_cut_short_line_is_unusable(self):
        self.assertEqual(self.label('ffprobe', '-show_streams -i'),
                         ('unusable', 'the line was cut short'))

    def test_an_unexpanded_option_value_is_unusable(self):
        self.assertEqual(self.label('ffprobe', '-read_intervals $t -show_packets in.mp4'),
                         ('unusable', 'an unexpanded value: -read_intervals $t'))


class Ffmpeg(Label):
    def test_the_copy_jobs_are_native(self):
        for command in (
            '-i in.mp4 -ss 00:01:00 -to 00:02:00 -c copy out.mp4',
            '-i in.mkv -c copy out.mp4',
            '-f concat -safe 0 -i list.txt -c copy out.mp4',
            '-i in.mp4 -vn -c:a copy out.m4a',
            '-i in.mp4 -c copy -movflags +faststart out.mp4',
            '-i in.mp4 -map 0:v -c copy -map_metadata -1 out.mp4',
            '-i in.mp4 -c copy -f hls -hls_time 6 out.m3u8',
            '-i in.mp4 -c copy -map 0 -f segment -segment_time 300 part%03d.mp4',
            '-f mp4 -i in.mp4 -c copy out.mp4',
        ):
            self.assert_native('ffmpeg', command)

    def test_a_transport_stream_is_read_by_probe_and_by_no_writer(self):
        # `coreav probe` reads MPEG-TS; no copy writer of step 1 does.
        self.assert_native('ffprobe', '-f mpegts -show_streams in.ts')
        self.assert_out('ffmpeg', '-f mpegts -i in.ts -c copy out.mp4', '-f mpegts')

    def test_a_time_ffmpeg_itself_refuses(self):
        self.assert_out('ffmpeg', '-i in.mp4 -to 00:84:84 -c copy out.mp4', '-to 00:84:84')
        self.assert_native('ffmpeg', '-i in.mp4 -ss 100ms -to 00:42:42.5 -c copy out.mp4')

    def test_a_container_coreav_reads_but_cannot_write(self):
        # -f matroska on the way in is read; on the way out it is refused.
        self.assert_native('ffmpeg', '-f matroska,webm -i in.mkv -c copy out.mp4')
        self.assert_out('ffmpeg', '-i in.mp4 -c copy -f matroska out', '-f matroska')

    def test_segment_numbering_is_not_an_image_sequence(self):
        self.assert_out('ffmpeg', '-i in.mp4 -c copy part%03d.mp4', 'output part%03d.mp4')

    def test_an_encode_is_out_of_scope(self):
        self.assert_out('ffmpeg', '-i in.mp4 -c:v libx264 out.mp4', '-c:v libx264')
        self.assert_out('ffmpeg', '-i in.mp4 out.mp4', 'no -c copy')

    def test_a_filter_is_out_of_scope_even_with_copy(self):
        self.assert_out('ffmpeg', '-i in.mp4 -vf scale=320:240 -c copy out.mp4', '-vf')

    def test_an_unknown_option_is_blamed_before_the_file_counts(self):
        # -bsf:v is not known, so its value would otherwise be read as a
        # second output file and the reason would blame the wrong thing.
        self.assert_out('ffmpeg', '-i in.mp4 -c copy -bsf:v h264_mp4toannexb out.ts', '-bsf:v')

    def test_muxing_two_inputs_together_is_not_a_native_job(self):
        self.assert_out('ffmpeg', '-i v.mp4 -i a.m4a -c copy out.mp4', 'more than one input')

    def test_containers_coreav_cannot_read_or_write(self):
        self.assert_out('ffmpeg', '-i in.mp3 -c copy out.mp4', 'input container .mp3')
        self.assert_out('ffmpeg', '-i in.mp4 -c copy out.mkv', 'output container .mkv')

    def test_a_device_or_an_image_sequence_is_not_a_file(self):
        self.assert_out('ffmpeg', '-i in.mp4 -c copy pipe:1', 'file pipe:1')
        self.assert_out('ffmpeg', '-f image2 -i f%03d.png -c copy out.mp4', '-f image2')

    def test_an_informational_query_is_refused_not_dropped(self):
        self.assert_out('ffmpeg', '-encoders', '-encoders')

    def test_a_cut_short_line_is_unusable(self):
        self.assertEqual(self.label('ffmpeg', '-i in.mp4 -c copy -ss'),
                         ('unusable', 'the line was cut short'))

    def test_an_unexpanded_time_is_unusable(self):
        self.assertEqual(self.label('ffmpeg', '-ss $start -i in.mp4 -c copy out.mp4'),
                         ('unusable', 'an unexpanded value: -ss $start'))


class Normalising(unittest.TestCase):
    def normalize(self, tool, command):
        return classify.normalize(tool, shlex.split(command))

    def test_the_file_names_become_placeholders(self):
        self.assertEqual(self.normalize('ffmpeg', '-i a.mp4 -ss 3 -c copy b.mp4'),
                         ['-i', 'INPUT', '-ss', '3', '-c', 'copy', 'OUTPUT'])
        self.assertEqual(self.normalize('ffprobe', '-show_streams a.mp4'),
                         ['-show_streams', 'INPUT'])
        self.assertEqual(self.normalize('ffprobe', '-i a.mp4 -show_streams'),
                         ['-i', 'INPUT', '-show_streams'])

    def test_an_unknown_options_value_keeps_its_text(self):
        # -skip_frame is not known, so `nokey` is a bare token; only the
        # last one, the input, is a file name.
        self.assertEqual(self.normalize('ffprobe', '-skip_frame nokey -show_frames a.mp4'),
                         ['-skip_frame', 'nokey', '-show_frames', 'INPUT'])

    def test_normalising_twice_changes_nothing(self):
        once = self.normalize('ffmpeg', '-i a.mp4 -i b.mp4 -c copy out.mp4')
        self.assertEqual(classify.normalize('ffmpeg', once), once)

    def test_a_home_directory_is_made_anonymous(self):
        self.assertEqual(classify.scrub('/home/alice/v.mp4'), '/home/USER/v.mp4')
        self.assertEqual(classify.scrub('C:\\Users\\Bob\\v.mp4'), 'C:\\Users\\USER\\v.mp4')
        self.assertEqual(classify.scrub('subtitles=/Users/bob/s.srt'), 'subtitles=/Users/USER/s.srt')
        self.assertEqual(classify.scrub('scale=320:240'), 'scale=320:240')

    def test_an_extension_is_read_from_a_url(self):
        self.assertEqual(classify.extension('https://host/a/b.MP4?x=1#y'), 'mp4')
        self.assertEqual(classify.extension('https://host/a/b'), '')
        self.assertEqual(classify.extension('C:\\videos\\a.mkv'), 'mkv')


if __name__ == '__main__':
    unittest.main()

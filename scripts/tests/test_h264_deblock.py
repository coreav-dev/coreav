import hashlib
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
import h264_deblock_conformance as deblock
import h264_g1_conformance as g1


def stream(enabled, alpha=0, beta=0, idc=None):
    def u(value, length):
        return f'{value:0{length}b}'

    def ue(value):
        value = f'{value + 1:b}'
        return '0' * (len(value) - 1) + value

    def se(value):
        return ue(2 * value - 1 if value > 0 else -2 * value)

    def nal(header, bits):
        bits += '1'
        bits += '0' * (-len(bits) % 8)
        data = bytes(int(bits[i:i + 8], 2) for i in range(0, len(bits), 8))
        escaped = bytearray()
        zeros = 0
        for value in data:
            if zeros == 2 and value <= 3:
                escaped.append(3)
                zeros = 0
            escaped.append(value)
            zeros = zeros + 1 if value == 0 else 0
        return b'\0\0\0\1' + bytes([header]) + escaped

    sps = u(66, 8) + u(0xc0, 8) + u(51, 8)
    sps += ''.join(ue(v) for v in (0, 0, 2, 1)) + '0' + ue(1) + ue(1) + '11'
    sps += '1' + ''.join(ue(v) for v in (0, 7, 0, 5))
    sps += '1001' + u(5, 3) + '01' + u(1, 8) * 3
    sps += '1' + ue(0) * 2 + '1' + u(1, 32) + u(60, 32) + '10000'
    pps = ue(0) * 2 + '00' + ue(0) * 3 + '0' + u(0, 2) + se(0) * 3 + '110'
    value = (0 if enabled else 1) if idc is None else idc
    idr = ue(0) + ue(7) + ue(0) + u(0, 4) + ue(0) + '00' + se(2) + ue(value)
    if value != 1:
        idr += se(alpha) + se(beta)
    return nal(0x67, sps) + nal(0x68, pps) + nal(0x65, idr)


class SyntheticFixtures(unittest.TestCase):
    def test_all_qps_and_directional_cropped_cases_are_fixed(self):
        self.assertEqual(deblock.QPS, tuple(range(10, 52)))
        self.assertEqual(deblock.CASES, (('gradient', 18, 22), ('vertical', 34, 18),
                                        ('horizontal', 18, 34), ('crossed', 32, 32)))
        for kind, width, height in deblock.CASES:
            with self.subTest(kind=kind):
                raw = deblock.synthetic(width, height, kind)
                self.assertEqual(len(raw), width * height * 3 // 2 * deblock.FRAMES)
                self.assertEqual(raw, deblock.synthetic(width, height, kind))
                frame_size = width * height * 3 // 2
                self.assertNotEqual(raw[:frame_size], raw[frame_size:2 * frame_size])
                luma_size, chroma_size = width * height, width * height // 4
                cb = raw[luma_size:luma_size + chroma_size]
                cr = raw[luma_size + chroma_size:frame_size]
                self.assertGreater(len(set(cb)), 1)
                self.assertGreater(len(set(cr)), 1)
                self.assertNotEqual(cb, cr)
        with self.assertRaises(ValueError):
            deblock.synthetic(16, 16, 'unknown')

    def test_directional_edges_and_independent_chroma_axes(self):
        for kind, width, height in (('vertical', 34, 18), ('horizontal', 18, 34)):
            with self.subTest(kind=kind):
                raw = deblock.synthetic(width, height, kind)
                luma = raw[:width * height]
                if kind == 'vertical':
                    self.assertEqual(luma[0], luma[width])
                    self.assertNotEqual(luma[3], luma[4])
                    self.assertNotEqual(luma[15], luma[16])
                else:
                    self.assertEqual(luma[0], luma[1])
                    self.assertNotEqual(luma[3 * width], luma[4 * width])
                    self.assertNotEqual(luma[15 * width], luma[16 * width])
                start, size, cw = width * height, width * height // 4, width // 2
                cb, cr = raw[start:start + size], raw[start + size:start + size * 2]
                if kind == 'vertical':
                    self.assertEqual(cb[0], cb[4])
                    self.assertNotEqual(cb[0], cb[4 * cw])
                    self.assertNotEqual(cr[0], cr[4])
                    self.assertEqual(cr[0], cr[4 * cw])
                else:
                    self.assertNotEqual(cb[0], cb[4])
                    self.assertEqual(cb[0], cb[4 * cw])
                    self.assertEqual(cr[0], cr[4])
                    self.assertNotEqual(cr[0], cr[4 * cw])

    def test_disabled_baseline_input_is_literal_prechange_fixture(self):
        raw = deblock.synthetic(18, 22, 'gradient')
        self.assertEqual(hashlib.sha256(raw).hexdigest(), deblock.BASELINE['input_sha256'])


class Report(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = pathlib.Path(self.temp.name)
        for name in ('driver', 'ffmpeg', 'ffprobe', 'ldecod'):
            (self.root / name).write_bytes(b'synthetic executable placeholder')
        self.out = self.root / 'output'
        self.argv = ['--driver', str(self.root / 'driver'), '--ffmpeg', str(self.root / 'ffmpeg'),
                     '--jm', str(self.root / 'ldecod'), '--output', str(self.out)]
        self.environment = patch.dict(os.environ, {'GITHUB_ACTIONS': 'true'})
        self.environment.start()
        self.addCleanup(self.environment.stop)
        self.platform = patch.object(deblock.sys, 'platform', 'linux')
        self.platform.start()
        self.addCleanup(self.platform.stop)
        self.pin = patch.object(g1.test_oracle, 'verify_bin', return_value=[])
        self.pin.start()
        self.addCleanup(self.pin.stop)

    def compare(self, driver, ffmpeg, jm, raw, output, width, height, qp, frames, **kwargs):
        if not kwargs.get('deblocking', False):
            self.assertEqual((width, height, qp, frames), (18, 22, 28, 3))
            return dict(deblock.BASELINE, conforming=True)
        return dict(width=width, height=height, qp=qp, frames=frames,
                    reconstruction_sha256='f' * 64, stream_sha256='e' * 64, conforming=True)

    def report(self):
        return json.loads((self.out / 'report.json').read_text())

    def test_complete_matrix_uses_enabled_compare_and_disabled_baseline_first(self):
        with patch.object(g1, 'compare', side_effect=self.compare) as compare:
            deblock.main(self.argv)
        report = self.report()
        self.assertTrue(report['complete'])
        self.assertEqual(report['expected_cells'], 168)
        self.assertEqual(len(report['cells']), 168)
        self.assertEqual(compare.call_count, 169)
        self.assertNotIn('deblocking', compare.call_args_list[0].kwargs)
        self.assertTrue(all(c.kwargs['deblocking'] is True for c in compare.call_args_list[1:]))
        self.assertEqual({(c['fixture'], c['qp']) for c in report['cells']},
                         {(kind, qp) for kind, _, _ in deblock.CASES for qp in range(10, 52)})
        self.assertTrue(report['witness']['changed'])
        self.assertEqual(report['witness']['qp'], 28)
        self.assertEqual(set(report['tools']), {'driver', 'ffmpeg', 'ffprobe', 'jm'})

    def test_filter_that_never_runs_fails_and_retains_witness(self):
        def unchanged(*args, **kwargs):
            result = self.compare(*args, **kwargs)
            result['reconstruction_sha256'] = deblock.BASELINE['reconstruction_sha256']
            return result

        with patch.object(g1, 'compare', side_effect=unchanged):
            with self.assertRaisesRegex(ValueError, 'changes reconstructed samples'):
                deblock.main(self.argv)
        report = self.report()
        self.assertFalse(report['complete'])
        self.assertFalse(report['witness']['changed'])
        self.assertEqual(len(report['cells']), 19)

    def test_changed_disabled_hash_fails_before_enabled_matrix(self):
        for field in ('stream_sha256', 'reconstruction_sha256'):
            with self.subTest(field=field):
                self.out = self.root / field
                self.argv[-1] = str(self.out)
                result = dict(deblock.BASELINE, **{field: 'f' * 64})
                with patch.object(g1, 'compare', return_value=result) as compare:
                    with self.assertRaisesRegex(ValueError, 'disabled baseline'):
                        deblock.main(self.argv)
                self.assertEqual(compare.call_count, 1)
                self.assertFalse(self.report()['complete'])
                self.assertEqual(self.report()['cells'], [])

    def test_timeout_preserves_completed_cells_and_incomplete_report(self):
        calls = 0

        def interrupted(*args, **kwargs):
            nonlocal calls
            calls += 1
            if calls == 4:
                raise subprocess.TimeoutExpired(['decoder'], 180)
            return self.compare(*args, **kwargs)

        with patch.object(g1, 'compare', side_effect=interrupted):
            with self.assertRaises(subprocess.TimeoutExpired):
                deblock.main(self.argv)
        self.assertFalse(self.report()['complete'])
        self.assertEqual(len(self.report()['cells']), 2)

    def test_pin_and_provenance_failures_retain_incomplete_report(self):
        with patch.object(g1.test_oracle, 'verify_bin', return_value=['wrong oracle']):
            with self.assertRaisesRegex(ValueError, 'wrong oracle'):
                deblock.main(self.argv)
        self.assertFalse(self.report()['complete'])
        self.argv[-1] = str(self.root / 'missing-tool')
        self.out = self.root / 'missing-tool'
        (self.root / 'driver').unlink()
        with self.assertRaises(FileNotFoundError):
            deblock.main(self.argv)
        self.assertFalse(self.report()['complete'])

    def test_developer_machine_refuses_without_opening_tools(self):
        with patch.dict(os.environ, {'GITHUB_ACTIONS': 'false'}), patch.object(g1, 'compare') as compare:
            with self.assertRaisesRegex(ValueError, 'only in Linux Actions'):
                deblock.main(self.argv)
        compare.assert_not_called()
        self.assertFalse(self.report()['complete'])


class SharedCompare(unittest.TestCase):
    def test_enabled_diagnostic_check_rejects_zero_exit_warnings(self):
        for stdout, stderr in ((b'', b'warning from decoder'), (b'Warning: damaged picture\n', b''), (b'Error: concealment\n', b'')):
            with self.subTest(stdout=stdout, stderr=stderr), tempfile.TemporaryDirectory() as temp:
                result = subprocess.CompletedProcess(['decoder'], 0, stdout, stderr)
                with patch.object(g1.subprocess, 'run', return_value=result):
                    with self.assertRaisesRegex(ValueError, 'decoder diagnostics'):
                        g1.run(['decoder'], cwd=temp, reject_diagnostics=True)
                    self.assertEqual(g1.run(['decoder'], cwd=temp), stdout)
                records = list(pathlib.Path(temp).glob('process-*.json'))
                self.assertEqual(len(records), 2)
                self.assertEqual(json.loads(records[0].read_text())['exit_code'], 0)

    def test_enabled_syntax_requires_idc_zero_and_zero_offsets(self):
        g1.inspect(stream(True), 18, 22, 28, 1, colour=deblock.COLOUR, deblocking=True)
        g1.inspect(stream(False), 18, 22, 28, 1)
        for enabled, alpha, beta, idc in ((False, 0, 0, 1), (True, 1, 0, 0),
                                           (True, 0, -1, 0), (True, 0, 0, 2)):
            with self.subTest(alpha=alpha, beta=beta, idc=idc), self.assertRaises(ValueError):
                g1.inspect(stream(enabled, alpha, beta, idc), 18, 22, 28, 1, deblocking=True)
        with self.assertRaises(ValueError):
            g1.inspect(stream(True), 18, 22, 28, 1)

    def test_native_planes_command_flag_failures_and_finite_timeouts(self):
        cases = [(True, None), (False, None)]
        cases += [(True, f'{decoder}-{mutation}') for decoder in ('coreav', 'ffmpeg', 'jm')
                  for mutation in ('y', 'cb', 'cr', 'truncated', 'extra')]
        cases += [(True, fault) for fault in ('format', 'range', 'ffmpeg', 'jm', 'timeout')]
        for enabled, fault in cases:
            with self.subTest(enabled=enabled, fault=fault), tempfile.TemporaryDirectory() as temp:
                root = pathlib.Path(temp)
                size = 18 * 22 * 3 // 2
                samples = bytes([64]) * (18 * 22) + bytes([96]) * (18 * 22 // 4) + bytes([160]) * (18 * 22 // 4)
                commands = []

                def decoded(decoder):
                    changed = bytearray(samples)
                    for component, at in (('y', 0), ('cb', 18 * 22), ('cr', 18 * 22 * 5 // 4)):
                        if fault == f'{decoder}-{component}':
                            changed[at] ^= 1
                    if fault == f'{decoder}-truncated':
                        changed = changed[:-1]
                    if fault == f'{decoder}-extra':
                        changed.extend(samples)
                    return changed

                def process(command, **kwargs):
                    self.assertEqual(kwargs['timeout'], 180)
                    commands.append(command)
                    name = pathlib.Path(command[0]).name
                    stdout, stderr, code = b'', b'', 0
                    if name == 'driver':
                        self.assertEqual(command[9:], ['false', '1', '1', '1', '0'] + (['true'] if enabled else []))
                        pathlib.Path(command[6]).write_bytes(stream(enabled))
                        pathlib.Path(command[7]).write_bytes(samples)
                        pathlib.Path(command[8]).write_bytes(decoded('coreav'))
                    elif name == 'ffprobe':
                        info = dict(width=18, height=22, pix_fmt='yuv420p', color_range='tv')
                        if fault == 'format':
                            info['pix_fmt'] = 'rgb24'
                        if fault == 'range':
                            info['color_range'] = 'pc'
                        stdout = json.dumps({'streams': [info]}).encode()
                    elif name == 'ffmpeg':
                        self.assertEqual(command[command.index('-pix_fmt') + 1], '+yuv420p')
                        pathlib.Path(command[-1]).write_bytes(decoded('ffmpeg'))
                        if fault == 'ffmpeg':
                            code, stderr = 1, b'oracle failed'
                        if fault == 'timeout':
                            raise subprocess.TimeoutExpired(command, 180, output=b'partial', stderr=b'timeout diagnostic')
                    elif name == 'ldecod':
                        pathlib.Path(command[command.index('-o') + 1]).write_bytes(decoded('jm'))
                        if fault == 'jm':
                            code, stderr = 1, b'oracle failed'
                    else:
                        self.fail(f'unexpected process {name}')
                    return subprocess.CompletedProcess(command, code, stdout=stdout, stderr=stderr)

                with patch.object(g1.subprocess, 'run', side_effect=process):
                    args = (root / 'driver', root / 'ffmpeg', root / 'ldecod', root / 'input',
                            root / 'cell', 18, 22, 28, 1)
                    options = dict(colour=deblock.COLOUR, deblocking=True) if enabled else {}
                    if fault is None:
                        result = g1.compare(*args, **options)
                        self.assertTrue(result['conforming'])
                        self.assertEqual(result['reconstruction_sha256'], hashlib.sha256(samples).hexdigest())
                        self.assertEqual(len(commands), 4)
                        g1.same_planes(root / 'cell/encoder.yuv', root / 'cell/jm.yuv', size)
                    else:
                        with self.assertRaises((ValueError, subprocess.SubprocessError)):
                            g1.compare(*args, **options)
                if fault == 'timeout':
                    records = [json.loads(p.read_text()) for p in (root / 'cell').glob('process-*.json')]
                    record = next(r for r in records if r['timed_out'])
                    self.assertEqual(record['stdout'], 'partial')
                    self.assertEqual(record['stderr'], 'timeout diagnostic')


class Workflow(unittest.TestCase):
    def test_new_coverage_is_triggered_run_separately_and_uploaded(self):
        root = pathlib.Path(__file__).resolve().parents[2]
        workflow = (root / '.github/workflows/h264-encoder.yml').read_text()
        for event, following in (('push', 'pull_request'), ('pull_request', 'merge_group')):
            block = workflow.split(f'  {event}:', 1)[1].split(f'  {following}:', 1)[0]
            for path in ('scripts/h264_deblock_conformance.py', 'scripts/tests/test_h264_deblock.py'):
                self.assertIn(f'      - {path}\n', block)
        self.assertIn("python3 -I -m unittest discover -s scripts/tests -p 'test_h264_g1*.py'", workflow)
        self.assertIn("python3 -I -m unittest discover -s scripts/tests -p 'test_h264_deblock.py'", workflow)
        self.assertIn('cargo build --locked --release -p coreav-h264-encoder --features full --example g1', workflow)
        old = workflow.split('      - name: Compare every visible plane with all three decoders', 1)[1].split('      - name:', 1)[0]
        self.assertIn('python3 -I scripts/h264_g1_conformance.py', old)
        self.assertIn('--fixtures "$RUNNER_TEMP/g1-existing-fixtures"', old)
        self.assertIn('--output "$RUNNER_TEMP/g1-conformance"', old)
        self.assertNotIn('deblock', old)
        new = workflow.split('      - name: Compare enabled deblocking at every QP', 1)[1].split('      - uses:', 1)[0]
        self.assertIn('python3 -I scripts/h264_deblock_conformance.py', new)
        self.assertIn('--output "$RUNNER_TEMP/deblock-conformance"', new)
        self.assertNotIn('--fixtures', new)
        upload = workflow.split('      - uses: actions/upload-artifact@', 1)[1]
        self.assertIn('if: always()', upload)
        self.assertIn('${{ runner.temp }}/g1-conformance', upload)
        self.assertIn('${{ runner.temp }}/deblock-conformance', upload)


if __name__ == '__main__':
    unittest.main()

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
import h264_p_conformance as p


def u(value, length):
    return f'{value:0{length}b}'


def ue(value):
    bits = f'{value + 1:b}'
    return '0' * (len(bits) - 1) + bits


def se(value):
    return ue(2 * value - 1 if value > 0 else -2 * value)


def nal(header, bits):
    bits += '1'
    bits += '0' * (-len(bits) % 8)
    data = bytes(int(bits[i:i + 8], 2) for i in range(0, len(bits), 8))
    escaped, zeros = bytearray(), 0
    for value in data:
        if zeros == 2 and value <= 3:
            escaped.append(3)
            zeros = 0
        escaped.append(value)
        zeros = zeros + 1 if value == 0 else 0
    return b'\0\0\0\1' + bytes([header]) + escaped


def parameters(width=16, height=16, **changes):
    fields = dict(profile=66, constraints=0xc0, level=51, framebits=0, poc=2,
                  refs=1, full=0, colour=1, cabac=0, constrained=1,
                  crop_right=((width + 15) // 16 * 16 - width) // 2,
                  crop_bottom=((height + 15) // 16 * 16 - height) // 2)
    fields.update(changes)
    f = fields
    sps = u(f['profile'], 8) + u(f['constraints'], 8) + u(f['level'], 8)
    sps += ue(0) + ue(f['framebits']) + ue(f['poc']) + ue(f['refs']) + '0'
    sps += ue((width + 15) // 16 - 1) + ue((height + 15) // 16 - 1) + '11'
    sps += '1' + ue(0) + ue(f['crop_right']) + ue(0) + ue(f['crop_bottom'])
    sps += '1001' + u(5, 3) + u(f['full'], 1) + '1' + u(f['colour'], 8) * 3
    sps += '1' + ue(0) * 2 + '1' + u(1, 32) + u(60, 32) + '10000'
    pps = ue(0) * 2 + u(f['cabac'], 1) + '0' + ue(0) * 3 + '000' + se(0) * 3
    pps += '1' + u(f['constrained'], 1) + '0'
    return nal(0x67, sps) + nal(0x68, pps)


def intra(inter=False, i4=False):
    if i4:
        return ue(5 if inter else 0) + '1' * 16 + ue(0) + ue(3)
    return ue(8 if inter else 3) + ue(0) + se(0) + '1'


def inter(mbtype=0, subtypes=(0, 0, 0, 0), mvd=0, cbp=0, delta=0, residual=''):
    bits = ue(mbtype)
    if mbtype in (3, 4):
        bits += ''.join(ue(s) for s in subtypes)
        count = sum((1, 2, 2, 4)[s] for s in subtypes)
    else:
        count = (1, 2, 2)[mbtype]
    bits += (se(mvd) + se(0)) * count + ue(cbp)
    return bits + (se(delta) + residual if cbp else '')


def payload(items):
    bits, skips = '', 0
    for item in items:
        if item is None:
            skips += 1
        else:
            bits += ue(skips) + item
            skips = 0
    return bits + (ue(skips) if skips else '')


def slice_nal(frame, body, interval=20, qp=28, enabled=False, **changes):
    idr = frame % interval == 0
    f = dict(slice_type=7 if idr else 5, number=frame % interval % 16,
             id=frame // interval % 2, marking=0, override=0, modification=0,
             delta=qp - 26, idc=0 if enabled else 1, alpha=0, beta=0)
    f.update(changes)
    bits = ue(0) + ue(f['slice_type']) + ue(0) + u(f['number'], 4)
    bits += ue(f['id']) + '00' if idr else u(f['override'], 1) + u(f['modification'], 1) + u(f['marking'], 1)
    bits += se(f['delta']) + ue(f['idc'])
    if f['idc'] != 1:
        bits += se(f['alpha']) + se(f['beta'])
    return nal(0x65 if idr else 0x61, bits + body)


def stream(pictures, width=16, height=16, interval=20, qp=28, enabled=False):
    params = parameters(width, height)
    result = bytearray()
    for frame, items in enumerate(pictures):
        body = ''.join(items) if frame % interval == 0 else payload(items)
        result.extend(params + slice_nal(frame, body, interval, qp, enabled))
    return bytes(result)


def write_forced(directory):
    directory.mkdir()
    rows = ['layout\tqp\tdeblocking']
    layouts = [inter(0), inter(1), inter(2)]
    layouts += [inter(3, (subtype,) * 4) for subtype in range(4)]
    layouts += [inter(3, (0, 1, 2, 3))]
    for layout, bits in enumerate(layouts):
        for qp in (10, 16, 28, 40, 51):
            for enabled in (0, 1):
                rows.append(f'{layout}\t{qp}\t{enabled}')
                stem = directory / f'forced-{qp}-{enabled}-{layout}'
                stem.with_suffix('.h264').write_bytes(stream([[intra()], [bits]], qp=qp, enabled=bool(enabled), interval=32))
                stem.with_suffix('.yuv').write_bytes(bytes([128]) * 768)
    (directory / 'manifest.tsv').write_text('\n'.join(rows) + '\n')


class Syntax(unittest.TestCase):
    def inspect(self, pictures, width=16, height=16, **kwargs):
        data = stream(pictures, width, height, **kwargs)
        return p.inspect(data, width, height, kwargs.get('qp', 28), len(pictures),
                         kwargs.get('enabled', False), kwargs.get('interval', 20))

    def test_all_types_and_subtypes_are_read_from_syntax(self):
        cases = [(inter(0), 1, 'P_L0_16x16'), (inter(1), 2, 'P_L0_16x8'),
                 (inter(2), 2, 'P_L0_8x16'), (intra(True), 0, 'I_16x16'),
                 (intra(True, True), 0, 'I_4x4')]
        for subtype, count in enumerate((4, 8, 8, 16)):
            for mbtype in (3, 4):
                cases.append((inter(mbtype, (subtype,) * 4), count, 'P_8x8'))
        for bits, count, mode in cases:
            with self.subTest(count=count, mode=mode):
                result = self.inspect([[intra()], [bits]])
                self.assertEqual(result[1]['mvcnt'], [count])
                self.assertEqual(result[1]['modes'][mode], 1)
                self.assertGreater(result[1]['macroblock_bits'][0], 0)
        i4_explicit = ue(5) + ('0' + u(2, 3)) * 16 + ue(0) + ue(3)
        self.inspect([[intra()], [i4_explicit]])

    def test_equal_zero_vectors_and_uniform_prediction_do_not_discount_partitions(self):
        eight = inter(3, (1,) * 4)
        sixteen = inter(3, (3,) * 4)
        result = self.inspect([[intra(), intra()], [eight, eight]], width=32)
        self.assertEqual(result[1]['mvcnt'], [8, 8])
        self.assertEqual(self.inspect([[intra(), intra()], [sixteen, intra(True)]], width=32)[1]['mvcnt'], [16, 0])
        with self.assertRaisesRegex(ValueError, 'MaxMvsPer2Mb'):
            self.inspect([[intra(), intra()], [sixteen, None]], width=32)
        thirteen = inter(3, (3, 3, 3, 0))
        four = inter(3, (0,) * 4)
        with self.assertRaisesRegex(ValueError, 'MaxMvsPer2Mb'):
            self.inspect([[intra(), intra()], [thirteen, four]], width=32)

    def test_cross_picture_pair_is_not_reset_at_the_slice_boundary(self):
        sixteen = inter(3, (3,) * 4)
        self.assertEqual(self.inspect([[intra()], [sixteen], [intra(True)]])[2]['mvcnt'], [0])
        with self.assertRaisesRegex(ValueError, 'MaxMvsPer2Mb'):
            self.inspect([[intra()], [sixteen], [None]])
        eight = inter(3, (2,) * 4)
        self.assertEqual(self.inspect([[intra()], [eight], [eight]])[2]['mvcnt'], [8])
        result = self.inspect([[intra()], [sixteen], [intra()], [None]], interval=2)
        self.assertEqual([f['mvcnt'] for f in result], [[0], [16], [0], [1]])

    def test_row_transition_carries_count(self):
        with self.assertRaisesRegex(ValueError, 'MaxMvsPer2Mb'):
            self.inspect([[intra()] * 4, [intra(True), inter(3, (3,) * 4), None, intra(True)]], width=32, height=32)

    def test_skip_runs_include_zero_runs_interior_and_tail(self):
        result = self.inspect([[intra()] * 4, [None, inter(), None, None]], width=32, height=32)
        self.assertEqual(result[1]['mvcnt'], [1] * 4)
        self.assertEqual(result[1]['modes']['P_Skip'], 3)
        self.assertEqual(result[1]['macroblock_bits'][0], 0)
        self.assertEqual(self.inspect([[intra()] * 4, [None] * 4], width=32, height=32)[1]['modes'], {'P_Skip': 4})
        for bad in (ue(5), ue(0), ue(3)):
            data = parameters(32, 32) + slice_nal(0, intra() * 4)
            data += parameters(32, 32) + slice_nal(1, bad)
            with self.subTest(bad=bad), self.assertRaises(ValueError):
                p.inspect(data, 32, 32, 28, 2)

    def test_gop_reset_wrap_and_cropped_geometry(self):
        result = self.inspect([[intra()]] + [[None]] * 18)
        self.assertEqual([f['frame_num'] for f in result], list(range(16)) + [0, 1, 2])
        pictures = [[intra()] * 4 if f % 3 == 0 else [None] * 4 for f in range(7)]
        result = self.inspect(pictures, width=18, height=22, interval=3, enabled=True, qp=10)
        self.assertEqual([f['idr'] for f in result], [True, False, False, True, False, False, True])

    def test_mvd_syntax_bounds_are_not_derived_vector_bounds(self):
        for value in (-32768, -8193, 8192, 32767):
            self.inspect([[intra()], [inter(mvd=value)]])
        for value in (-32769, 32768):
            with self.assertRaisesRegex(ValueError, 'mvd_l0 syntax'):
                self.inspect([[intra()], [inter(mvd=value)]])

    def test_fixed_qp_and_truncated_residual_headers(self):
        for qp in (10, 16, 28, 40, 51):
            self.inspect([[intra()], [inter()]], qp=qp)
        for bits in (inter(cbp=1, delta=1), inter(cbp=1, delta=-1), inter(cbp=1),
                     ue(30), ue(31), ue(48), ue(3) + ue(4),
                     ue(5) + '1' * 16 + ue(4), inter()[:-1]):
            with self.subTest(bits=bits), self.assertRaises(ValueError):
                self.inspect([[intra()], [bits]])
        with self.assertRaises(ValueError):
            self.inspect([[ue(25)]])
        for qp in (9, 52):
            with self.assertRaises(ValueError):
                self.inspect([[intra()]], qp=qp)

    def test_wrong_headers_parameter_bytes_and_trailing_payload(self):
        base = parameters() + slice_nal(0, intra())
        for change in (dict(delta=1), dict(number=1), dict(slice_type=2), dict(id=1), dict(idc=2)):
            with self.subTest(change=change), self.assertRaises(ValueError):
                p.inspect(parameters() + slice_nal(0, intra(), **change), 16, 16, 28, 1)
        for change in (dict(marking=1), dict(override=1), dict(modification=1), dict(number=0)):
            with self.subTest(change=change), self.assertRaises(ValueError):
                p.inspect(base + parameters() + slice_nal(1, payload([inter()]), **change), 16, 16, 28, 2)
        for change in (dict(profile=77), dict(constraints=0x80), dict(level=50), dict(framebits=1),
                       dict(poc=0), dict(refs=2), dict(full=1), dict(colour=6), dict(cabac=1),
                       dict(constrained=0), dict(crop_right=1)):
            with self.subTest(change=change), self.assertRaises(ValueError):
                p.inspect(parameters(**change) + slice_nal(0, intra()), 16, 16, 28, 1)
        changed = base + parameters(full=1) + slice_nal(1, payload([inter()]))
        with self.assertRaisesRegex(ValueError, 'identical parameter bytes'):
            p.inspect(changed, 16, 16, 28, 2)
        for bad in (base[:-1], base + b'\0', base + b'\0\0\0\1', base * 2,
                    parameters() + slice_nal(0, intra() + '0')):
            with self.subTest(bad=bad[-8:]), self.assertRaises(ValueError):
                p.inspect(bad, 16, 16, 28, 1)
        for width, height in ((17, 16), (14, 16), (16, 1082)):
            with self.assertRaises(ValueError):
                p.inspect(base, width, height, 28, 1)
        for change in (dict(alpha=1), dict(beta=-1), dict(idc=1)):
            with self.subTest(change=change), self.assertRaises(ValueError):
                p.inspect(parameters() + slice_nal(0, intra(), enabled=True, **change), 16, 16, 28, 1, True)
        nals = p.g1.units(base)
        for indices in ((1, 0, 2), (0, 2, 1)):
            data = b''.join(b'\0\0\0\1' + nals[i] for i in indices)
            with self.assertRaisesRegex(ValueError, 'NAL headers'):
                p.inspect(data, 16, 16, 28, 1)
        for header in (0x01, 0x41, 0x66, 0xe5):
            data = parameters() + b'\0\0\0\1' + bytes([header]) + nals[2][1:]
            with self.subTest(header=header), self.assertRaisesRegex(ValueError, 'NAL headers'):
                p.inspect(data, 16, 16, 28, 1)
        for raw in (b'\x61\0\0\x03\x04\x80', b'\x61\0\0\x03', b'\x61\0\0\x01\x80'):
            with self.assertRaisesRegex(ValueError, 'emulation'):
                p.rbsp(raw)


class Residuals(unittest.TestCase):
    def setUp(self):
        self.tables = p.vlc_tables()

    def test_known_specification_codewords_and_finite_literal_reader(self):
        self.assertEqual(self.tables['TOKENS'][0], (33, 98, 484, 102, 34))
        self.assertEqual(self.tables['CHROMA_ZEROS'][0][:4], (33, 34, 35, 3))
        with tempfile.TemporaryDirectory() as temp:
            path = pathlib.Path(temp) / 'bad.rs'
            source = (p.ROOT / 'crates/coreav-h264-encoder/src/vlc.rs').read_text()
            for text in (source.replace('[33, 98,', '[danger(), 98,', 1),
                         source.replace('[33, 98,', '[33,', 1), source + source,
                         source.replace('[33, 98,', '[32, 98,', 1), 'x' * 32769):
                path.write_text(text)
                with self.assertRaises(ValueError):
                    p.vlc_tables(path)

    def block(self, bits, maximum=16, nc=0):
        r = p.rbsp(nal(0x61, bits)[4:])
        total = p.residual_block(r, maximum, nc, self.tables)
        r.finish()
        return total

    def test_zero_tokens_all_contexts_and_chroma_dc(self):
        for nc, bits in ((0, '1'), (1, '1'), (2, '11'), (3, '11'), (4, '1111'),
                         (7, '1111'), (8, '000011'), (16, '000011'), (-1, '01')):
            self.assertEqual(self.block(bits, 4 if nc == -1 else 16, nc), 0)

    def test_nonzero_tokens_levels_zeros_runs_and_profile_escape(self):
        self.assertEqual(self.block('01' + '0' + '1'), 1)
        self.assertEqual(self.block('001' + '01' + '110' + '1'), 2)
        self.assertEqual(self.block('1' + '0' + '1', 4, -1), 1)
        self.assertEqual(self.block('000101' + '1' + '1'), 1)
        for prefix in (14, 15):
            bits = '000101' + '0' * prefix + '1' + '0' * (4 if prefix == 14 else 12) + '1'
            self.assertEqual(self.block(bits), 1)
        for bits, maximum in (('000101' + '0' * 16 + '1', 16),
                              ('01' + '0' + '000000001', 15), ('0' * 16, 16),
                              ('01', 16), ('000101', 16)):
            with self.subTest(bits=bits), self.assertRaises(ValueError):
                self.block(bits, maximum)

    def test_dense_sixteen_token_for_ac_is_rejected(self):
        with self.assertRaisesRegex(ValueError, 'coefficient token'):
            self.block('111111', 15, 8)

    def test_frame_grid_context_and_chroma_dc_order(self):
        luma = '01' + '0' + '1'
        body = inter(cbp=2, residual=luma + '1' * 3)
        pictures = p.inspect(stream([[intra()], [body]]), 16, 16, 28, 2)
        self.assertEqual(pictures[1]['mvcnt'], [1])
        chroma = inter(cbp=1, residual='1' + '0' + '1' + '01')
        p.inspect(stream([[intra()], [chroma]]), 16, 16, 28, 2)
        both = inter(cbp=6, residual='01' * 2 + '1' * 8)
        p.inspect(stream([[intra()], [both]]), 16, 16, 28, 2)
        three = '00011' + '000' + '0101'
        contextual = inter(cbp=2, residual=three + '11' + '11' + '1')
        p.inspect(stream([[intra()], [contextual]]), 16, 16, 28, 2)
        dense_first = '0000000000001000' + '000' + '1' + '10' * 12
        dense_next = '111111' + '000' + '1' + '10' * 12
        dense = ue(0) + '1' * 16 + ue(0) + ue(2) + se(0) + dense_first + dense_next * 15
        adjacent_dc = ue(3) + ue(0) + se(0) + '000011'
        pictures = [[dense, adjacent_dc], [None, intra(True)]]
        parsed = p.inspect(stream(pictures, width=32), 32, 16, 28, 2)
        self.assertEqual(parsed[0]['modes'], {'I_4x4': 1, 'I_16x16': 1})
        self.assertEqual(parsed[1]['modes'], {'P_Skip': 1, 'I_16x16': 1})
        grid = p.Picture(32, 32, self.tables, 0)
        for component, stride in ((0, 8), (1, 4), (2, 4)):
            grid.grids[component][stride] = 2
            grid.grids[component][1] = 5
            self.assertEqual(grid.context(component, 1, 1), 4)
            self.assertEqual(grid.context(component, 0, 2), 2)

    def test_macroblock_layer_bit_ceiling_excludes_skip_run(self):
        picture = p.Picture(16, 16, self.tables, 0)
        picture.admit(0, 'I_16x16', 3200)
        with self.assertRaisesRegex(ValueError, 'macroblock_layer bits'):
            picture.admit(0, 'I_16x16', 3201)
        ordinary = inter(3, (3,) * 4)
        parsed = p.inspect(stream([[intra()], [ordinary]]), 16, 16, 28, 2)
        self.assertEqual(parsed[1]['macroblock_bits'], [len(ordinary)])
        self.assertNotEqual(parsed[1]['macroblock_bits'], [len(payload([ordinary]))])
        large_levels = ('0' * 15 + '1' + '1' * 12) * 16
        dense = '0000000000000100' + large_levels + ('111100' + large_levels) * 7
        oversized = inter(cbp=7, residual=dense)
        self.assertEqual(len(oversized), 3653)
        with self.assertRaisesRegex(ValueError, 'macroblock_layer bits'):
            p.inspect(stream([[intra()], [oversized]]), 16, 16, 28, 2)


class Synthetic(unittest.TestCase):
    def test_roster_is_tiny_bounded_and_explicit(self):
        roster = p.cells()
        self.assertEqual(len(roster), 46)
        self.assertEqual({c['qp'] for c in roster}, {10, 16, 28, 40, 51})
        self.assertEqual({tuple(c['phase']) for c in roster if c['kind'] == 'phase'},
                         {(x, y) for x in range(4) for y in range(4)})
        self.assertTrue(all(((c['width'] + 15) // 16) * ((c['height'] + 15) // 16) <= 4 for c in roster))
        self.assertTrue(any(c['frames'] >= 19 and c['interval'] >= 20 for c in roster))
        self.assertTrue(any(c['interval'] == 3 and c['frames'] == 7 for c in roster))
        for cell in roster:
            self.assertIn(dict(cell, deblocking=not cell['deblocking']), roster)

    def test_interpolation_has_independent_analytic_examples_and_edge_extension(self):
        ramp = lambda x, y: 80 + 4 * x + 8 * y
        for x in range(4):
            for y in range(4):
                self.assertEqual(p.luma_sample(ramp, x, y), 80 + x + y * 2)
        self.assertEqual(p.luma_sample(lambda x, y: 100, -3, -7), 100)
        self.assertEqual(p.chroma_sample(ramp, -1, -1), 79)
        impulse = lambda x, y: 255 if (x, y) == (0, 0) else 0
        self.assertEqual(p.luma_sample(impulse, 2, 0), 159)
        self.assertEqual(p.luma_sample(impulse, 2, 2), 100)
        self.assertEqual(p.chroma_sample(impulse, 4, 4), 64)

    def test_generated_planes_are_deterministic_synthetic_and_independent(self):
        for kind in ('still', 'reset', 'integer', 'cropped', 'chroma', 'opposing', 'abrupt', 'phase'):
            raw = p.synthetic(18, 22, 2, kind, (1, 3))
            self.assertEqual(len(raw), 18 * 22 * 3)
            self.assertEqual(raw, p.synthetic(18, 22, 2, kind, (1, 3)))
            size = 18 * 22 * 3 // 2
            if kind in ('still', 'reset'):
                self.assertEqual(raw[:size], raw[size:])
            else:
                self.assertNotEqual(raw[:size], raw[size:])
            self.assertNotEqual(raw[396:495], raw[495:594])


class Compare(unittest.TestCase):
    def test_strict_zero_exit_diagnostics_are_retained(self):
        for stdout, stderr in ((b'Warning: concealment', b''), (b'', b'warning'),
                               (b'error without colon', b''), (b'Corrupted picture', b'')):
            with tempfile.TemporaryDirectory() as temp, self.subTest(stdout=stdout, stderr=stderr):
                with patch.object(p.g1.subprocess, 'run', return_value=subprocess.CompletedProcess(['decoder'], 0, stdout, stderr)):
                    with self.assertRaisesRegex(ValueError, 'diagnostics'):
                        p.run(['decoder'], temp)
                record = json.loads(next(pathlib.Path(temp).glob('process-*.json')).read_text())
                self.assertEqual(record['exit_code'], 0)

    def test_native_shape_digest_metadata_and_complete_order(self):
        faults = [None, 'format', 'range', 'width', 'metadata', 'count', 'order', 'timeout']
        faults += [f'{decoder}-{part}' for decoder in ('coreav', 'ffmpeg', 'jm')
                   for part in ('y', 'cb', 'cr', 'extra', 'truncated', 'warning', 'exit')]
        for fault in faults:
            with self.subTest(fault=fault), tempfile.TemporaryDirectory() as temp:
                root = pathlib.Path(temp)
                frame = bytes([128]) * 256 + bytes([96]) * 64 + bytes([160]) * 64
                samples = frame + bytes(v ^ 1 for v in frame)
                (root / 'input').write_bytes(samples)
                commands = []

                def decoded(name):
                    value = bytearray(samples)
                    for part, at in (('y', 0), ('cb', 256), ('cr', 320)):
                        if fault == f'{name}-{part}':
                            value[at] ^= 1
                    if fault == f'{name}-extra':
                        value += frame
                    if fault == f'{name}-truncated':
                        value.pop()
                    if fault == 'order' and name == 'ffmpeg':
                        value = value[384:] + value[:384]
                    return value

                def process(command, **kwargs):
                    self.assertEqual(kwargs['timeout'], 180)
                    commands.append(command)
                    executable = pathlib.Path(command[0]).name
                    name = {'driver': 'coreav', 'ldecod': 'jm'}.get(executable, executable)
                    stdout, stderr, code = b'', b'', 0
                    if executable == 'driver':
                        self.assertEqual(len(command), 11)
                        self.assertEqual(command[9:], ['false', '20'])
                        pathlib.Path(command[6]).write_bytes(stream([[intra()], [inter()]]))
                        pathlib.Path(command[7]).write_bytes(samples)
                        pathlib.Path(command[8]).write_bytes(decoded(name))
                    elif executable == 'ffprobe':
                        info = dict(width=16, height=16, pix_fmt='yuv420p', color_range='tv',
                                    color_space='bt709', color_transfer='bt709', color_primaries='bt709',
                                    chroma_location='left', profile='Constrained Baseline', level=51, nb_read_frames='2')
                        for trigger, key, value in (('format', 'pix_fmt', 'rgb24'), ('range', 'color_range', 'pc'),
                                                    ('width', 'width', 18), ('metadata', 'color_space', 'bt470bg'),
                                                    ('count', 'nb_read_frames', '1')):
                            if fault == trigger:
                                info[key] = value
                        stdout = json.dumps({'streams': [info]}).encode()
                    elif executable == 'ffmpeg':
                        self.assertEqual(command[command.index('-pix_fmt') + 1], '+yuv420p')
                        self.assertIn('-xerror', command)
                        self.assertIn('passthrough', command)
                        pathlib.Path(command[-1]).write_bytes(decoded(name))
                        if fault == 'timeout':
                            raise subprocess.TimeoutExpired(command, 180, output=b'partial', stderr=b'interrupted')
                    elif executable == 'ldecod':
                        pathlib.Path(command[command.index('-o') + 1]).write_bytes(decoded(name))
                    else:
                        self.fail(executable)
                    if fault == f'{name}-warning':
                        stdout = b'Warning: synthetic damage'
                    if fault == f'{name}-exit':
                        code, stderr = 1, b'decoder failed'
                    return subprocess.CompletedProcess(command, code, stdout, stderr)

                with patch.object(p.g1.subprocess, 'run', side_effect=process):
                    args = (root / 'driver', root / 'ffmpeg', root / 'ldecod', root / 'input', root / 'output', 16, 16, 28, 2, False, 20)
                    if fault is None:
                        result = p.compare(*args)
                        self.assertTrue(result['conforming'])
                        self.assertEqual(result['reconstruction_sha256'], hashlib.sha256(samples).hexdigest())
                        self.assertEqual(result['modes']['P_L0_16x16'], 1)
                        self.assertEqual(len(commands), 4)
                    else:
                        with self.assertRaises((ValueError, subprocess.SubprocessError)):
                            p.compare(*args)
                if fault == 'timeout':
                    records = [json.loads(r.read_text()) for r in (root / 'output').glob('process-*.json')]
                    record = next(r for r in records if r['timed_out'])
                    self.assertEqual(record['stdout'], 'partial')
                    self.assertEqual(record['stderr'], 'interrupted')


class Forced(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = pathlib.Path(self.temp.name)
        self.directory = self.root / 'forced'
        write_forced(self.directory)

    def test_exact_inventory_hashes_and_every_independent_layout(self):
        inventory = p.forced_inventory(self.directory)
        self.assertEqual(len(inventory['entries']), 80)
        self.assertEqual(len(inventory['sha256']), 64)
        for entry in inventory['entries']:
            path = self.directory / entry['h264']['name']
            syntax = p.inspect(path.read_bytes(), 16, 16, entry['qp'], 2, bool(entry['deblocking']), 32)
            p.forced_layout(syntax, entry['layout'])
        path.write_bytes(path.read_bytes()[:-1] + b'\x80')
        self.assertNotEqual(p.forced_inventory(self.directory)['sha256'], inventory['sha256'])

    def test_missing_duplicate_extra_and_invalid_manifest_entries_fail(self):
        manifest = self.directory / 'manifest.tsv'
        original = manifest.read_text()
        for bad in (original + '0\t10\t0\n', original.replace('0\t10\t0\n', ''),
                    original.replace('0\t10\t0', '8\t10\t0'),
                    original.replace('layout\tqp\tdeblocking', 'wrong header')):
            manifest.write_text(bad)
            with self.assertRaises(ValueError):
                p.forced_inventory(self.directory)
        manifest.write_text(original)
        missing = self.directory / 'forced-10-0-0.h264'
        content = missing.read_bytes()
        missing.unlink()
        with self.assertRaisesRegex(ValueError, 'missing'):
            p.forced_inventory(self.directory)
        missing.write_bytes(content)
        extra = self.directory / 'extra.yuv'
        extra.write_bytes(b'extra')
        with self.assertRaisesRegex(ValueError, 'extra'):
            p.forced_inventory(self.directory)
        extra.unlink()
        recon = missing.with_suffix('.yuv')
        recon.write_bytes(recon.read_bytes() + b'extra')
        with self.assertRaisesRegex(ValueError, 'reconstruction size'):
            p.forced_inventory(self.directory)

    def test_changed_bytes_wrong_layout_and_reordered_equal_count_subtypes_fail_before_decoder(self):
        inventory = p.forced_inventory(self.directory)
        entry = inventory['entries'][0]
        path = self.directory / entry['h264']['name']
        original = path.read_bytes()
        path.write_bytes(original[:-1] + bytes([original[-1] ^ 1]))
        with self.assertRaisesRegex(ValueError, 'unchanged bytes'):
            p.fixture_unchanged(self.directory, entry)
        path.write_bytes(original)
        for layout, bits in ((0, inter(1)), (7, inter(3, (3, 2, 1, 0)))):
            path = self.directory / f'forced-10-0-{layout}.h264'
            path.write_bytes(stream([[intra()], [bits]], qp=10, interval=32))
            entry = next(e for e in p.forced_inventory(self.directory)['entries']
                         if (e['layout'], e['qp'], e['deblocking']) == (layout, 10, 0))
            with patch.object(p.g1, 'capture') as capture, self.assertRaisesRegex(ValueError, 'emitted partition layout'):
                p.compare_forced(self.root / 'decode', self.root / 'ffmpeg', self.root / 'ldecod',
                                 self.directory, entry, self.root / f'wrong-{layout}')
            capture.assert_not_called()

    def test_streamed_scalar_decoder_output_is_strict_and_retained(self):
        entry = p.forced_inventory(self.directory)['entries'][0]
        samples = bytes([128]) * 768
        faults = (None, 'y', 'cb', 'cr', 'extra', 'truncated', 'warning', 'stdout-warning', 'exit', 'timeout', 'changed')
        for fault in faults:
            with self.subTest(fault=fault):
                output = self.root / str(fault)
                stream_path = self.directory / entry['h264']['name']
                original = stream_path.read_bytes()
                commands = []

                def process(command, **kwargs):
                    commands.append(command)
                    self.assertEqual(kwargs['timeout'], 180)
                    self.assertEqual(command, [str(self.root / 'decode'), '--kernels', 'scalar', '--threads', '1', str(stream_path)])
                    value = bytearray(samples)
                    for part, at in (('y', 0), ('cb', 256), ('cr', 320)):
                        if fault == part:
                            value[at] ^= 1
                    if fault == 'extra':
                        value += samples
                    if fault == 'truncated':
                        value.pop()
                    if fault == 'stdout-warning':
                        value += b'Warning: synthetic damage'
                    kwargs['stdout'].write(value)
                    if fault == 'changed':
                        stream_path.write_bytes(original[:-1] + bytes([original[-1] ^ 1]))
                    if fault == 'timeout':
                        raise subprocess.TimeoutExpired(command, 180, stderr=b'partial diagnostic')
                    return subprocess.CompletedProcess(command, 1 if fault == 'exit' else 0, None,
                                                       b'Warning: damaged' if fault in ('warning', 'exit') else b'')

                with patch.object(p.g1.subprocess, 'run', side_effect=process), \
                        patch.object(p, 'decode_oracles') as oracles:
                    args = (self.root / 'decode', self.root / 'ffmpeg', self.root / 'ldecod', self.directory, entry, output)
                    if fault is None:
                        result = p.compare_forced(*args)
                        self.assertTrue(result['conforming'])
                        self.assertEqual(result['modes']['P_L0_16x16'], 1)
                        oracles.assert_called_once_with(self.root / 'ffmpeg', self.root / 'ldecod',
                                                       stream_path, stream_path.with_suffix('.yuv'), output, 16, 16, 2)
                    else:
                        with self.assertRaises((ValueError, subprocess.SubprocessError)):
                            p.compare_forced(*args)
                self.assertEqual(len(commands), 1)
                record = json.loads(next(output.glob('process-*.json')).read_text())
                self.assertEqual(record['stdout_file'], 'coreav.yuv')
                self.assertEqual(record['timed_out'], fault == 'timeout')
                self.assertEqual(record['exit_code'], None if fault == 'timeout' else 1 if fault == 'exit' else 0)
                self.assertTrue((output / 'coreav.yuv').exists())
                stream_path.write_bytes(original)


    def test_forced_fixture_uses_all_three_native_decoders_and_rejects_changed_reconstruction(self):
        for damaged in (False, True):
            with self.subTest(damaged=damaged):
                recon = self.directory / 'forced-10-0-0.yuv'
                samples = bytes([128]) * 768
                recon.write_bytes(bytes([129]) + samples[1:] if damaged else samples)
                entry = p.forced_inventory(self.directory)['entries'][0]
                commands = []

                def process(command, **kwargs):
                    commands.append(command)
                    self.assertEqual(kwargs['timeout'], 180)
                    executable = pathlib.Path(command[0]).name
                    stdout = b''
                    if executable == 'decode':
                        kwargs['stdout'].write(samples)
                        stdout = None
                    elif executable == 'ffprobe':
                        stdout = json.dumps({'streams': [dict(width=16, height=16, pix_fmt='yuv420p', color_range='tv',
                            color_space='bt709', color_transfer='bt709', color_primaries='bt709', chroma_location='left',
                            profile='Constrained Baseline', level=51, nb_read_frames='2')]}).encode()
                    elif executable == 'ffmpeg':
                        self.assertEqual(command[command.index('-pix_fmt') + 1], '+yuv420p')
                        pathlib.Path(command[-1]).write_bytes(samples)
                    elif executable == 'ldecod':
                        pathlib.Path(command[command.index('-o') + 1]).write_bytes(samples)
                    else:
                        self.fail(executable)
                    return subprocess.CompletedProcess(command, 0, stdout, b'')

                output = self.root / f'native-{damaged}'
                with patch.object(p.g1.subprocess, 'run', side_effect=process):
                    args = (self.root / 'decode', self.root / 'ffmpeg', self.root / 'ldecod', self.directory, entry, output)
                    if damaged:
                        with self.assertRaisesRegex(ValueError, 'reconstruction SHA-256'):
                            p.compare_forced(*args)
                    else:
                        self.assertTrue(p.compare_forced(*args)['conforming'])
                        self.assertEqual([pathlib.Path(command[0]).name for command in commands], ['decode', 'ffprobe', 'ffmpeg', 'ldecod'])
                        self.assertEqual(len(list(output.glob('process-*.json'))), 4)


class Report(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = pathlib.Path(self.temp.name)
        self.out = self.root / 'output'
        self.argv = ['--driver', str(self.root / 'driver'), '--ffmpeg', str(self.root / 'ffmpeg'),
                     '--jm', str(self.root / 'ldecod'), '--decoder', str(self.root / 'decode'),
                     '--forced-fixtures', str(self.root / 'forced'), '--output', str(self.out)]
        self.inventory = {'sha256': 'synthetic inventory', 'entries': [dict(layout=layout, qp=qp, deblocking=enabled)
                          for layout in range(8) for qp in p.QPS for enabled in (0, 1)]}
        synthetic = patch.object(p, 'synthetic', return_value=b'synthetic reference')
        synthetic.start()
        self.addCleanup(synthetic.stop)

    def forced(self, decoder, ffmpeg, jm, directory, entry, output):
        mode, subtypes, _ = p.FORCED_LAYOUTS[entry['layout']]
        modes = {mode: 1}
        for subtype in subtypes:
            key = ('sub_8x8', 'sub_8x4', 'sub_4x8', 'sub_4x4')[subtype]
            modes[key] = modes.get(key, 0) + 1
        return dict(entry, modes=modes, conforming=True)

    def report(self):
        return json.loads((self.out / 'report.json').read_text())

    def test_complete_report_and_explicit_unselected_mode_limitations(self):
        def compare(*args, **kwargs):
            return dict(modes={'P_Skip': 1, 'P_L0_16x16': 1}, conforming=True)

        with patch.dict(os.environ, GITHUB_ACTIONS='true'), patch.object(p.sys, 'platform', 'linux'), \
                patch.object(p, 'verify_tools', return_value={'synthetic': True}), \
                patch.object(p, 'forced_inventory', return_value=self.inventory), \
                patch.object(p, 'compare_forced', side_effect=self.forced) as forced, \
                patch.object(p, 'compare', side_effect=compare) as call:
            p.main(self.argv)
        report = self.report()
        self.assertTrue(report['complete'])
        self.assertEqual(call.call_count, 46)
        self.assertEqual(len(report['cells']), 46)
        self.assertIn('sub_4x4', report['unselected_modes'])
        self.assertNotIn('P_Skip', report['unselected_modes'])
        self.assertEqual(len(report['limitations']), 3)
        self.assertEqual(len(report['forced_cells']), 80)
        self.assertEqual(forced.call_count, 80)
        self.assertEqual(report['expected_forced_cells'], 80)
        self.assertEqual(report['uncovered_partition_modes'], [])
        self.assertEqual(report['forced_inventory'], self.inventory)

    def test_environment_pin_missing_tool_and_timeout_keep_incomplete_report(self):
        for fault in ('environment', 'platform', 'pin', 'missing', 'timeout', 'witness'):
            with self.subTest(fault=fault):
                self.out = self.root / fault
                self.argv[-1] = str(self.out)
                tools_error = ValueError('bad pin') if fault == 'pin' else FileNotFoundError() if fault == 'missing' else None
                result = dict(modes={'P_Skip': 1, 'P_L0_16x16': 1}, conforming=True)
                if fault == 'witness':
                    result['modes'] = {}
                side_effect = [result, subprocess.TimeoutExpired(['decoder'], 180)] if fault == 'timeout' else None
                with patch.dict(os.environ, GITHUB_ACTIONS='false' if fault == 'environment' else 'true'), \
                        patch.object(p.sys, 'platform', 'darwin' if fault == 'platform' else 'linux'), \
                        patch.object(p, 'verify_tools', return_value={}, side_effect=tools_error), \
                        patch.object(p, 'forced_inventory', return_value=self.inventory), \
                        patch.object(p, 'compare_forced', side_effect=self.forced), \
                        patch.object(p, 'compare', return_value=result, side_effect=side_effect) as call:
                    with self.assertRaises((ValueError, FileNotFoundError, subprocess.TimeoutExpired)):
                        p.main(self.argv)
                report = self.report()
                self.assertFalse(report['complete'])
                if fault in ('environment', 'platform', 'pin', 'missing'):
                    call.assert_not_called()
                    self.assertEqual(list(self.out.glob('*.yuv')), [])
                if fault == 'timeout':
                    self.assertEqual(len(report['cells']), 1)

    def test_real_missing_tools_and_jm_receipt_hash_binding(self):
        paths = [self.root / name for name in ('driver', 'ffmpeg', 'ffprobe', 'ldecod', 'decode')]
        with self.assertRaisesRegex(ValueError, 'executable missing'):
            p.verify_tools(paths[0], paths[1], paths[3], paths[4])
        for path in paths:
            path.write_bytes(b'synthetic executable')
            path.chmod(0o755)
        pin = dict(line.split('=', 1) for line in (p.ROOT / 'scripts/jm-decoder.env').read_text().splitlines())
        receipt = f'JM_VERSION={pin["JM_VERSION"]}\nJM_SHA256={pin["JM_SHA256"]}\n'
        receipt += "BUILD_COMMAND=make -C JM/ldecod -j2 CC='cc -fcommon'\nsynthetic compiler\n"
        receipt += f'{p.g1.digest(paths[3])}  {paths[3]}\n'
        (self.root / 'jm-build.txt').write_text(receipt)
        with patch.object(p.g1.test_oracle, 'verify_bin', return_value=[]):
            tools = p.verify_tools(paths[0], paths[1], paths[3], paths[4])
            self.assertEqual(set(tools['executables']), {'driver', 'ffmpeg', 'ffprobe', 'jm', 'decoder'})
            paths[3].write_bytes(b'changed')
            with self.assertRaisesRegex(ValueError, 'executable hash'):
                p.verify_tools(paths[0], paths[1], paths[3], paths[4])
            (self.root / 'jm-build.txt').write_text(receipt.replace('JM_VERSION=', 'WRONG='))
            with self.assertRaisesRegex(ValueError, 'receipt'):
                p.verify_tools(paths[0], paths[1], paths[3], paths[4])
        with patch.object(p.g1.test_oracle, 'verify_bin', return_value=['wrong FFmpeg pin']):
            with self.assertRaisesRegex(ValueError, 'wrong FFmpeg pin'):
                p.verify_tools(paths[0], paths[1], paths[3], paths[4])

    def test_forced_inventory_decoder_and_coverage_failures_remain_incomplete(self):
        for fault in ('inventory', 'decoder', 'coverage', 'changed-inventory'):
            with self.subTest(fault=fault):
                self.out = self.root / fault
                self.argv[-1] = str(self.out)
                inventory_side_effect = ValueError('missing inventory') if fault == 'inventory' else \
                    [self.inventory, dict(self.inventory, sha256='changed')] if fault == 'changed-inventory' else None
                count = 0

                def forced(*args):
                    nonlocal count
                    count += 1
                    if fault == 'decoder' and count == 3:
                        raise subprocess.TimeoutExpired(['decode'], 180)
                    result = self.forced(*args)
                    if fault == 'coverage':
                        result['modes'] = {}
                    return result

                with patch.dict(os.environ, GITHUB_ACTIONS='true'), patch.object(p.sys, 'platform', 'linux'), \
                        patch.object(p, 'verify_tools', return_value={}), \
                        patch.object(p, 'forced_inventory', return_value=self.inventory, side_effect=inventory_side_effect), \
                        patch.object(p, 'compare', return_value=dict(modes={'P_Skip': 1, 'P_L0_16x16': 1}, conforming=True)), \
                        patch.object(p, 'compare_forced', side_effect=forced):
                    with self.assertRaises((ValueError, subprocess.TimeoutExpired)):
                        p.main(self.argv)
                report = self.report()
                self.assertFalse(report['complete'])
                self.assertEqual(len(report['cells']), 0 if fault == 'inventory' else 46)
                self.assertEqual(len(report['forced_cells']), 2 if fault == 'decoder' else 0 if fault == 'inventory' else 80)


class Workflow(unittest.TestCase):
    def test_p_coverage_is_separate_after_unchanged_intra_and_deblock(self):
        workflow = (p.ROOT / '.github/workflows/h264-encoder.yml').read_text()
        for event, following in (('push', 'pull_request'), ('pull_request', 'merge_group')):
            block = workflow.split(f'  {event}:', 1)[1].split(f'  {following}:', 1)[0]
            for path in ('scripts/h264_p_conformance.py', 'scripts/tests/test_h264_p.py'):
                self.assertIn(f'      - {path}\n', block)
        self.assertIn("python3 -I -m unittest discover -s scripts/tests -p 'test_h264_p.py'", workflow)
        self.assertIn('cargo build --locked --release -p coreav-h264-encoder --features full --example p', workflow)
        self.assertIn('H264_P_FIXTURE_DIR="$RUNNER_TEMP/p-forced" cargo test --locked --release -p coreav-h264-encoder --features full', workflow)
        self.assertIn('cargo build --locked --release -p coreav-h264 --example decode', workflow)
        self.assertIn('--decoder target/release/examples/decode', workflow)
        self.assertIn('--forced-fixtures "$RUNNER_TEMP/p-forced"', workflow)
        self.assertLess(workflow.index('python3 -I scripts/h264_g1_conformance.py'), workflow.index('python3 -I scripts/h264_deblock_conformance.py'))
        self.assertLess(workflow.index('python3 -I scripts/h264_deblock_conformance.py'), workflow.index('python3 -I scripts/h264_p_conformance.py'))
        upload = workflow.split('name: h264-p-conformance', 1)[1]
        self.assertIn('${{ runner.temp }}/p-conformance', upload)
        self.assertIn('${{ runner.temp }}/p-forced', upload)
        self.assertIn('if: always()', workflow.split('name: h264-p-conformance', 1)[0].rsplit('- uses:', 1)[1])


if __name__ == '__main__':
    unittest.main()

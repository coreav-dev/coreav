import importlib.util
import pathlib
import struct
import tempfile
import unittest

SPEC = importlib.util.spec_from_file_location('opus_identity', pathlib.Path(__file__).parents[1] / 'opus_identity.py')
IDENTITY = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(IDENTITY)


def dump(samples, channels=1):
    return (struct.pack('<Q', len(samples)) + b''.join(
        struct.pack('<4Q', index, len(values) // channels, channels, len(values))
        + struct.pack('<' + 'I' * len(values), *values)
        for index, values in enumerate(samples)))


class Identity(unittest.TestCase):
    def check(self, base, head, expected=1, channels=1):
        with tempfile.TemporaryDirectory() as directory:
            a, b = (pathlib.Path(directory) / name for name in ('base', 'head'))
            a.write_bytes(base)
            b.write_bytes(head)
            return IDENTITY.compare(a, b, expected, channels)

    def test_low_bit_difference_fails_even_when_s16_agrees(self):
        a, b = 0x3DCCCCCD, 0x3DCCCCCE
        quantized = [round(struct.unpack('<f', struct.pack('<I', bits))[0] * 32768) for bits in (a, b)]
        self.assertEqual(quantized[0], quantized[1])
        result = self.check(dump([[a]]), dump([[b]]))
        self.assertFalse(result['passed'])
        self.assertEqual(result['first_mismatch'], {'packet': 0, 'kind': 'sample_bits'})

    def test_equal_bits_pass_with_packet_boundaries_and_channels(self):
        data = dump([[0, 0x80000000], [0x3F800000, 0xBF800000]], channels=2)
        self.assertTrue(self.check(data, data, expected=2, channels=2)['passed'])

    def test_packet_boundary_count_and_channel_differences_fail(self):
        base = dump([[0, 0]])
        for head in (dump([[0]]), dump([[0, 0]], channels=2), dump([[0], [0]])):
            with self.subTest(head=head):
                self.assertFalse(self.check(base, head)['passed'])

    def test_signed_zero_is_not_equal(self):
        self.assertFalse(self.check(dump([[0]]), dump([[0x80000000]]))['passed'])

    def test_truncated_extra_and_invalid_headers_fail_on_both_sides(self):
        valid = dump([[0]])
        bad = [valid[:size] for size in (0, 7, 8, 20, len(valid) - 1)]
        bad += [valid + b'\x00', struct.pack('<Q', 0),
                struct.pack('<5Q', 1, 0, 1, 1, 2) + bytes(8),
                struct.pack('<5Q', 1, 0, 5761, 1, 5761) + bytes(5761 * 4)]
        for data in bad:
            for a, b in ((data, valid), (valid, data)):
                with self.subTest(length=len(data)):
                    self.assertFalse(self.check(a, b)['passed'])

    def test_empty_work_and_invalid_channel_counts_are_refused(self):
        for expected, channels in ((0, 1), (1, 0), (1, 256)):
            with self.assertRaises(ValueError):
                self.check(dump([[0]]), dump([[0]]), expected, channels)


if __name__ == '__main__':
    unittest.main()

"""Tests for the reproducer fetcher's archive handling."""
import io
import pathlib
import sys
import tarfile
import unittest
import zipfile

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent.parent))
import fetch_reproducers  # noqa: E402

CAP = 1000


def zipped(payload):
    out = io.BytesIO()
    with zipfile.ZipFile(out, 'w', zipfile.ZIP_DEFLATED) as archive:
        archive.writestr('case.mp4', payload)
    return out.getvalue()


def tarred(payload):
    out = io.BytesIO()
    with tarfile.open(fileobj=out, mode='w:gz') as archive:
        info = tarfile.TarInfo('case.mp4')
        info.size = len(payload)
        archive.addfile(info, io.BytesIO(payload))
    return out.getvalue()


class MemberTest(unittest.TestCase):
    def test_members_within_the_cap_are_returned(self):
        for pack in (zipped, tarred):
            self.assertEqual(fetch_reproducers.member(pack(b'x' * CAP), 'case.mp4', CAP), b'x' * CAP)

    def test_expansion_beyond_the_cap_is_refused(self):
        # One mebibyte of zeros compresses to about a kilobyte.
        for pack in (zipped, tarred):
            blob = pack(bytes(1 << 20))
            self.assertLess(len(blob), 1 << 20)
            with self.assertRaisesRegex(ValueError, 'size cap'):
                fetch_reproducers.member(blob, 'case.mp4', CAP)

    def test_understated_zip_size_is_still_capped(self):
        blob = bytearray(zipped(bytes(1 << 20)))
        # Rewrite the uncompressed size in the central directory (offset 24
        # of the entry) to claim a member within the cap.
        central = blob.rindex(b'PK\x01\x02')
        blob[central + 24:central + 28] = (10).to_bytes(4, 'little')
        with self.assertRaises(Exception):
            fetch_reproducers.member(bytes(blob), 'case.mp4', CAP)


if __name__ == '__main__':
    unittest.main()

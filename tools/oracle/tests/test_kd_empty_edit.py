"""The byte evidence of KD-FF9-EMPTY-EDIT-PADDING (`oracle_kd.empty_edit_padding`):
the last AAC packet's padding after an empty edit, as each tool lists it, on
generated files, and every near miss left out."""
import importlib.util
import pathlib
import shutil
import struct
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[3]
SPEC = importlib.util.spec_from_file_location('oracle_kd', ROOT / 'scripts' / 'oracle_kd.py')
kd = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(kd)

# AAC LC at 48 kHz (index 3), stereo, frames of 1,024 samples, and the
# extension that rules SBR out, without which SBR is open at any rate.
LC_48K = bytes([2 << 3 | 3 >> 1, (3 & 1) << 7 | 2 << 3])
EXT = kd.NO_SBR_EXTENSION


def box(kind, payload=b''):
    return struct.pack('>I', 8 + len(payload)) + kind + payload


def full_box(kind, version, flags, payload=b''):
    return box(kind, bytes([version]) + flags.to_bytes(3, 'big') + payload)


def esds(asc):
    dsi = b'\x05' + bytes([len(asc)]) + asc
    dcd = b'\x04' + bytes([13 + len(dsi)]) + b'\x40\x15' + b'\0' * 11 + dsi
    es = b'\x03' + bytes([3 + len(dcd)]) + b'\0\x01\0' + dcd
    return full_box(b'esds', 0, 0, es)


def movie(edits=((50, -1, 1 << 16), (100, 1024, 1 << 16)), stts=((5, 1024), (1, 700)), asc=LC_48K + EXT,
          timescale=48000, mdhd=None, handler=b'soun', ctts=False, mvex=False, elst_version=0, more=(), index=1):
    """One audio track of 10-byte samples in one chunk, timed by `stts`
    (count, delta) at `timescale`, in a movie of 1,000 ticks a second;
    `more` configurations follow in further descriptions, and the chunk
    names description `index`."""
    count = sum(n for n, _ in stts)
    mdhd = sum(n * d for n, d in stts) if mdhd is None else mdhd
    mvhd = full_box(b'mvhd', 0, 0, b'\0' * 8 + struct.pack('>II', 1000, 0) + b'\0' * 80)
    tkhd = full_box(b'tkhd', 0, 0, b'\0' * 8 + struct.pack('>I', 1) + b'\0' * 68)
    mdhd = full_box(b'mdhd', 0, 0, b'\0' * 8 + struct.pack('>II', timescale, mdhd) + b'\0' * 4)
    hdlr = full_box(b'hdlr', 0, 0, b'\0' * 4 + handler + b'\0' * 13)
    entry = (struct.pack('>QqI', d, t, r) if elst_version else struct.pack('>IiI', d, t, r) for d, t, r in edits)
    elst = full_box(b'elst', elst_version, 0, struct.pack('>I', len(edits)) + b''.join(entry))
    mp4a = b''.join(box(b'mp4a', b'\0' * 6 + b'\0\x01' + b'\0' * 8
                        + struct.pack('>HHHHI', 2, 16, 0, 0, timescale << 16) + esds(c)) for c in (asc,) + tuple(more))
    tables = (full_box(b'stsd', 0, 0, struct.pack('>I', 1 + len(more)) + mp4a)
              + full_box(b'stts', 0, 0, struct.pack('>I', len(stts)) + b''.join(struct.pack('>II', *e) for e in stts))
              + full_box(b'stsz', 0, 0, struct.pack('>II', 10, count))
              + full_box(b'stsc', 0, 0, struct.pack('>IIII', 1, 1, count, index)))
    if ctts:
        tables += full_box(b'ctts', 0, 0, struct.pack('>III', 1, count, 0))
    extra = box(b'mvex', full_box(b'trex', 0, 0, struct.pack('>IIIII', 1, 1, 0, 0, 0))) if mvex else b''

    def build(chunk):
        stbl = box(b'stbl', tables + full_box(b'stco', 0, 0, struct.pack('>II', 1, chunk)))
        trak = box(b'trak', tkhd + box(b'edts', elst) + box(b'mdia', mdhd + hdlr + box(b'minf', stbl)))
        return box(b'ftyp', b'isom\0\0\0\0') + box(b'moov', mvhd + trak + extra)
    head = build(0)
    return build(len(head) + 8) + box(b'mdat', bytes(10 * count)), len(head) + 8 + 10 * (count - 1)


class EmptyEditPadding(unittest.TestCase):
    def evidence(self, data):
        directory = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, directory)
        (pathlib.Path(directory) / 'f.mp4').write_bytes(data)
        return kd.empty_edit_padding(kd.read_facts(directory, 'f.mp4'))

    def test_the_packet_starts_after_the_end(self):
        # 50 ms empty (2,400 ticks), media from 1,024 to the media's end at
        # 5,820: the last sample, 700 ticks at 5,120, is listed at 6,496.
        data, last = movie()
        self.assertEqual(self.evidence(data), [kd.EmptyEditPadding(1, last, 10, None, 324)])

    def test_the_packet_straddles_the_end(self):
        # 30 ms empty (1,440 ticks): listed at 5,536, it ends past 5,820, so
        # ffprobe counts the frame from there, 740 samples.
        data, last = movie(edits=((30, -1, 1 << 16), (100, 1024, 1 << 16)))
        self.assertEqual(self.evidence(data), [kd.EmptyEditPadding(1, last, 10, 740, 324)])
        # 44.1 kHz from media time 0, an empty edit of 19 ms (837.9 ticks,
        # 838), a media edit of 440,294.4 ticks to the end at 440,294, a last
        # sample of 998 ticks at 439,296: 864 against 26.
        rate = bytes([2 << 3 | 4 >> 1, (4 & 1) << 7 | 1 << 3]) + EXT
        data, last = movie(edits=((19, -1, 1 << 16), (9984, 0, 1 << 16)), stts=((429, 1024), (1, 998)),
                           asc=rate, timescale=44100)
        self.assertEqual(self.evidence(data), [kd.EmptyEditPadding(1, last, 10, 864, 26)])

    def test_frames_of_960(self):
        flag = bytes([LC_48K[0], LC_48K[1] | 4]) + EXT
        data, last = movie(stts=((5, 960), (1, 700)), edits=((50, -1, 1 << 16), (100, 960, 1 << 16)), asc=flag)
        self.assertEqual(self.evidence(data), [kd.EmptyEditPadding(1, last, 10, None, 260)])

    def test_tools_that_agree_give_no_evidence(self):
        # 20 ms empty (960 ticks): the packet, listed at 5,056, ends before
        # 5,820, and both tools list 324.
        self.assertEqual(self.evidence(movie(edits=((20, -1, 1 << 16), (100, 1024, 1 << 16)))[0]), [])
        # A last sample of a whole frame leaves nothing past the end.
        self.assertEqual(self.evidence(movie(stts=((6, 1024),))[0]), [])

    def test_other_shapes_give_no_evidence(self):
        sbr = bytes([5 << 3 | 6 >> 1, (6 & 1) << 7 | 2 << 3, 0x18, 0x80])
        for name, data in (
                ('no empty edit', movie(edits=((100, 1024, 1 << 16),))),
                ('empty edit of 0', movie(edits=((0, -1, 1 << 16), (100, 1024, 1 << 16)))),
                ('media edit first', movie(edits=((100, 1024, 1 << 16), (50, -1, 1 << 16)))),
                ('two media edits', movie(edits=((50, -1, 1 << 16), (50, 1024, 1 << 16), (50, 3424, 1 << 16)))),
                ('a rate', movie(edits=((50, -1, 1 << 16), (100, 1024, 2 << 16)))),
                ('an edit ending early', movie(edits=((50, -1, 1 << 16), (90, 1024, 1 << 16)))),
                ('the media header longer', movie(mdhd=5900)),
                ('the media header shorter', movie(mdhd=5800)),
                ('a video track', movie(handler=b'vide')),
                ('composition offsets', movie(ctts=True)),
                ('fragments', movie(mvex=True)),
                ('SBR', movie(asc=sbr)),
                ('another rate', movie(asc=bytes([2 << 3 | 4 >> 1, (4 & 1) << 7 | 2 << 3]) + EXT)),
                ('dependsOnCoreCoder', movie(asc=bytes([LC_48K[0], LC_48K[1] | 2]) + EXT)),
                ('extensionFlag', movie(asc=bytes([LC_48K[0], LC_48K[1] | 1]) + EXT)),
                ('SBR signalled present', movie(asc=LC_48K + b'\x56\xe5\x80')),
                ('trailing bytes', movie(asc=LC_48K + EXT + b'\0')),
                ('main profile', movie(asc=bytes([1 << 3 | LC_48K[0] & 7, LC_48K[1]]) + EXT)),
                ('no configuration', movie(asc=b'')),
        ):
            self.assertEqual(self.evidence(data[0]), [], name)

    def test_an_empty_edit_halfway_between_two_ticks_gives_no_evidence(self):
        # At 7,350 Hz (index 12) 20 ms is 147 ticks, 10 ms 73.5.
        rate = bytes([2 << 3 | 12 >> 1, (12 & 1) << 7 | 1 << 3]) + EXT
        data, last = movie(edits=((20, -1, 1 << 16), (1000, 0, 1 << 16)), asc=rate, timescale=7350)
        self.assertEqual(self.evidence(data), [kd.EmptyEditPadding(1, last, 10, 5267 + 1024 - 5820, 324)])
        data, _ = movie(edits=((10, -1, 1 << 16), (1000, 0, 1 << 16)), asc=rate, timescale=7350)
        self.assertEqual(self.evidence(data), [])

    def test_the_last_samples_description_gives_the_frame(self):
        # A second description of 960-sample frames, named by the chunk: the
        # first one's 1,024 would give 324 where 260 is due.
        flag = bytes([LC_48K[0], LC_48K[1] | 4]) + EXT
        self.assertEqual(self.evidence(movie(more=(flag,), index=2)[0]), [])
        self.assertEqual(self.evidence(movie(more=(flag,))[0]), [])
        self.assertEqual(self.evidence(movie(index=2)[0]), [])

    def test_every_rate_needs_the_extension_that_rules_sbr_out(self):
        # Implicit SBR is allowed at any core rate (ISO/IEC 14496-3 1.6.5.3),
        # and would double the frame: a configuration without the extension,
        # at 48, 44.1 or 24 kHz, gives no evidence; with it, it does.
        for config, timescale in ((b'\x11\x90', 48000), (b'\x12\x10', 44100), (b'\x13\x10', 24000)):
            edits = ((100, -1, 1 << 16), (300, 1024, 1 << 16))
            self.assertEqual(self.evidence(movie(asc=config, timescale=timescale, edits=edits)[0]), [], config)
            data, last = movie(asc=config + EXT, timescale=timescale, edits=edits)
            self.assertEqual(self.evidence(data), [kd.EmptyEditPadding(1, last, 10, None, 324)], config)

    def test_the_last_sample_must_start_inside_the_media_edit(self):
        # A media edit from 10,000, past the samples' end at 5,820, presents
        # none of them, and one from 5,500 starts inside the last sample.
        self.assertEqual(self.evidence(movie(edits=((1000, -1, 1 << 16), (100, 10000, 1 << 16)))[0]), [])
        self.assertEqual(self.evidence(movie(edits=((1000, -1, 1 << 16), (100, 5500, 1 << 16)))[0]), [])
        data, last = movie(edits=((1000, -1, 1 << 16), (100, 5120, 1 << 16)))
        self.assertEqual(self.evidence(data), [kd.EmptyEditPadding(1, last, 10, None, 324)])

    def test_a_version_1_edit_list_reads_the_same(self):
        data, last = movie(elst_version=1)
        self.assertEqual(self.evidence(data), [kd.EmptyEditPadding(1, last, 10, None, 324)])

    def test_unreadable_files_give_no_evidence(self):
        self.assertEqual(kd.empty_edit_padding(None), [])
        data, _ = movie()
        self.assertEqual(self.evidence(data[:len(data) // 2]), [])


if __name__ == '__main__':
    unittest.main()

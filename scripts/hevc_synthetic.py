#!/usr/bin/env python3
"""Classify committed synthetic references as header-only intra candidates."""

import argparse
import pathlib
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import hevc_fixture as fixture
import test_oracle


def inspect(data_dir, out_dir, parser, qualified, harness):
    registered = harness.read_json(fixture.REGISTRATION)
    harness.require(registered == harness.read_json(data_dir / 'reference.json'),
                    'synthetic preparation differs from committed registration')
    harness.require(registered['recipes'] == fixture.recipes()
                    and registered['decode_command'] == fixture.decode_command(),
                    'synthetic registered commands differ')
    harness.require(fixture.binary_identity(qualified.ffmpeg, qualified.ffprobe)
                    == registered['oracle'], 'synthetic oracle identity differs')
    harness.require([r['id'] for r in registered['fixtures']]
                    == [r['id'] for r in fixture.recipes()], 'synthetic fixture membership differs')
    registration_sha = harness.digest(fixture.REGISTRATION)
    records = []
    for expected in registered['fixtures']:
        identity = expected['id']
        path = data_dir / 'run-1' / (identity + '.hevc')
        bitstream = fixture.bounded_bytes(path)
        raw = fixture.bounded_bytes(data_dir / 'run-1' / (identity + '.yuv'))
        harness.require(len(bitstream) == expected['bitstream_bytes']
                        and fixture.sha256(bitstream) == expected['bitstream_sha256'],
                        'synthetic compressed bytes differ')
        harness.require(fixture.raw_metadata(raw) == expected['raw'],
                        'synthetic reference bytes differ')
        harness.require(fixture.verify_hash_sei(bitstream, raw)
                        == expected['decoded_picture_hash_sei_md5'], 'synthetic SEI differs')
        directory = out_dir / identity
        code, native, _ = harness.child([parser, path], directory, 'native')
        harness.require(code == 0, 'synthetic native parser failed')
        summary, pictures, outputs = harness.native_rows(native, bitstream)
        harness.require(summary['pictures'] == summary['output_pictures'] == 2
                        and len(pictures) == len(outputs) == 2,
                        'synthetic native picture count differs')
        harness.require(all(p['profile_idc'] == 2 and p['bit_depth'] == 10
                        and p['coded_width'] == p['coded_height'] == p['width'] == p['height'] == 64
                        and p['all_intra'] and p['filters_disabled']
                        and p['nal_type'] in (19, 20) for p in pictures),
                        'synthetic native Main10 unfiltered intra proof differs')
        packets = harness.probe(qualified.ffprobe, path, directory)
        frames = harness.probe(qualified.ffprobe, path, directory, frames=True)['frames']
        checks = harness.compare_headers(summary, pictures, outputs, packets, frames)
        harness.require(checks['output_order_checked_by_packet_positions'],
                        'synthetic independent output order is unproved')
        references = [dict(output_index=index, bytes=fixture.FRAME_BYTES,
                           md5=picture['visible_yuv_md5'],
                           coded_component_md5=picture['coded_component_md5'],
                           source='bench/hevc-conformance/synthetic.json#' + identity,
                           source_sha256=registration_sha,
                           source_scope='synthetic_visible_yuv_picture')
                      for index, picture in enumerate(expected['raw']['pictures'])]
        candidates = harness.coverage(pictures, outputs, references)
        harness.require(len(candidates) == 2 and all(c['gate'] == 'G2' and c['bit_depth'] == 10
                        for c in candidates), 'synthetic Main10 G2 candidate proof differs')
        records.append(dict(test='SYNTHETIC_' + identity, source_kind='synthetic',
                            sha256=expected['bitstream_sha256'], status='headers_checked',
                            reference_count=dict(value=2, method='synthetic_registered_recipe',
                                published=False, source='bench/hevc-conformance/synthetic.json#' + identity,
                                source_sha256=registration_sha),
                            reference_frames=references, intra_candidates=candidates,
                            tools=summary['tools'], **checks))
    result = dict(complete=True, source_kind='synthetic',
                  registration_sha256=registration_sha, parser_sha256=harness.digest(parser),
                  oracle={name: harness.digest(getattr(qualified, name)) for name in ('ffmpeg', 'ffprobe')},
                  streams=records, reconstruction_gate_passed=False)
    harness.write_json(out_dir / 'census.json', result)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--data', required=True, type=pathlib.Path)
    parser.add_argument('--out', required=True, type=pathlib.Path)
    parser.add_argument('--parser', required=True, type=pathlib.Path)
    parser.add_argument('--oracle-bin', required=True, type=pathlib.Path)
    args = parser.parse_args()
    import hevc_conformance as harness
    qualified = test_oracle.require(args.oracle_bin / 'ffmpeg', args.oracle_bin / 'ffprobe')
    inspect(args.data, args.out, args.parser, qualified, harness)
    print('four synthetic Main10 header candidates checked; no reconstruction verdict')
    return 0


if __name__ == '__main__':
    sys.exit(main())

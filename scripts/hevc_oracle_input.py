import hashlib
import re

STREAM = 'VPSSPSPPS_A'
STREAM_SHA256 = '8501a5e5f0f191d3810a7aecab43aacac1fcc28f08cb3731158cc48a89501a27'
PREPARED_SHA256 = 'd2097afe2c90bda74915fae48d567ac260b7932781644ba9cdb3ebbc69d315dd'
DESCRIPTION_SHA256 = '35df5c194219524b2bab8bcff8260eb0405f1b61df1af260db06284d1703e061'
DOCUMENT = 'bench/hevc-conformance/oracle-preparation.md'


def reorder(data, harness):
    nals = harness.annex_b(data)
    markers = list(re.finditer(b'\x00\x00\x00\x01|\x00\x00\x01', data))
    harness.require(len(markers) == len(nals)
                    and all(m.end() == n['offset'] for m, n in zip(markers, nals)),
                    'oracle preparation framing disagrees')
    first = next((i for i, n in enumerate(nals) if n['nal_type'] < 32), len(nals))
    harness.require(0 < first < len(nals)
                    and all(n['nal_type'] in (32, 33, 34) for n in nals[:first])
                    and not any(n['nal_type'] in (32, 33, 34) for n in nals[first:]),
                    'oracle preparation requires only leading parameter sets')
    starts = [m.start() for m in markers] + [len(data)]
    units = [data[a:b] for a, b in zip(starts, starts[1:])]
    order = sorted(range(first), key=lambda i: nals[i]['nal_type']) + list(range(first, len(nals)))
    prepared = data[:starts[0]] + b''.join(units[i] for i in order)
    after = harness.annex_b(prepared)
    harness.require(len(prepared) == len(data) and prepared[starts[first]:] == data[starts[first]:]
                    and len(after) == len(nals) and after[first]['offset'] == nals[first]['offset']
                    and all(prepared[new['offset']:new['offset'] + new['size']]
                            == data[nals[i]['offset']:nals[i]['offset'] + nals[i]['size']]
                            for new, i in zip(after, order)), 'oracle preparation changed a NAL byte')
    harness.require(all([units[i] for i in order[:first] if nals[i]['nal_type'] == kind]
                        == [units[i] for i in range(first) if nals[i]['nal_type'] == kind]
                        for kind in (32, 33, 34)), 'oracle preparation changed within-type order')
    return prepared, dict(method='stable_leading_parameter_sets', bytes=len(data),
        original_sha256=hashlib.sha256(data).hexdigest(),
        prepared_sha256=hashlib.sha256(prepared).hexdigest(),
        prefix_nals=first, prefix_counts={str(k): sum(n['nal_type'] == k for n in nals[:first])
                                        for k in (32, 33, 34)},
        first_vcl_offset=nals[first]['offset'], suffix_unchanged=True, every_nal_byte_preserved=True,
        within_type_order_preserved=True, original_nal_indices_in_prepared_order=order,
        original_unit_sha256=[hashlib.sha256(unit).hexdigest() for unit in units])


def prepare(case, source, directory, harness):
    count = case.get('published_output_count', {})
    harness.require(case['test'] == STREAM and count.get('value') == 6
                    and count.get('source') == 'VPSSPSPPS_A_MainConcept_1.txt'
                    and count.get('source_sha256') == DESCRIPTION_SHA256,
                    'oracle preparation is registered only for VPSSPSPPS_A and its published count')
    harness.require(source.stat().st_size == 106928 and harness.digest(source) == STREAM_SHA256,
                    'oracle preparation original stream identity differs')
    prepared, receipt = reorder(source.read_bytes(), harness)
    harness.require(receipt['prefix_counts'] == {'32': 106, '33': 106, '34': 106}
                    and receipt['first_vcl_offset'] == 8586
                    and receipt['prepared_sha256'] == PREPARED_SHA256,
                    'oracle preparation differs from registered leading prefix')
    destination = source.parent / 'oracle-prepared.hevc'
    destination.write_bytes(prepared)
    receipt.update(stream=STREAM, document=DOCUMENT, document_sha256=harness.digest(harness.ROOT / DOCUMENT),
                   decision='Captain, 2026-11-04', published_output_count=count)
    harness.write_json(directory / 'oracle-preparation.json', receipt)
    return destination, receipt

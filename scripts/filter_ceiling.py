#!/usr/bin/env python3
"""Disposable instruction-removal experiment; not a decoder candidate."""

import argparse
import difflib
import json
import os
import pathlib
import platform
import re
import statistics
import sys
import tempfile

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import hevc_reference as ref


def body_span(text, signature):
    if text.count(signature) != 1:
        raise ValueError('ambiguous_function')
    start = text.index('{', text.index(signature))
    level = 1
    end = start + 1
    while level:
        level += (text[end] == '{') - (text[end] == '}')
        end += 1
    return start + 1, end - 1


def erased(text, kind):
    if kind == 'deblock10':
        for name in ('deblock_luma_scalar', 'deblock_chroma_scalar'):
            start, _ = body_span(text, f'pub fn {name}(')
            text = text[:start] + '\n    if depth.bits() == 10 {\n        return Ok(core::hint::black_box(edge));\n    }\n' + text[start:]
        return text
    if kind == 'sao8-validation':
        first = 'if samples.iter().any(|v| *v > self.depth.max_sample()) {'
        second = 'if source\n            .iter()'
        if text.count(first) != 1 or text.count(second) != 1:
            raise ValueError('validation_span_changed')
        return text.replace(first, 'if self.depth.bits() != 8 && samples.iter().any(|v| *v > self.depth.max_sample()) {').replace(second, 'if self.depth.bits() != 8 && source\n            .iter()')
    if kind == 'sao8-tail':
        needle = '        let mut changed = changed;'
        if text.count(needle) != 2:
            raise ValueError('tail_span_changed')
        return text.replace(needle, '        if self.depth.bits() == 8 {\n            return Ok(changed);\n        }\n' + needle)
    if kind == 'deblock':
        for name, ignored in (
            ('deblock_luma_with_strength', 'depth, qp, offsets, strength, protected'),
            ('deblock_chroma', 'depth, qp, tc_offset, protected'),
        ):
            start, end = body_span(text, f'pub fn {name}(')
            text = text[:start] + f'\n    let _ = ({ignored});\n    Ok(core::hint::black_box(edge))\n' + text[end:]
        return text
    if kind != 'sao10':
        raise ValueError('unknown_erase_arm')
    for owner, signature, replacement in (
        ('SaoBand', 'pub fn apply(self, samples:',
         '\n        if self.depth.bits() == 10 {\n            core::hint::black_box(&mut *samples);\n            return Ok(0);\n        }'),
        ('SaoEdge', 'pub fn apply(',
         '\n        if self.depth.bits() == 10 {\n            core::hint::black_box(neighbours);\n            output.copy_from_slice(source);\n            return Ok(0);\n        }'),
    ):
        offset = text.index(f'impl {owner} {{')
        tail = text[offset:]
        start, _ = body_span(tail, signature)
        position = offset + start
        text = text[:position] + replacement + text[position:]
    return text


def erased_driver(text, kind):
    if kind == 'deblock-write':
        start, end = body_span(text, 'fn write_edge<const N: usize>(')
        return text[:start] + '\n        let _ = (component, horizontal, edge, along, strength);\n        core::hint::black_box(result);\n        Ok(())\n    ' + text[end:]
    if kind == 'deblock8-gather':
        for width in (8, 4):
            needle = f'                        let mut samples = [[0; {width}]; 4];'
            if text.count(needle) != 1:
                raise ValueError('gather_span_changed')
            start = text.index(needle)
            end = text.index('                        let offset = *ctu' if width == 4 else '                        let result = kernels::', start)
            original = text[start:end]
            prefix = f'                        let mut samples = core::hint::black_box([[0; {width}]; 4]);\n                        if self.depth.bits() != 8 {{\n'
            gather = original[len(needle) + 1:]
            text = text[:start] + prefix + gather + '                        }\n' + text[end:]
        return text
    if kind == 'sao8-protection':
        needle = '''                                let protected = self.cell(x * scale, y * scale)?.protected;
                                x = ((x / (4 / scale) + 1) * (4 / scale)).min(x1);
                                while x < x1
                                    && self.cell(x * scale, y * scale)?.protected == protected
                                {
                                    x = (x + 4 / scale).min(x1);
                                }'''
        if text.count(needle) != 1:
            raise ValueError('protection_span_changed')
        replacement = '''                                let protected;
                                if self.depth.bits() == 8 {
                                    protected = core::hint::black_box(false);
                                    x = x1;
                                } else {
''' + needle.replace('let protected =', 'protected =') + '\n                                }'
        return text.replace(needle, replacement)
    raise ValueError('unknown_driver_arm')


def functions(annotation):
    rows = []
    for line in annotation.splitlines():
        match = re.match(r'^\s*([\d,]+)\s+\(\s*([\d.]+)%\)\s+(.+)$', line)
        if match and match[3] != 'PROGRAM TOTALS':
            rows.append(dict(instructions=int(match[1].replace(',', '')),
                             percent=float(match[2]), function=match[3]))
    if not rows:
        raise ValueError('missing_function_attribution')
    return rows


def count(binary, source, out, identity, arm, expected):
    path = out / f'{identity}-{arm}.callgrind'
    stdout, _ = ref.command(['valgrind', '--tool=callgrind', '--error-exitcode=99',
        '--collect-jumps=no', '--dump-instr=no', '--cache-sim=no',
        f'--callgrind-out-file={path}', binary, source, 'decode'], timeout=3600)
    work = json.loads(stdout)
    if work != expected:
        raise ValueError('changed_decode_work')
    text = path.read_text()
    events = re.search(r'^events: (.+)$', text, re.M)[1].split()
    totals = re.search(r'^summary: (.+)$', text, re.M)[1].split()
    total = int(totals[events.index('Ir')])
    annotation, _ = ref.command(['callgrind_annotate', '--auto=no', '--inclusive=no',
        '--show=Ir', '--threshold=100', path])
    (out / f'{identity}-{arm}-functions.txt').write_bytes(annotation)
    rows = functions(annotation.decode())
    if sum(row['instructions'] for row in rows) != total:
        raise ValueError('incomplete_function_attribution')
    return dict(instructions=total, work=work, functions=rows)


def pixels(binary, source):
    stdout, _ = ref.command([binary, source, 'check'])
    rows = [json.loads(line) for line in stdout.splitlines()]
    if rows[-1].get('complete') is not True:
        raise ValueError('incomplete_pixel_work')
    return dict(pictures=[(row['output_bytes'], row['output_md5']) for row in rows if row.get('type') == 'picture'],
                summary=rows[-1])


def run(args):
    if re.fullmatch('[0-9a-f]{40}', args.baseline) is None:
        raise ValueError('full_baseline_sha_required')
    args.only = set(args.only.split(','))
    if not args.only or args.only - {f'd{i:02}' for i in range(26)} - {f'c{i:02}' for i in range(4)}:
        raise ValueError('invalid_input_ids')
    oracle = ref.test_oracle.require(args.oracle_bin / 'ffmpeg', args.oracle_bin / 'ffprobe')
    args.out.mkdir(parents=True, exist_ok=True)
    binaries = {arm: args.binaries / arm for arm in ('main', *args.arms)}
    report = dict(complete=False, baseline=args.baseline, frames_cap=args.frames, audit_only=args.audit_only,
        method='Physical kernel erasure on matched picture prefixes, suffix SEI removed equally in all arms; instruction reference, not timing or a correct decoder',
        caveats='Erasure also removes validation and changes reconstructed reference samples, so later prediction and SAO classification work can change. SAO erasure preserves its source copy. Whole-crate compiler decisions can change unrelated kernels. Neither is a realizable SIMD gain or a corpus-wide CPU share.',
        selected_ids=sorted(args.only), depth=args.depth,
        binaries={arm: ref.conformance.digest(path) for arm, path in binaries.items()},
        oracle=ref.oracle_pin.load()['BUILD'],
        host=dict(cpu=next(line.split(':', 1)[1].strip() for line in pathlib.Path('/proc/cpuinfo').read_text().splitlines()
                           if line.startswith('model name')), kernel=platform.release(),
                  affinity=sorted(os.sched_getaffinity(0)), cores=os.cpu_count(), load=os.getloadavg()),
        files=[])
    if args.audit_only:
        report['method'] = 'Original decoder pixel verification and work counters on longer prefixes; no removal counts are repeated'
        report['caveats'] = 'Only observes the selected prefixes; not a performance comparison'
    pool = ref.round_files.pool()
    pool.limits.max_requests = 4000
    pool.limits.max_bytes = 32 << 30

    def save():
        report['r2_requests'] = pool.limits.requests
        (args.out / 'results.json').write_text(json.dumps(report, indent=2) + '\n')

    try:
        with tempfile.TemporaryDirectory(prefix='filter-ceiling-') as temp:
            directory = pathlib.Path(temp)
            for identity, original, original_sha in ref.inputs(args, oracle, directory, pool):
                row = dict(id=identity, source_sha256=original_sha, complete=False)
                report['files'].append(row)
                save()
                expected = ref.verify(binaries['main'], oracle, original)
                if not 0 < expected['frames'] <= args.frames or args.depth and expected['depth'] != args.depth:
                    raise ValueError('unexpected_prefix_or_depth')
                if args.audit_only:
                    check = pixels(binaries['main'], original)
                    row.update(expected, check=check, complete=True)
                    save()
                    print(json.dumps(dict(id=identity, depth=args.depth, complete=True)), flush=True)
                    continue
                source = directory / f'{identity}-without-suffix.hevc'
                ref.command([oracle.ffmpeg, '-hide_banner', '-loglevel', 'error', '-nostdin',
                    '-i', original, '-map', '0:v:0', '-frames:v', str(args.frames), '-c:v', 'copy',
                    '-bsf:v', 'filter_units=remove_types=40', '-f', 'hevc', '-y', source])
                if ref.verify(binaries['main'], oracle, source) != expected:
                    raise ValueError('suffix_removal_changed_pixels')
                row.update(expected, prepared_sha256=ref.conformance.digest(source),
                           original_prefix_sha256=ref.conformance.digest(original), calls={})
                stdout, _ = ref.command([binaries['main'], source, 'decode'])
                work = json.loads(stdout)
                if work['pictures'] != expected['frames'] or work['decoded_pictures'] < expected['frames'] or work['bins'] <= 0:
                    raise ValueError('incomplete_prefix_work')
                row['checks'] = {arm: pixels(binary, source) for arm, binary in binaries.items()}
                main_pixels = row['checks']['main']['pictures']
                row['pixel_equal_to_main'] = {arm: check['pictures'] == main_pixels
                                             for arm, check in row['checks'].items()}
                if expected['depth'] == 8 and 'sao10' in binaries and not row['pixel_equal_to_main']['sao10']:
                    raise ValueError('sao10_erase_changed_eight_bit_pixels')
                for arm, binary in binaries.items():
                    applicable = arm == 'main' or not (
                        arm == 'deblock10' and expected['depth'] != 10
                        or arm in ('deblock8-gather', 'sao8-validation', 'sao8-tail', 'sao8-protection') and expected['depth'] != 8
                        or arm.startswith('sao8-') and not any(row['checks']['main']['summary']['stats']['sao_blocks'])
                        or arm in ('deblock10', 'deblock-write', 'deblock8-gather') and not any(row['checks']['main']['summary']['stats']['deblock_edges'])
                    )
                    if not applicable:
                        if not row['pixel_equal_to_main'][arm]:
                            raise ValueError('inactive_arm_changed_pixels')
                        row['calls'][arm] = dict(row['calls']['main'], inactive=True)
                        save()
                        continue
                    row['calls'][arm] = count(binary, source, args.out, identity, arm, work)
                    save()
                if len(report['files']) == 1:
                    control = count(binaries['main'], source, args.out, identity, 'main-aa', work)
                    row['aa_instructions'] = control['instructions']
                    if control['instructions'] != row['calls']['main']['instructions']:
                        raise ValueError('instruction_aa_mismatch')
                total = row['calls']['main']['instructions']
                row['erased_percent'] = {arm: 100 * (total - row['calls'][arm]['instructions']) / total
                                         for arm in args.arms}
                row['complete'] = True
                save()
                print(json.dumps(dict(id=identity, depth=expected['depth'], complete=True)), flush=True)
        if {row['id'] for row in report['files']} != args.only:
            raise ValueError('incomplete_input_set')
        if not args.audit_only:
            report['median_erased_percent'] = {arm: statistics.median(row['erased_percent'][arm] for row in report['files'])
                                               for arm in args.arms}
        report['complete'] = True
    finally:
        pool.close()
        save()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='action', required=True)
    patch = sub.add_parser('erase')
    patch.add_argument('--filter', type=pathlib.Path, required=True)
    kinds = ('deblock', 'sao10', 'deblock10', 'deblock-write', 'deblock8-gather', 'sao8-validation', 'sao8-tail', 'sao8-protection')
    patch.add_argument('--kind', choices=kinds, required=True)
    patch.add_argument('--diff', type=pathlib.Path, required=True)
    patch.add_argument('--driver', type=pathlib.Path)
    run_parser = sub.add_parser('run')
    run_parser.add_argument('--binaries', type=pathlib.Path, required=True)
    run_parser.add_argument('--oracle-bin', type=pathlib.Path, required=True)
    run_parser.add_argument('--out', type=pathlib.Path, required=True)
    run_parser.add_argument('--baseline', required=True)
    run_parser.add_argument('--arms', nargs='+', choices=kinds, default=['deblock', 'sao10'])
    run_parser.add_argument('--only', required=True)
    run_parser.add_argument('--depth', type=int, choices=(0, 8, 10), required=True)
    run_parser.add_argument('--frames', type=int, choices=range(1, 33), default=1)
    run_parser.add_argument('--audit-only', action='store_true')
    args = parser.parse_args()
    if args.action == 'erase':
        driver_arm = args.kind in ('deblock-write', 'deblock8-gather', 'sao8-protection')
        path = args.driver if driver_arm else args.filter
        if path is None:
            raise ValueError('missing_driver_path')
        original = path.read_text()
        modified = erased_driver(original, args.kind) if driver_arm else erased(original, args.kind)
        args.diff.write_text(''.join(difflib.unified_diff(original.splitlines(True), modified.splitlines(True),
                                                        fromfile='filter.rs', tofile=f'{args.kind}/filter.rs')))
        path.write_text(modified)
    else:
        run(args)


if __name__ == '__main__':
    main()

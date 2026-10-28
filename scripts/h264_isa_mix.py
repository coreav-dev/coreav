#!/usr/bin/env python3
"""How many of our executed instructions are vector instructions.

Joins Callgrind's per-instruction counts (`--dump-instr=yes`) against our
own disassembly, so every executed instruction is classified by whether it
touches a vector register. Only our own binaries are disassembled; a
third-party binary is never an input here.

    valgrind --tool=callgrind --dump-instr=yes --callgrind-out-file=cg.out BIN ARGS
    h264_isa_mix.py --binary BIN --callgrind cg.out
"""
import argparse
import collections
import re
import subprocess

DISASM = re.compile(r'^\s*([0-9a-f]+):\t[0-9a-f ]+\t([a-z0-9.]+)\s*(.*)$')
SYMBOL = re.compile(r'^([0-9a-f]+)\s+\S+\s+(.+)$')
VECTOR_REG = re.compile(r'%[xyz]mm\d')
# Callgrind compresses names: "fn=(7) name" defines id 7, a later
# "fn=(7)" refers back to it. Same for ob= and fl=.
COMPRESSED = re.compile(r'^(c?)(ob|fn)=\((\d+)\)(?:\s+(.*))?$')


def disassemble(binary):
    out = subprocess.run(['objdump', '-d', binary],
                         capture_output=True, text=True, check=True).stdout
    table = {}
    for line in out.splitlines():
        m = DISASM.match(line)
        if m:
            table[int(m.group(1), 16)] = (m.group(2), m.group(3))
    return table


def symbols(binary):
    out = subprocess.run(['nm', '--defined-only', binary],
                         capture_output=True, text=True).stdout
    found = {}
    for line in out.splitlines():
        m = SYMBOL.match(line)
        if m:
            found.setdefault(m.group(2).strip(), int(m.group(1), 16))
    return found


def parse(path, binary):
    """(address -> Ir) for the main object, plus fn name -> first address."""
    counts = collections.Counter()
    first = {}
    names = {'ob': {}, 'fn': {}}
    name, position, mine = None, 0, False
    # The cost line after a "calls=" line is the call's inclusive
    # cost at the call site, not instructions executed there.
    skip_next_cost = False
    with open(path) as handle:
        for line in handle:
            line = line.rstrip('\n')
            m = COMPRESSED.match(line)
            if m:
                called, kind, ident, text = m.groups()
                if text:
                    # A "cob="/"cfn=" line names the callee, in the same id
                    # namespace, and is often where an object is first named.
                    names[kind][ident] = text
                if called:
                    continue
                resolved = names[kind].get(ident, '')
                if kind == 'ob':
                    mine = binary in resolved
                else:
                    name = resolved
                position = 0
                continue
            if line.startswith('calls='):
                skip_next_cost = True
                continue
            if line.startswith(('ob=', 'fn=', 'fl=', 'fi=', 'fe=',
                                'cob=', 'cfn=', 'cfl=', 'jump=', 'jcnd=')):
                continue
            if not line or not line[0] in '0123456789+-*':
                continue
            parts = line.split()
            if len(parts) < 3:
                continue
            where = parts[0]
            if where == '*':
                pass
            elif where[0] in '+-':
                position += int(where)
            else:
                try:
                    position = int(where, 16)
                except ValueError:
                    continue
            if skip_next_cost:
                skip_next_cost = False
                continue
            if not mine:
                continue
            try:
                counts[position] += int(parts[2])
            except ValueError:
                continue
            if name and name not in first:
                first[name] = position
    return counts, first


def bias(first, syms):
    """Load bias from symbols seen in both the profile and the binary."""
    votes = collections.Counter()
    for name, runtime in first.items():
        fileaddr = syms.get(name)
        if fileaddr is not None:
            votes[runtime - fileaddr] += 1
    if not votes:
        return None, 0
    shift, hits = votes.most_common(1)[0]
    return shift, hits


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--binary', required=True)
    ap.add_argument('--callgrind', required=True)
    args = ap.parse_args()

    table = disassemble(args.binary)
    counts, first = parse(args.callgrind, args.binary)
    syms = symbols(args.binary)
    shift, hits = bias(first, syms)
    if shift is None:
        raise SystemExit('no symbol matched between the profile and the binary')
    print(f'load bias 0x{shift:x}, agreed by {hits} symbols')

    by_class = collections.Counter()
    by_mnemonic = collections.Counter()
    unmatched = 0
    for address, n in counts.items():
        entry = table.get(address - shift)
        if entry is None:
            unmatched += n
            continue
        kind = 'vector' if VECTOR_REG.search(entry[1]) else 'scalar'
        by_class[kind] += n
        by_mnemonic[(kind, entry[0])] += n

    total = sum(by_class.values())
    print(f'matched {total:,} instructions, unmatched {unmatched:,} '
          f'({unmatched / (total + unmatched) * 100:.1f}%)')
    for kind in ('vector', 'scalar'):
        print(f'  {kind:7} {by_class[kind]:>15,}  {by_class[kind]/total*100:5.1f}%')
    print('\ntop vector mnemonics:')
    shown = 0
    for (kind, mnemonic), n in by_mnemonic.most_common():
        if kind == 'vector' and shown < 12:
            print(f'  {mnemonic:14} {n:>14,}  {n/total*100:5.2f}%')
            shown += 1


if __name__ == '__main__':
    main()

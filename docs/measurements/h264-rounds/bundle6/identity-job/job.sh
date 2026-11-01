#!/bin/bash
# Bundle 6: output identity of k against base, fail-closed (ident.sh), on the 32 training streams at 1 and 8 threads
# with and without fast, and on the 64 held-out streams at 1 and 8 threads with fast.
set -u -o pipefail
cd "$(dirname "$0")"
mkdir -p out; rm -f out/identity.txt out/identity.stop
chmod +x decode-* ident.sh
{ uname -a; grep -m1 'model name' /proc/cpuinfo; nproc; sha256sum decode-*; } > out/host.txt 2>&1
{ for f in es/*.h264; do for t in 1 8; do for v in fast nf; do echo "train $f $t $v"; done; done; done
  for f in es-held/*.h264; do for t in 1 8; do echo "held $f $t fast"; done; done; } > out/calls.txt
xargs -P 8 -L1 ./ident.sh < out/calls.txt
python3 - <<'EOF'
import collections
calls = [l.split() for l in open('out/calls.txt')]
rows = [l.split() for l in open('out/identity.txt')] if __import__('os').path.exists('out/identity.txt') else []
c = collections.Counter((r[0], r[2], r[3], r[-1]) for r in rows)
bad = [' '.join(r) for r in rows if r[-1] != 'same']
with open('out/identity-summary.txt', 'w') as o:
    for k, n in sorted(c.items()): print(*k, n, file=o)
    print(f'{len(rows)} of {len(calls)} calls recorded', file=o)
    print('complete: every call same' if not bad and len(rows) == len(calls) else 'NOT COMPLETE', *bad, sep='\n', file=o)
EOF
cat out/identity-summary.txt
grep -q '^complete' out/identity-summary.txt

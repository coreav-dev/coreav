"""order.py DATA... > order.txt: the decoder's sampled functions (perf cpu-clock), each stream's samples weighted to the
same total, in descending order of weight, mangled names (lld --symbol-ordering-file)."""
import collections, subprocess, sys
w = collections.Counter()
for d in sys.argv[1:]:
    out = subprocess.run(['perf', 'report', '-i', d, '--no-demangle', '--stdio', '--sort', 'sym', '-q', '--percent-limit', '0'],
                         capture_output=True, text=True).stdout
    for l in out.splitlines():
        p = l.split()
        if len(p) >= 3 and p[0].endswith('%') and p[1] == '[.]':
            w[p[2]] += float(p[0][:-1])
for n, x in w.most_common():
    print(n)

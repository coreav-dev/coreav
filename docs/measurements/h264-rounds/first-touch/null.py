"""null.py, run in pf-run1/: the null arm (base timed twice), median and largest deviation per stream over the 32.
Output: pf-run1/null.txt."""
import json, statistics as st
d, w = [], []
for c in ('sd', '720p', '1080p', '4k'):
    for s in json.load(open(f't8.{c}.json'))['streams']:
        d.append(s['null']['base_discard']); w.append(s['null']['base_write'])
print(f'null discard median {st.median(d):+.4f} max|dev| {max(map(abs, d)):.4f}; '
      f'write median {st.median(w):+.4f} max|dev| {max(map(abs, w)):.4f}')

import json, os, subprocess, hashlib, sys
FF = sys.argv[1]; BIN = sys.argv[2]; out = {}
L = json.load(open(os.path.expanduser('~/hd/es.json')))
for name, info in sorted(L.items()):
    p = os.path.expanduser(f'~/hd/es/{name}'); F = info['frames']
    r = subprocess.run([FF, '-nostdin', '-v', 'error', '-threads', '1', '-i', p, '-frames:v', str(F), '-fps_mode', 'passthrough', '-f', 'framemd5', '-'], capture_output=True, text=True)
    ff = [l.split(',') for l in r.stdout.splitlines() if l and not l.startswith('#')]
    ffm = [(int(x[4]), x[5].strip()) for x in ff]
    o = subprocess.run(['taskset', '-c', '2', BIN, '--frames', str(F), p], capture_output=True)
    data = o.stdout; pos = 0; mine = []
    for size, _ in ffm:
        chunk = data[pos:pos+size]; pos += size
        mine.append((len(chunk), hashlib.md5(chunk).hexdigest()))
    extra = len(data) - pos
    first = next((i for i, (a, b) in enumerate(zip(mine, ffm)) if a[0] != b[0] or a[1] != b[1][1] and False or a[1] != b[1]), None)
    same = first is None and extra == 0 and o.returncode == 0 and r.returncode == 0
    out[name] = {'ffmpeg_frames': len(ffm), 'ours_bytes_extra': extra, 'ours_exit': o.returncode, 'ff_exit': r.returncode, 'first_diff': first, 'identical': same}
    print(name, len(ffm), 'OK' if same else f'DIFF first={first} extra={extra} exit={o.returncode}/{r.returncode}', flush=True)
json.dump(out, open(os.path.expanduser('~/hd/identity.json'), 'w'), indent=1)
print('identical', sum(v['identical'] for v in out.values()), 'of', len(out))

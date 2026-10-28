"""scale.py ROUNDS OUT US_BIN [streams...]: wall, CPU, faults, switches of ours and FFmpeg at 1 (pinned cpu 2) and 8 threads."""
import json, os, subprocess, sys, time
R = int(sys.argv[1]); OUT = sys.argv[2]; US = sys.argv[3]; sel = sys.argv[4:]
ES = '$HOME/h264work/train/es/'
FF = '$HOME/h264work/ff902/ffmpeg-n9.0.2-17-g2a571b6068-linux64-gpl-9.0/bin/ffmpeg'
FR = {'sd': 1800, '720p': 900, '1080p': 300, '4k': 150}
files = sorted(f for f in os.listdir(ES) if not sel or f[:12] in sel)
def cmds(f):
    n = FR[f.split('.')[2]]; s = ES + f; o = {}
    for t in (1, 8):
        pin = ['taskset', '-c', '2'] if t == 1 else []
        o[f'us{t}'] = pin + [US, '--frames', str(n), '--threads', str(t), '--discard', s]
        o[f'ff{t}'] = pin + [FF, '-nostdin', '-v', 'error', '-threads', str(t), '-i', s, '-frames:v', str(n),
                             '-fps_mode', 'passthrough', '-f', 'null', '-']
    return o
def run(argv):
    t0 = time.perf_counter()
    p = subprocess.Popen(argv, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    _, st, ru = os.wait4(p.pid, 0); w = time.perf_counter() - t0
    if st: raise SystemExit(f'fail {argv}')
    return [w, ru.ru_utime + ru.ru_stime, ru.ru_stime, ru.ru_minflt, ru.ru_nvcsw, ru.ru_nivcsw]
res = {f: {k: [] for k in cmds(f)} for f in files}
for r in range(R + 1):
    for f in files:
        c = cmds(f); ks = list(c); ks = ks[r % 4:] + ks[:r % 4]
        for k in ks:
            m = run(c[k])
            if r: res[f][k].append(m)
    json.dump(res, open(OUT, 'w'))
    print('round', r, flush=True)

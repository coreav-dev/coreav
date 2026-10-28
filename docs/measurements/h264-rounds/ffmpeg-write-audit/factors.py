"""python3 -I factors.py OUT.json -> for the streams whose FFmpeg `-f rawvideo` run at the default frame-rate conversion differs from
`-fps_mode passthrough` (vsync*.txt), FFmpeg alone, one thread, pinned to core 2, five rounds interleaved: the default command's time,
the passthrough command's, and their ratio (the factor by which a stored write ratio over the default is too high), for FFmpeg 9.0.2 and 7.0.2.
ES files are at ~/tr/es (training) and ~/hd/es (held-out)."""
import json, os, statistics, subprocess, sys, time
FF = {'ff9': os.path.expanduser('~/ff9/x/ffmpeg-n9.0.2-17-g2a571b6068-linux64-gpl-9.0/bin/ffmpeg'), 'ff7': '/opt/coreav-oracle-bin/ffmpeg'}
S = [('~/tr/es', '3b1b441e23ae', '4k', 150), ('~/tr/es', '6da34958296c', '1080p', 300), ('~/tr/es', '8ae90cadb5bc', 'sd', 1800),
     ('~/tr/es', 'b41f70a7fa60', '1080p', 300), ('~/hd/es', '48640c5a3dd7', '1080p', 300), ('~/hd/es', '9d99b565f2ec', 'sd', 1800)]
def run(c):
    s = time.perf_counter(); subprocess.run(['taskset', '-c', '2'] + c, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    return time.perf_counter() - s
out = {}
for d, sid, cls, n in S:
    f = os.path.expanduser(f'{d}/{sid}.0.{cls}.h264')
    for name, ff in FF.items():
        base = [ff, '-nostdin', '-v', 'error', '-threads', '1', '-i', f, '-frames:v', str(n)]
        a, b = base + ['-f', 'rawvideo', '-'], base + ['-fps_mode', 'passthrough', '-f', 'rawvideo', '-']
        run(a); run(b)
        ta, tb = [], []
        for _ in range(5):
            ta.append(run(a)); tb.append(run(b))
        da, pb = statistics.median(ta), statistics.median(tb)
        out[f'{sid}.{name}'] = {'default': da, 'passthrough': pb, 'factor': pb / da}
        print(sid, name, 'default %.3f passthrough %.3f factor %.3f' % (da, pb, pb / da), flush=True)
json.dump(out, open(sys.argv[1], 'w'), indent=1)

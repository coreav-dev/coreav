"""python3 -I vsync.py ES_DIR FFMPEG DECODE -> per training stream: pictures FFmpeg writes with `-f rawvideo` at its default
frame-rate conversion and with `-fps_mode passthrough`, against the pictures ours writes, at the class frame counts, and the
dup/drop counts FFmpeg reports. A stream whose timestamps are not a steady rate has duplicates or drops there."""
import os, re, subprocess, sys
es, ff, dec = sys.argv[1:4]
FR = {'sd': 1800, '720p': 900, '1080p': 300, '4k': 150}
def count(argv):
    p = subprocess.Popen(argv, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    n = 0
    while chunk := p.stdout.read(1 << 22): n += len(chunk)
    return n, p.stderr.read().decode(errors='replace')
print('stream class frames ours_bytes ffmpeg_default_bytes ffmpeg_passthrough_bytes dup drop')
for name in sorted(os.listdir(es)):
    sid, cls = name.split('.')[0], name.split('.')[2]
    f, path = FR[cls], os.path.join(es, name)
    base = [ff, '-nostdin', '-threads', '4', '-i', path, '-frames:v', str(f)]
    ours, _ = count([dec, '--frames', str(f), path])
    d, err = count(base + ['-v', 'info', '-f', 'rawvideo', '-'])
    m = re.findall(r'dup=(\d+) drop=(\d+)', err.replace('\r', '\n'))
    p, _ = count(base + ['-v', 'error', '-fps_mode', 'passthrough', '-f', 'rawvideo', '-'])
    dup, drop = m[-1] if m else ('0', '0')
    flag = '' if d == ours == p and dup == drop == '0' else '  <-- differs'
    print(f'{sid} {cls} {f} {ours} {d} {p} {dup} {drop}{flag}')

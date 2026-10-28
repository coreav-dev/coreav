import json, os, sys, shutil, hashlib, urllib.request, zipfile, time
M = json.load(open(os.path.expanduser('~/jvt/conformance-streams.json')))['streams']
OUT = os.path.expanduser('~/jvt/extracted'); ZIP = os.path.expanduser('~/jvt/zip')
os.makedirs(OUT, exist_ok=True); os.makedirs(ZIP, exist_ok=True)
BASE = "https://www.itu.int/wftp3/av-arch/jvt-site/draft_conformance"
def digest(p):
    h = hashlib.sha256()
    with open(p, 'rb') as f:
        for c in iter(lambda: f.read(1 << 20), b''): h.update(c)
    return h.hexdigest()
for s in M:
    if not s['in_scope']: continue
    dst = os.path.join(OUT, s['name'])
    if os.path.exists(os.path.join(dst, '.done')): continue
    zp = os.path.join(ZIP, s['zip']); ok = False
    for attempt in range(3):
        for sub in ('AVCv1', 'FRExt'):
            try:
                with urllib.request.urlopen(f"{BASE}/{sub}/{s['zip']}", timeout=300) as r, open(zp, 'wb') as o: shutil.copyfileobj(r, o)
            except Exception as e: last = e; continue
            if digest(zp) == s['zip_sha256']: ok = True; break
        if ok: break
        time.sleep(5)
    if not ok: print('FAIL', s['name']); continue
    os.makedirs(dst, exist_ok=True)
    with zipfile.ZipFile(zp) as z:
        for m in z.namelist():
            t = os.path.realpath(os.path.join(dst, m))
            assert t.startswith(os.path.realpath(dst) + os.sep) or t == os.path.realpath(dst)
        z.extractall(dst)
    for root, _, fs in os.walk(dst):
        for f in fs:
            if f.lower().endswith('.yuv'): os.remove(os.path.join(root, f))
    keep = os.path.basename(s['bitstream'])
    for root, _, fs in os.walk(dst):
        for f in fs:
            if f not in (keep, '.done'): os.remove(os.path.join(root, f))
    os.remove(zp); open(os.path.join(dst, '.done'), 'w').close()
    print('ok', s['name'], flush=True)

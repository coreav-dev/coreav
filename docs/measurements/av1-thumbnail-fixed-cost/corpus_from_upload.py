"""Stand-in for tools/stage3-report/corpus.py on the measuring VM: the files were uploaded with the job, so a
download is a copy from FILES, checked by SHA-256 as the bucket download is. No credentials exist here."""
import hashlib, os, pathlib, shutil
FILES = pathlib.Path(os.environ['ARMS_FILES'])
def credentials():
    return {}
def child_env():
    env = dict(os.environ)
    env['PYTHONDONTWRITEBYTECODE'] = '1'
    return env
def sha256_file(path):
    d = hashlib.sha256()
    with open(path, 'rb') as h:
        while c := h.read(1 << 22):
            d.update(c)
    return d.hexdigest()
def download(entry, dest):
    src = FILES / f"{entry['sha256']}.{entry['ext']}"
    if not src.is_file():
        return False
    os.link(src, dest) if not os.path.exists(dest) else None
    return sha256_file(dest) == entry['sha256']

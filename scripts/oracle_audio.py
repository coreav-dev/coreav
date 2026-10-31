"""Input options for every oracle audio decode; see docs/design/ci-checks.md."""
import ast
from pathlib import Path
import re
import subprocess

INPUT_ARGS = tuple(Path(__file__).with_name('oracle-audio-args.txt').read_text().split())
ARG_FILE = 'scripts/oracle-audio-args.txt'
VIDEO_OR_COPY = {
    ('docs/measurements/h264-n2-heldout/tools/identity.py', ''): 'raw H.264 elementary streams only',
    ('scripts/extract_check.py', 'framemd5'): 'copied audio packets',
    ('scripts/remux_check.py', 'framemd5'): 'copied packets only, including sanitize',
    ('scripts/mkv_remux_check.py', 'framemd5'): 'copies audio; only Annex B video is decoded',
    ('scripts/h264_agreement.py', 'ffmpeg_frames'): 'video stream only',
    ('scripts/oracle.py', 'decode'): 'video streams only',
    ('scripts/thumbnail_oracle.py', 'framemd5'): 'video stream only',
    ('scripts/transcode_check.py', 'native_conformance'): 'video stream only',
}
FORMATS = {'framemd5', 'f32le', 's32le', 's16le', 'pcm_f32le'}


def python_problems(text, rel):
    tree = ast.parse(text)
    imported = any(isinstance(n, ast.ImportFrom) and n.module == 'oracle_audio' and
                   any(a.name == 'INPUT_ARGS' and a.asname == 'AUDIO_DECODE_INPUT' for a in n.names)
                   for n in tree.body)
    assigned = any(isinstance(n, ast.Name) and n.id == 'AUDIO_DECODE_INPUT' and isinstance(n.ctx, ast.Store)
                   for n in ast.walk(tree))
    for scope in ast.walk(tree):
        if not isinstance(scope, (ast.FunctionDef, ast.AsyncFunctionDef, ast.Module)):
            continue
        body = [n for n in scope.body if not isinstance(n, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef))]
        lists = [n for statement in body for n in ast.walk(statement) if isinstance(n, (ast.List, ast.Tuple))]
        strings = {n.value for seq in lists for n in ast.walk(seq) if isinstance(n, ast.Constant) and isinstance(n.value, str)}
        if not strings & FORMATS or (rel, getattr(scope, 'name', '')) in VIDEO_OR_COPY:
            continue
        for seq in lists:
            values = [n.value if isinstance(n, ast.Constant) else None for n in seq.elts]
            codecs = [values[i + 1] for i, value in enumerate(values[:-1])
                      if value in ('-c', '-codec', '-c:a', '-codec:a', '-acodec')]
            if '-i' not in values or 'lavfi' in values or codecs == ['copy']:
                continue
            for at, value in enumerate(values):
                if value != '-i':
                    continue
                before = seq.elts[max(0, at - 1):at]
                if not imported or assigned or not any(isinstance(n, ast.Starred) and isinstance(n.value, ast.Name) and
                                                       n.value.id == 'AUDIO_DECODE_INPUT' for n in before):
                    yield f'{rel}:{seq.lineno}: audio decode input must use oracle_audio.INPUT_ARGS as AUDIO_DECODE_INPUT before -i'


def problems(root):
    paths = subprocess.check_output(['git', '-C', str(root), 'ls-files', '-z']).decode().split('\0')
    for rel in filter(None, paths):
        path = Path(rel)
        if path.suffix not in ('.py', '.sh', '.rs', '.yml', '.yaml') or \
                ('tests' in path.parts and path.suffix != '.rs') or 'selftest' in path.name:
            continue
        if rel == 'scripts/oracle_audio.py':
            continue
        text = (root / rel).read_text()
        if re.search(r'-threads:a(?:\b|:)', text):
            yield f'{rel}: use {ARG_FILE}, not a second audio thread policy'
        if path.suffix == '.py':
            yield from python_problems(text, rel)
        elif path.suffix == '.rs':
            for command in re.findall(r'Command::new\([^;]*?\.output\(\)', text, re.S):
                if any(f'"{fmt}"' in command for fmt in ('f32le', 's32le', 's16le')) and ARG_FILE not in command:
                    yield f'{rel}: raw audio oracle must include {ARG_FILE} before -i'
        elif re.search(r'\b(?:f32le|s32le|framemd5)\b', text) and '-i ' in text and 'ffmpeg' in text.lower() and ARG_FILE not in text:
            yield f'{rel}: audio-capable frame oracle must read {ARG_FILE} before -i'

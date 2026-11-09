import os
from pathlib import Path
import shutil
import subprocess
import sys

sys.path.insert(0, str(Path('diagnostic').resolve()))
import arms

BASE = os.environ['BASE']
PIN = 'a797f127eb54b929768a47b9abbd6253a44aa8ea'
ROOT = Path('/tmp/prune')
(ROOT / 'bin').mkdir(parents=True, exist_ok=True)
Path('ab').mkdir(exist_ok=True)
Path('ab/prepare.py').write_bytes(subprocess.check_output(['git', 'show', f'{PIN}:ab/prepare.py']))
driver = Path('diagnostic/encode.rs').read_text()
env = dict(os.environ, CARGO_PROFILE_RELEASE_DEBUG='1', RUSTFLAGS='-C force-frame-pointers=yes', CARGO_TARGET_DIR=str(ROOT / 'target'))
for arm in ['main', *os.environ['ARMS'].split(',')]:
    tree = ROOT / arm
    head = os.environ['PR_SHA'] if arm == 'pr' else BASE
    subprocess.run(['git', 'worktree', 'add', '--detach', tree, head], check=True)
    if arm not in ('main', 'pr'):
        arms.apply(tree, arm)
    (tree / 'crates/coreav-h264-encoder/examples').mkdir(exist_ok=True)
    (tree / 'crates/coreav-h264-encoder/examples/map.rs').write_text(driver)
    with (tree / 'crates/coreav-h264-encoder/Cargo.toml').open('a') as f:
        f.write('\n[[example]]\nname = "map"\nrequired-features = ["full"]\n')
    subprocess.run(['git', 'diff', '--stat'], cwd=tree, check=True)
    subprocess.run(['cargo', 'build', '--locked', '--release', '-p', 'coreav-h264-encoder', '--features', 'full', '--example', 'map'], cwd=tree, check=True, env=env)
    shutil.copyfile(ROOT / 'target/release/examples/map', ROOT / 'bin' / arm)
    (ROOT / 'bin' / arm).chmod(0o755)

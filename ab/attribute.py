from pathlib import Path
import re

root = Path(__file__).resolve().parents[1]
entries = {
    'kernels.rs': ['sad', 'satd'],
    'transform.rs': ['forward_scalar', 'inverse_scalar', 'satd', 'zero_scalar', 'levels_scalar', 'scaled_scalar'],
}
for filename, names in entries.items():
    path = root / 'crates/coreav-h264-encoder/src' / filename
    text = path.read_text()
    for name in names:
        pattern = rf'(?m)^(\s*)(pub\(crate\) )?fn {name}\('
        text, count = re.subn(pattern, rf'\1#[inline(never)]\n\1\2fn {name}(', text)
        if count != 1:
            raise ValueError('scalar entry point missing or ambiguous')
    path.write_text(text)

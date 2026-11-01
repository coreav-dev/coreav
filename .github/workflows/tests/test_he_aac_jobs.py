"""The HE-AAC jobs of audio-conformance.yml (docs/design/audio-decode.md,
"HE-AAC bars") read untrusted media and the corpus store: no checkout of
theirs leaves the token in the local Git configuration."""
import pathlib
import re
import unittest

WORKFLOW = pathlib.Path(__file__).resolve().parents[1] / 'audio-conformance.yml'
JOB = re.compile(r'^  ([A-Za-z0-9_-]+):\s*$')


def jobs(text):
    out, name = {}, None
    for line in text.splitlines():
        found = JOB.match(line)
        if found:
            name = found.group(1)
            out[name] = []
        elif name:
            out[name].append(line)
    return out


def checkouts(lines):
    """Each checkout step of a job: the persist-credentials value of its
    with: block, None when it has none."""
    found = []
    for at, line in enumerate(lines):
        if 'uses: actions/checkout@' not in line:
            continue
        indent = len(line) - len(line.lstrip(' -'))
        value = None
        for later in lines[at + 1:]:
            if later.strip().startswith('- ') and len(later) - len(later.lstrip(' ')) < indent:
                break
            if later.strip().startswith('persist-credentials:'):
                value = later.split(':', 1)[1].strip()
        found.append(value)
    return found


class HeAacJobs(unittest.TestCase):
    def test_every_checkout_drops_the_token(self):
        he = {k: v for k, v in jobs(WORKFLOW.read_text()).items() if k.startswith('he-aac')}
        self.assertEqual(sorted(he), ['he-aac', 'he-aac-encode', 'he-aac-sources'])
        for name, lines in he.items():
            steps = checkouts(lines)
            self.assertTrue(steps, name)
            self.assertEqual(steps, ['false'] * len(steps), name)


if __name__ == '__main__':
    unittest.main()

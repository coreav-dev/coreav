"""The documentation site's pages (docs-site/, docs/design/docs-site.md) link only to what exists, and keep the public text rules.

- Every page docs.json lists exists, and every page exists in docs.json.
- A link into this repository (https://github.com/coreav-dev/coreav/blob/main/PATH or /tree/main/PATH) names a path on
  this commit, and its #anchor names a heading of that file.
- A link to another page is root-relative without the base path: Mintlify serves the site under /docs and adds it
  itself, so /docs/install would become /docs/docs/install.
- Every measurement page under docs/measurements is listed on the measurements page.
- Every file under docs/agents is listed in AGENTS.md, and every docs/agents link in that index is a file.
- No page breaks a rule of scripts/pr_text_check.py, or makes a speed claim against FFmpeg: that belongs on a
  measurement page, with its bars.

The help text of the command reference is checked by crates/coreav-cli/tests/docs_site.rs, and Mintlify's own build
and link check run in .github/workflows/docs-site.yml.

    python3 -I -m unittest discover -s scripts/tests -p 'test_*.py'
"""
import gzip
import hashlib
import json
import os
import pathlib
import re
import shlex
import subprocess
import sys
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[2]
SITE = ROOT / 'docs-site'
sys.path.insert(0, str(ROOT / 'scripts'))
import pr_text_check  # noqa: E402

REPO = re.compile(r'https://github\.com/coreav-dev/coreav/(?:blob|tree)/main/([^)#"\s]+)(?:#([^)"\s]+))?')
INTERNAL = re.compile(r'(?:\]\(|href=")(/[^)"\s]*)')
FASTER = re.compile(r'faster than ffmpeg', re.I)


def pages():
    """Each page's route ('index', 'commands/probe') and its file, snippets left out."""
    return {p.relative_to(SITE).with_suffix('').as_posix(): p
            for p in SITE.rglob('*.mdx') if p.relative_to(SITE).parts[0] != 'snippets'}


def listed(node):
    """Every page route in docs.json's navigation."""
    if isinstance(node, str):
        return [node]
    if isinstance(node, list):
        return [page for item in node for page in listed(item)]
    if isinstance(node, dict):
        return [page for key, value in node.items() if key in ('tabs', 'groups', 'pages') for page in listed(value)]
    return []


def slugs(markdown):
    """The anchors GitHub gives the headings of a Markdown file."""
    out, seen = set(), {}
    fenced = False
    for line in markdown.splitlines():
        if line.startswith('```'):
            fenced = not fenced
        heading = re.match(r'#{1,6} (.+)', line)
        if fenced or not heading:
            continue
        slug = re.sub(r'[^\w\- ]', '', heading.group(1).strip().lower()).replace(' ', '-')
        n = seen.get(slug, 0)
        seen[slug] = n + 1
        out.add(slug if n == 0 else f'{slug}-{n}')
    return out


class DocsSite(unittest.TestCase):
    def test_raw_audio_distinguishes_integer_copy_from_float_packing(self):
        text = (SITE / 'commands/ffmpeg.mdx').read_text()
        self.assertIn('S16 output copies integer bytes without a floating-point conversion', text)
        self.assertIn('F32 output scales the input samples to floats', text)

    def test_raw_audio_documents_platform_gates_only_for_changed_rates(self):
        text = (SITE / 'commands/ffmpeg.mdx').read_text()
        self.assertIn('Changed-rate headerless output requires the platform-specific exact registration', text)
        self.assertIn('Unchanged-rate identity output is platform-independent', text)
        self.assertNotIn('Headerless output additionally requires the platform-specific', text)

    def test_frame_output_documents_decoded_bytes_per_decoder(self):
        for name in ('docs/design/frame-output.md', 'docs-site/commands/ffmpeg.mdx'):
            text = (ROOT / name).read_text().lower()
            with self.subTest(page=name):
                self.assertTrue('4 gib per decoder' in text, f'{name}: name the decoded-byte limit and its per-decoder scope')
                self.assertFalse('decoded-sample limit' in text or 'total decoded samples' in text,
                                 f'{name}: max_decoded_bytes counts bytes, not samples')

    def test_frame_output_documents_native_decoder_session_timeout(self):
        for name in ('docs/design/frame-output.md', 'docs-site/commands/ffmpeg.mdx'):
            text = (ROOT / name).read_text()
            with self.subTest(page=name):
                for fact in ('60-second decoder-session deadline', 'clock-supported', 'slow output'):
                    self.assertTrue(fact in text, f'{name}: describe the native H.264 session deadline, including time spent writing output')
                self.assertFalse('per decoder call' in text, f'{name}: the native H.264 deadline is not renewed for each call')

    def test_pnpm_approval_documents_both_supported_settings(self):
        platform = (SITE / 'platforms.mdx').read_text()
        for fact in ('pnpm 10.0 through 10.4', 'pnpm 10.5 through 10.x', 'pnpm 11 and later',
                     'onlyBuiltDependencies', 'allowBuilds', 'package.json', 'pnpm-workspace.yaml',
                     'preserve existing approvals', 'copy the complete list', 'pnpm rebuild coreav',
                     'neverBuiltDependencies', 'do not combine'):
            self.assertIn(fact, platform, f'docs-site/platforms.mdx: restore the approval rule {fact}')
        for name in ('packages/cli/README.md', 'docs/design/npm-cli.md'):
            text = (ROOT / name).read_text()
            with self.subTest(page=name):
                self.assertIn('https://coreav.dev/docs/platforms#install-scripts', text,
                              f'{name}: link the one owner of version-specific pnpm settings')
                self.assertNotRegex(text, r'onlyBuiltDependencies|allowBuilds',
                                    f'{name}: keep version-specific pnpm settings only on the platform page')
                self.assertIn('pnpm rebuild coreav', text,
                              f'{name}: tell existing installations to run pnpm rebuild coreav')

    def test_bun_trust_documents_the_explicit_list(self):
        platform = (SITE / 'platforms.mdx').read_text()
        for fact in ('trustedDependencies', 'replaces its default list', 'any required packages'):
            self.assertIn(fact, platform, f'docs-site/platforms.mdx: restore the Bun trust rule {fact}')
        self.assertIn('https://coreav.dev/docs/platforms#install-scripts',
                      (ROOT / 'packages/cli/README.md').read_text())

    def test_navigation_and_pages_agree(self):
        navigation = listed(json.loads((SITE / 'docs.json').read_text())['navigation'])
        self.assertEqual(sorted(set(navigation)), sorted(navigation), 'docs-site/docs.json lists a page twice')
        missing = sorted(set(navigation) - set(pages()))
        unlisted = sorted(set(pages()) - set(navigation))
        self.assertFalse(missing, f'docs-site/docs.json lists pages that do not exist: {missing}')
        self.assertFalse(unlisted, f'pages that docs-site/docs.json does not list, which no reader can reach: {unlisted}')

    def test_links_into_the_repository_resolve(self):
        for route, page in pages().items():
            for path, anchor in REPO.findall(page.read_text()):
                target = ROOT / path
                with self.subTest(page=route, link=path):
                    self.assertTrue(target.exists(), f'docs-site/{route}.mdx links {path}, which does not exist on this commit')
                    if anchor:
                        self.assertTrue(anchor in slugs(target.read_text()),
                                        f'docs-site/{route}.mdx links {path}#{anchor}, but {path} has no such heading')

    def test_links_between_pages_resolve_without_the_base_path(self):
        routes = set(pages())
        for route, page in pages().items():
            for link in INTERNAL.findall(page.read_text()):
                target = link.split('#')[0].strip('/') or 'index'
                with self.subTest(page=route, link=link):
                    self.assertFalse(link == '/docs' or link.startswith('/docs/'),
                                     f'docs-site/{route}.mdx links {link}: drop the /docs prefix, which Mintlify adds as the base path')
                    if not target.startswith(('logo/', 'favicon')):
                        self.assertTrue(target in routes or f'{target}/index' in routes,
                                        f'docs-site/{route}.mdx links {link}, which is no page')

    def test_every_measurement_page_is_listed(self):
        index = (SITE / 'measurements.mdx').read_text()
        for page in sorted((ROOT / 'docs' / 'measurements').glob('*.md')):
            if page.name == 'README.md':
                continue
            with self.subTest(page=page.name):
                self.assertTrue(f'docs/measurements/{page.name})' in index,
                                f'docs/measurements/{page.name} is not listed on docs-site/measurements.mdx: add a row for it')

    def replay_shell_fixture(self, directory):
        page = (ROOT / 'docs/measurements/codec-v2-images.md').read_text()
        block = re.search(r'```sh\n(.*?)\n```', page, re.S)
        self.assertIsNotNone(block)
        programs = directory / 'bin'
        programs.mkdir()
        driver = programs / 'python3'
        driver.write_text('#!/bin/sh\nexec ' + shlex.quote(sys.executable) + ' -I - "$@" <<\'PY\'\n'
                          'import os, pathlib, sys\n'
                          'if os.environ["COREAV_REPLAY_TEST_FAIL"] == "1":\n'
                          '    print("synthetic replay failure", file=sys.stderr)\n'
                          '    sys.exit(2)\n'
                          'output = pathlib.Path(sys.argv[sys.argv.index("--summary") + 1])\n'
                          'output.write_bytes(pathlib.Path(os.environ["COREAV_REPLAY_TEST_SUMMARY"]).read_bytes())\n'
                          'PY\n')
        driver.chmod(0o755)
        return block.group(1), {**os.environ, 'TMPDIR': str(directory),
                                'PATH': str(programs) + os.pathsep + os.environ['PATH'],
                                'COREAV_REPLAY_TEST_FAIL': '0',
                                'COREAV_REPLAY_TEST_SUMMARY': str(ROOT / 'docs/measurements/codec-v2-images/summary.json')}

    def test_v2_image_evidence_matches_its_recorded_digests(self):
        data = ROOT / 'docs/measurements/codec-v2-images'
        digests = json.loads((data / 'evidence-digests.json').read_text())
        for name, expected in digests['files'].items():
            with self.subTest(file=name):
                self.assertEqual(hashlib.sha256((data / name).read_bytes()).hexdigest(), expected)
        for name, expected in digests['decompressed'].items():
            with self.subTest(decompressed=name):
                self.assertEqual(hashlib.sha256(gzip.decompress((data / name).read_bytes())).hexdigest(), expected)
        retention = json.loads((data / 'retention.json').read_text())
        for key in ('evidence', 'binaries'):
            with self.subTest(archive=key):
                self.assertEqual(retention['archives'][key + '.zip']['sha256'],
                                 digests['artifacts'][key]['digest'].removeprefix('sha256:'))

    def test_v2_image_replay_cannot_pass_with_a_stale_summary(self):
        data = ROOT / 'docs/measurements/codec-v2-images'
        expected = (data / 'summary.json').read_bytes()
        with tempfile.TemporaryDirectory() as tmp:
            directory = pathlib.Path(tmp)
            command, env = self.replay_shell_fixture(directory)
            for valid in (True, False):
                with self.subTest(valid=valid):
                    summary = directory / 'v2-image-summary.json'
                    summary.write_bytes(expected)
                    result = subprocess.run(['/bin/sh', '-c', command], cwd=ROOT,
                                            env={**env, 'COREAV_REPLAY_TEST_FAIL': '0' if valid else '1'},
                                            capture_output=True, text=True)
                    if valid:
                        self.assertEqual(result.returncode, 0, result.stderr)
                    else:
                        self.assertIn('synthetic replay failure', result.stderr)
                        self.assertNotEqual(result.returncode, 0)
                    self.assertEqual(summary.read_bytes(), expected)

    def test_v2_image_replay_does_not_follow_a_preexisting_summary_symlink(self):
        with tempfile.TemporaryDirectory() as tmp:
            directory = pathlib.Path(tmp)
            command, env = self.replay_shell_fixture(directory)
            target = directory / 'synthetic-target.json'
            original = b'synthetic target, not a summary'
            target.write_bytes(original)
            summary = directory / 'v2-image-summary.json'
            summary.symlink_to(target)
            result = subprocess.run(['/bin/sh', '-c', command], cwd=ROOT,
                                    env=env, capture_output=True, text=True)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(target.read_bytes(), original)
            self.assertTrue(summary.is_symlink())
            self.assertEqual(set(directory.iterdir()), {target, summary, directory / 'bin'})

    def test_decoder_host_replay_requires_valid_success_and_specific_void_refusal(self):
        page = (ROOT / 'docs/measurements/transcode-decoder-host.md').read_text()
        block = re.search(r'```sh\n(.*?)\n```', page, re.S)
        self.assertIsNotNone(block)
        for case in ('ok', 'valid_failure', 'valid_mismatch', 'void_success', 'void_failure'):
            with self.subTest(case=case), tempfile.TemporaryDirectory() as tmp:
                directory = pathlib.Path(tmp)
                programs = directory / 'bin'
                programs.mkdir()
                git = programs / 'git'
                git.write_text('#!/bin/sh\nif [ "$2" = add ]; then mkdir -p "$4"; fi\n')
                git.chmod(0o755)
                driver = programs / 'python3'
                driver.write_text('#!/bin/sh\nexec ' + shlex.quote(sys.executable) + ' -I - "$@" <<\'PY\'\n'
                                  'import os, pathlib, sys\n'
                                  'output = pathlib.Path(sys.argv[sys.argv.index("--out") + 1])\n'
                                  'with open(os.environ["DOC_REPLAY_TRACE"], "a") as trace:\n'
                                  '    trace.write(output.name + "\\n")\n'
                                  'case = os.environ["DOC_REPLAY_CASE"]\n'
                                  'if output.name == "valid":\n'
                                  '    expected = pathlib.Path(os.environ["DOC_REPLAY_SUMMARY"]).read_bytes()\n'
                                  '    (output / "summary.json").write_bytes(b"{}" if case == "valid_mismatch" else expected)\n'
                                  '    sys.exit(7 if case == "valid_failure" else 0)\n'
                                  'if case == "void_success":\n'
                                  '    sys.exit(0)\n'
                                  'print("OSError: unexpected failure" if case == "void_failure" else\n'
                                  '      "ValueError: incomplete capture", file=sys.stderr)\n'
                                  'sys.exit(1)\n'
                                  'PY\n')
                driver.chmod(0o755)
                trace = directory / 'trace'
                env = {**os.environ, 'PATH': str(programs) + os.pathsep + os.environ['PATH'],
                       'TMPDIR': str(directory), 'DOC_REPLAY_CASE': case, 'DOC_REPLAY_TRACE': str(trace),
                       'DOC_REPLAY_SUMMARY': str(ROOT / 'docs/measurements/transcode-decoder-host/valid/summary.json')}
                result = subprocess.run(['/bin/sh', '-c', block.group(1)], cwd=ROOT, env=env,
                                        capture_output=True, text=True, timeout=30)
                if case == 'ok':
                    self.assertEqual(result.returncode, 0, result.stderr)
                else:
                    self.assertNotEqual(result.returncode, 0)
                expected = ['valid'] if case.startswith('valid_') else ['valid', 'void']
                self.assertEqual(trace.read_text().splitlines(), expected)

    def test_every_agents_area_file_is_listed(self):
        index = (ROOT / 'AGENTS.md').read_text()
        area = ROOT / 'docs' / 'agents'
        for page in sorted(area.glob('*.md')):
            with self.subTest(page=page.name):
                self.assertTrue(f'docs/agents/{page.name})' in index,
                                f'docs/agents/{page.name} is not listed in AGENTS.md: add a row for it')
        for name in sorted(set(re.findall(r'docs/agents/([A-Za-z0-9._-]+\.md)', index))):
            with self.subTest(link=name):
                self.assertTrue((area / name).is_file(),
                                f'AGENTS.md links docs/agents/{name}, which is not a file: add the file or remove that row')

    def test_public_text(self):
        for page in sorted(SITE.rglob('*')):
            if page.suffix not in ('.mdx', '.json'):
                continue
            text = page.read_text()
            name = page.relative_to(ROOT).as_posix()
            with self.subTest(page=name):
                self.assertEqual(pr_text_check.matches(text, []), [], f'{name} breaks a rule of scripts/pr_text_check.py')
                self.assertIsNone(FASTER.search(text),
                                  f'{name} makes a speed claim against FFmpeg: link the measurement page that states the bar and the result instead')


if __name__ == '__main__':
    unittest.main()

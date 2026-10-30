"""The documentation site's pages (docs-site/, docs/design/docs-site.md) link only to what exists, and keep the public text rules.

- Every page docs.json lists exists, and every page exists in docs.json.
- A link into this repository (https://github.com/coreav-dev/coreav/blob/main/PATH or /tree/main/PATH) names a path on
  this commit, and its #anchor names a heading of that file.
- A link to another page is root-relative without the base path: Mintlify serves the site under /docs and adds it
  itself, so /docs/install would become /docs/docs/install.
- Every measurement page under docs/measurements is listed on the measurements page.
- No page breaks a rule of scripts/pr_text_check.py, or makes a speed claim against FFmpeg: that belongs on a
  measurement page, with its bars.

The help text of the command reference is checked by crates/coreav-cli/tests/docs_site.rs, and Mintlify's own build
and link check run in .github/workflows/docs-site.yml.

    python3 -I -m unittest discover -s scripts/tests -p 'test_*.py'
"""
import json
import pathlib
import re
import sys
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

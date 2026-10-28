#!/usr/bin/env python3
"""Takes the snapshot the command-coverage set is extracted from
(tools/compat-corpus/commands-v1/README.md).

Asks the public Stack Exchange API for the highest-voted Stack Overflow
questions of a tag and for their answers, and writes one JSON file holding
the answers with their bodies, scores and content licences. The snapshot is
the input of `extract.py`; it is not committed, because it holds whole post
bodies. What is committed is the extracted invocations, the snapshot's date
and its question and answer ids, so that anyone can take the snapshot again
or rebuild the set from the public Stack Exchange data dump.

    collect.py --tag ffmpeg --questions 500 --out snapshot-ffmpeg.json

Deterministic given the site's state: the questions are the first N of the
tag ordered by score, descending, and the answers are all of theirs. Scores
change, so the snapshot's date is part of the set's provenance.

The API is used without a key: 300 requests a day per address, and this
takes about (N / 100) * 2 of them. Python 3.11 standard library only.
"""
import argparse
import gzip
import json
import pathlib
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

API = 'https://api.stackexchange.com/2.3'
SITE = 'stackoverflow'
# The question fields the set's provenance needs, and the answer fields plus
# the body the extraction reads. `withbody` adds `body` to the default
# filter; the rest are in it.
FILTER_QUESTIONS = 'withbody'
FILTER_ANSWERS = 'withbody'
PAGE = 100
RETRIES = 4


def get(path, **params):
    """One API call, decoded. Retries a throttle or a server error."""
    params = {'site': SITE, **params}
    url = f'{API}/{path}?' + urllib.parse.urlencode(params)
    for attempt in range(RETRIES):
        try:
            with urllib.request.urlopen(url, timeout=60) as response:
                raw = response.read()
                if response.headers.get('Content-Encoding') == 'gzip':
                    raw = gzip.decompress(raw)
                body = json.loads(raw)
        except (urllib.error.URLError, TimeoutError, json.JSONDecodeError) as error:
            if attempt == RETRIES - 1:
                raise SystemExit(f'collect: {url}: {error}')
            time.sleep(2 ** attempt)
            continue
        if 'error_id' in body:
            raise SystemExit(f'collect: {url}: {body.get("error_message")}')
        backoff = body.get('backoff')
        if backoff:
            time.sleep(backoff + 1)
        return body
    raise SystemExit(f'collect: {url}: out of retries')


def questions(tag, wanted):
    """The `wanted` highest-voted questions of the tag, score descending."""
    out, page = [], 1
    while len(out) < wanted:
        body = get('questions', tagged=tag, sort='votes', order='desc',
                   page=page, pagesize=PAGE, filter=FILTER_QUESTIONS)
        out.extend(body['items'])
        if not body.get('has_more'):
            break
        page += 1
    return out[:wanted]


def answers(ids):
    """Every answer of those question ids."""
    out = []
    for start in range(0, len(ids), PAGE):
        batch = ';'.join(str(i) for i in ids[start:start + PAGE])
        page = 1
        while True:
            body = get(f'questions/{batch}/answers', sort='votes', order='desc',
                       page=page, pagesize=PAGE, filter=FILTER_ANSWERS)
            out.extend(body['items'])
            if not body.get('has_more'):
                break
            page += 1
    return out


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--tag', required=True)
    parser.add_argument('--questions', type=int, default=500)
    parser.add_argument('--out', type=pathlib.Path, required=True)
    args = parser.parse_args()

    asked = questions(args.tag, args.questions)
    ids = [q['question_id'] for q in asked]
    found = answers(ids)
    snapshot = {
        'site': SITE,
        'tag': args.tag,
        'sort': 'votes desc',
        'taken': time.strftime('%Y-%m-%d', time.gmtime()),
        'questions': [{k: q[k] for k in ('question_id', 'score', 'title', 'link', 'tags')
                       if k in q} for q in asked],
        'answers': found,
    }
    args.out.write_text(json.dumps(snapshot), encoding='utf-8')
    print(f'{args.tag}: {len(asked)} questions, {len(found)} answers -> {args.out}')
    return 0


if __name__ == '__main__':
    sys.exit(main())

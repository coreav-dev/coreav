"""Validate the build-time decoder admission record, without decoding media."""

import json
from pathlib import Path
import re


ROOT = Path(__file__).resolve().parents[1]
RECORD = ROOT / 'docs/compat/audio-decoder-eligibility.json'
REQUIREMENTS = {'mpeg1_layer3', 'aac_lc', 'supported_mp4_timeline'}


def heading_anchors(markdown):
    anchors, counts = set(), {}
    fenced = False
    for line in markdown.splitlines():
        if line.startswith('```'):
            fenced = not fenced
        heading = re.match(r'#{1,6} (.+)', line)
        if fenced or not heading:
            continue
        slug = re.sub(r'[^\w\- ]', '', heading.group(1).strip().lower()).replace(' ', '-')
        count = counts.get(slug, 0)
        counts[slug] = count + 1
        anchors.add(slug if count == 0 else f'{slug}-{count}')
    return anchors


def validate(record):
    def require(condition, message):
        if not condition:
            raise ValueError(message)

    require(isinstance(record, dict) and set(record) == {
        'schema', 'oracle_version', 'decoders'}, 'invalid record fields')
    require(record['schema'] == 'coreav.audio-decoder-eligibility/1', 'unknown schema')
    require(record['oracle_version'] == '9.0.2', 'unregistered oracle version')
    require(isinstance(record['decoders'], list), 'decoders must be an array')
    codecs = set()
    for row in record['decoders']:
        require(isinstance(row, dict) and set(row) == {
            'codec', 'kind', 'decoder_eligible', 'presentations', 'evidence',
            'limitations'}, 'invalid decoder fields')
        codec = row['codec']
        require(isinstance(codec, str) and codec in {'mp3', 'opus', 'aac'}, 'unknown codec')
        require(codec not in codecs, 'duplicate codec')
        codecs.add(codec)
        require(row['kind'] == 'conformance', 'lossy decoder rows use only conformance')
        require(type(row['decoder_eligible']) is bool, 'eligibility must be boolean')
        require(codec != 'aac' or not row['decoder_eligible'],
                'AAC eligibility requires a registered ISO conformance procedure')
        require(isinstance(row['limitations'], list) and all(
            isinstance(value, str) and value for value in row['limitations']),
            'limitations must contain nonempty strings')
        evidence = row['evidence']
        require(isinstance(evidence, str), 'evidence must be a documentation link')
        path, separator, anchor = evidence.partition('#')
        require(separator and path.startswith('docs/') and anchor,
                'evidence must have a documentation anchor')
        target = (ROOT / path).resolve()
        require(target.is_relative_to(ROOT / 'docs') and target.is_file(),
                'evidence must exist inside docs')
        require(anchor in heading_anchors(target.read_text()), 'evidence heading does not exist')
        require(isinstance(row['presentations'], list), 'presentations must be an array')
        containers = set()
        for presentation in row['presentations']:
            require(isinstance(presentation, dict) and set(presentation) == {
                'container', 'eligible', 'requirements'}, 'invalid presentation fields')
            container = presentation['container']
            require(isinstance(container, str) and container in {'mp4', 'matroska'},
                    'unknown container')
            require(container not in containers, 'duplicate container')
            containers.add(container)
            require(type(presentation['eligible']) is bool, 'eligibility must be boolean')
            requirements = presentation['requirements']
            require(isinstance(requirements, list) and all(
                isinstance(value, str) and value in REQUIREMENTS for value in requirements),
                'unknown requirement')
            require(len(requirements) == len(set(requirements)), 'duplicate requirement')
            if presentation['eligible']:
                require(row['decoder_eligible'], 'presentation requires decoder eligibility')
                require(container == 'mp4' and 'supported_mp4_timeline' in requirements,
                        'presentation lacks registered timeline predicate')
                profile = {'mp3': 'mpeg1_layer3', 'aac': 'aac_lc'}.get(codec)
                require(profile is not None and profile in requirements,
                        'presentation lacks registered decoder profile predicate')


if __name__ == '__main__':
    validate(json.loads(RECORD.read_text()))
    print('audio decoder eligibility: valid')

from collections import Counter
import json
from pathlib import Path
import runpy
import sys
from threading import Lock
import boto3

root = Path(sys.argv[1])
requests, lock = Counter(), Lock()
original = boto3.client
def account(event_name, **kwargs):
    with lock:
        if requests.total() >= 10000:
            raise RuntimeError('request budget exhausted')
        requests[event_name.rsplit('.', 1)[-1]] += 1
def client(*args, **kwargs):
    result = original(*args, **kwargs)
    result.meta.events.register('before-send.s3', account)
    return result
boto3.client = client
sys.argv = ['ab/prepare.py', str(root/'work'), str(root/'bin')]
try:
    runpy.run_path('ab/prepare.py', run_name='__main__')
finally:
    (root/'requests.json').write_text(json.dumps(dict(requests), indent=2))

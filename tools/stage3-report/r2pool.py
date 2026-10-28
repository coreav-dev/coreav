"""Keep-alive SigV4 client for R2, copied from spike A's H7 adaptive run
(docs/spikes/spike-a/h7-adaptive/).
One `Pool` per timed run, so TCP and TLS setup is always timed. Credentials
come from the environment and are never printed; errors carry no URLs or
headers. The bucket is the credentials' `bucket`, or else R2_BUCKET."""
from datetime import datetime, timezone
import hashlib
import hmac
import http.client
import os
import queue
import ssl
import threading
import time
import urllib.parse


class Limits:
    """Run-wide caps on requests and bytes, checked before each request."""

    def __init__(self, requests, bytes_):
        self.max_requests, self.max_bytes = requests, bytes_
        self.requests = self.bytes = 0
        self.lock = threading.Lock()

    def reserve(self, length):
        with self.lock:
            if self.requests >= self.max_requests or self.bytes + length > self.max_bytes:
                raise RuntimeError('R2 run cap exceeded')
            self.requests += 1
            self.bytes += length


class Pool:
    def __init__(self, credentials, size, limits, timeout=60):
        self.access, self.secret = credentials['access'], credentials['secret']
        parts = urllib.parse.urlsplit(credentials['endpoint'])
        if parts.scheme != 'https':
            raise ValueError('R2 requires HTTPS')
        self.host, self.bucket = parts.netloc, credentials.get('bucket') or os.environ.get('R2_BUCKET')
        if not self.bucket:
            raise ValueError('R2_BUCKET names the corpus bucket')
        self.size, self.limits, self.timeout = size, limits, timeout
        self.idle = queue.LifoQueue()
        self.opened = 0
        self.connect_seconds = []
        self.lock = threading.Lock()
        self.context = ssl.create_default_context()
        self.requests = self.bytes = 0
        # Per request: time to the response headers after the request was
        # sent on an open connection (a warm round trip).
        self.latencies = []

    def _connection(self):
        try:
            return self.idle.get_nowait()
        except queue.Empty:
            pass
        with self.lock:
            if self.opened >= self.size:
                wait = True
            else:
                self.opened += 1
                wait = False
        if wait:
            return self.idle.get(timeout=self.timeout)
        connection = http.client.HTTPSConnection(self.host, timeout=self.timeout, context=self.context)
        start = time.perf_counter()
        connection.connect()
        with self.lock:
            self.connect_seconds.append(time.perf_counter() - start)
        return connection

    def _headers(self, key, extra):
        now = datetime.now(timezone.utc)
        stamp = now.strftime('%Y%m%dT%H%M%SZ')
        path = '/' + urllib.parse.quote(self.bucket + '/' + key, safe='/~')
        fields = {k.lower(): str(v) for k, v in extra.items()}
        fields.update({'host': self.host, 'x-amz-date': stamp, 'x-amz-content-sha256': 'UNSIGNED-PAYLOAD'})
        names = ';'.join(sorted(fields))
        canonical = '\n'.join(['GET', path, '', ''.join(f'{k}:{fields[k]}\n' for k in sorted(fields)),
                               names, 'UNSIGNED-PAYLOAD'])
        scope = f'{stamp[:8]}/auto/s3/aws4_request'
        to_sign = '\n'.join(['AWS4-HMAC-SHA256', stamp, scope, hashlib.sha256(canonical.encode()).hexdigest()])
        signing = ('AWS4' + self.secret).encode()
        for value in [stamp[:8], 'auto', 's3', 'aws4_request']:
            signing = hmac.new(signing, value.encode(), hashlib.sha256).digest()
        signature = hmac.new(signing, to_sign.encode(), hashlib.sha256).hexdigest()
        fields['authorization'] = (f'AWS4-HMAC-SHA256 Credential={self.access}/{scope}, '
                                   f'SignedHeaders={names}, Signature={signature}')
        return path, fields

    def get(self, key, size, etag, offset=None, length=None, sink=None):
        """GET the object or one range. Returns the bytes, or writes them to
        `sink(position, chunk)` and returns the count. Checks length, range
        and ETag; never retries."""
        whole = offset is None
        expected = size if whole else length
        self.limits.reserve(expected)
        extra = {'if-match': f'"{etag}"'}
        if not whole:
            extra['range'] = f'bytes={offset}-{offset + length - 1}'
        path, headers = self._headers(key, extra)
        connection = self._connection()
        try:
            sent = time.perf_counter()
            connection.request('GET', path, headers=headers)
            response = connection.getresponse()
            with self.lock:
                self.latencies.append(time.perf_counter() - sent)
            status = response.status
            if whole:
                ok = status == 200 and int(response.getheader('Content-Length', -1)) == size
            else:
                ok = status == 206 and response.getheader('Content-Range') == \
                    f'bytes {offset}-{offset + length - 1}/{size}'
            if not ok or response.getheader('ETag') != f'"{etag}"':
                response.read()
                raise RuntimeError(f'R2 GET returned an unexpected response (HTTP {status})')
            if sink is None:
                data = response.read()
                received = len(data)
            else:
                received, position = 0, 0 if whole else offset
                while chunk := response.read(1 << 20):
                    sink(position + received, chunk)
                    received += len(chunk)
                data = received
            if received != expected:
                raise RuntimeError('R2 GET returned a short body')
        except BaseException:
            connection.close()
            with self.lock:
                self.opened -= 1
            raise
        self.idle.put(connection)
        with self.lock:
            self.requests += 1
            self.bytes += expected
        return data

    def stream(self, key, size, etag):
        """Open a whole-object GET on its own connection and return
        `(connection, response, connect_seconds, latency_seconds)`; the
        caller reads the body and closes the connection. Checks status,
        length and ETag before returning; never retries."""
        self.limits.reserve(size)
        path, headers = self._headers(key, {'if-match': f'"{etag}"'})
        connection = http.client.HTTPSConnection(self.host, timeout=self.timeout, context=self.context)
        start = time.perf_counter()
        try:
            connection.connect()
            connected = time.perf_counter()
            connection.request('GET', path, headers=headers)
            response = connection.getresponse()
            latency = time.perf_counter() - connected
            if (response.status != 200 or int(response.getheader('Content-Length', -1)) != size
                    or response.getheader('ETag') != f'"{etag}"'):
                raise RuntimeError(f'R2 GET returned an unexpected response (HTTP {response.status})')
        except BaseException:
            connection.close()
            raise
        with self.lock:
            self.requests += 1
            self.latencies.append(latency)
        return connection, response, connected - start, latency

    def head(self, key):
        """Size and ETag from a one-byte range GET (untimed setup)."""
        self.limits.reserve(1)
        path, headers = self._headers(key, {'range': 'bytes=0-0'})
        connection = self._connection()
        try:
            connection.request('GET', path, headers=headers)
            response = connection.getresponse()
            response.read()
            total = int(response.getheader('Content-Range', '/0').rsplit('/', 1)[1])
            etag = response.getheader('ETag', '').strip('"')
            if response.status != 206 or not etag:
                raise RuntimeError('R2 size probe failed')
        finally:
            connection.close()
            with self.lock:
                self.opened -= 1
        return total, etag

    def close(self):
        while True:
            try:
                self.idle.get_nowait().close()
            except queue.Empty:
                return

import { createServer } from "node:http";

export async function serve(bytes, mode = {}) {
  const log = [];
  const server = createServer((request, response) => {
    const etag = mode.etag?.() ?? '"v1"';
    const range = /^bytes=(\d+)-(\d+)$/.exec(request.headers.range ?? "");
    log.push({
      range: request.headers.range,
      ifMatch: request.headers["if-match"]
    });
    if (request.headers["if-match"] && request.headers["if-match"] !== etag) {
      response.writeHead(412).end();
      return;
    }
    if (mode.redirect) {
      response.writeHead(302, {
        location: mode.redirect
      }).end();
      return;
    }
    if (mode.chunked) {
      response.writeHead(200, {
        etag: etag
      });
      response.write(bytes);
      response.end();
      return;
    }
    if (mode.ignoreRanges || range === null) {
      response.writeHead(200, {
        "content-length": bytes.length,
        etag: etag
      });
      response.end(bytes);
      return;
    }
    const start = Number(range[1]);
    const end = Math.min(Number(range[2]), bytes.length - 1);
    if (start >= bytes.length) {
      response.writeHead(416, {
        "content-range": `bytes */${bytes.length}`
      }).end();
      return;
    }
    response.writeHead(206, {
      "content-range": `bytes ${start}-${end}/${bytes.length}`,
      "content-length": end - start + 1,
      etag: etag
    });
    response.end(bytes.subarray(start, end + 1));
  });
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  const url = `http://127.0.0.1:${server.address().port}/object`;
  return {
    url: url,
    log: log,
    close: () => new Promise(resolve => server.close(resolve))
  };
}

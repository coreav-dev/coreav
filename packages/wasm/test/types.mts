import {
  bytesSource, chapters, check, clip, CoreAVError, errorResponse, extractAudio,
  extractSubtitles, ffprobe, ffprobeText, hlsGet, hlsIndex, httpSource, init,
  moduleInfo, probe, probeText, rangeSource, remux, RequestLimitError, sanitize,
  SourceChangedError, SourceNotFoundError
} from "@coreav/wasm";
import { probe as nodeProbe, ffprobe as nodeFfprobe } from "@coreav/wasm/node";
import type { FfprobeDocument, Session, Source } from "@coreav/wasm";

const bytes = new Uint8Array(8);
const source = bytesSource(bytes);
init(bytes);
const build: string = moduleInfo().build;
const document: FfprobeDocument = await ffprobe(source, { filename: "clip.mp4", limits: { maxTracks: 2 } });
const text: string = await ffprobeText(source);
await probe(source, { allow: ["h264"], strict: true, scanFragments: false, scanFrameSizes: true });
await probeText(source);
await nodeProbe(source);
const nodeDocument: FfprobeDocument = await nodeFfprobe(source, { filename: "clip.mp4" });
await chapters(source);
await check(source, { policy: "{}" });
await clip(source, { start: "1.5", audio: "none" });
await extractAudio(source, { stream: 1 });
await extractSubtitles(source);
await remux(source, { streams: [0, 1] });
await sanitize(source, { keep: ["language"] });
const indexed = await hlsIndex(source, { segmentDuration: "6" });
await hlsGet(indexed.index, "master.m3u8");
httpSource(new URL("https://media.invalid/file"), { headers: { accept: "*/*" }, maxRequests: 5 });
rangeSource(async () => ({ body: new ReadableStream<Uint8Array>(), offset: 0, size: 0 }));
const remote: Session = {
  size: 0, inMemory: false, read: async () => bytes,
  readNow: () => 0, plan: () => {}, planRegions: () => {}, close: () => {}
};
const foreign: Source = { open: () => remote };
await probe(foreign);
errorResponse(new CoreAVError('{"schema":"coreav.error/1","kind":"io","message":"read failed"}'));
errorResponse(new SourceNotFoundError());
new SourceChangedError();
new RequestLimitError(5);
void [build, document, nodeDocument, text];

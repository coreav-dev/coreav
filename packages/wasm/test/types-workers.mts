import {
  errorResponse, ffprobeText, r2Source, R2_DEFAULTS, type R2BucketLike
} from "@coreav/wasm/workers";

interface Env {
  MEDIA: R2Bucket;
}

const handler: ExportedHandler<Env> = {
  async fetch(request, env) {
    const bucket: R2BucketLike = env.MEDIA;
    try {
      const key = new URL(request.url).pathname;
      const source = r2Source(bucket, key, { ...R2_DEFAULTS, memoryBytes: 40 << 20 });
      return new Response(await ffprobeText(source));
    } catch (error) {
      return errorResponse(error);
    }
  },
};

export default handler;

/* libavformat's side of the packet-listing speed bar of docs/design/mpegts.md
 * ("Speed bars, round 2"), in process.
 *
 * A black box: written from FFmpeg's public API documentation
 * (avformat_open_input, av_find_input_format, avformat_find_stream_info,
 * av_read_frame, avformat_close_input, avio_alloc_context), compiled against
 * the installed headers of the pinned LGPL shared build (build-lav.sh). No
 * FFmpeg source was read or copied.
 *
 * usage: lav_ts packets SRC    open the file as mpegts, find stream info,
 *                              read every packet
 *        lav_ts --identity     the library version and license
 * Reads through a 64 KiB custom I/O buffer with pread. Times the operation
 * in process, opening the file included, and prints one JSON line
 * {"ok","ns","count","bytes","error"}; the exit status is 0 also when the
 * operation fails (ok false), and 2 on a usage error or a library other
 * than the pinned build (another version, a GPL or nonfree configuration,
 * or headers that do not match the library).
 */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <inttypes.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

#include <libavformat/avformat.h>
#include <libavutil/avutil.h>

#define IO_BUFFER (64 * 1024)
#define PINNED "n9.0.2-17-g2a571b6068"

static uint64_t now_ns(void) {
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return (uint64_t)t.tv_sec * 1000000000u + (uint64_t)t.tv_nsec;
}

typedef struct {
    int fd;
    int64_t pos, size;
} Io;

static int read_cb(void *opaque, uint8_t *buf, int n) {
    Io *io = opaque;
    ssize_t r = pread(io->fd, buf, (size_t)n, io->pos);
    if (r < 0) return AVERROR(errno);
    if (r == 0) return AVERROR_EOF;
    io->pos += r;
    return (int)r;
}

static int64_t seek_cb(void *opaque, int64_t offset, int whence) {
    Io *io = opaque;
    if (whence & AVSEEK_SIZE) return io->size;
    whence &= ~AVSEEK_FORCE;
    int64_t to;
    if (whence == SEEK_SET) to = offset;
    else if (whence == SEEK_CUR) to = io->pos + offset;
    else if (whence == SEEK_END) to = io->size + offset;
    else return AVERROR(EINVAL);
    if (to < 0) return AVERROR(EINVAL);
    io->pos = to;
    return to;
}

static int open_input(const char *src, Io *in, AVFormatContext **ic) {
    in->fd = open(src, O_RDONLY);
    if (in->fd < 0) return AVERROR(errno);
    in->size = lseek(in->fd, 0, SEEK_END);
    uint8_t *buffer = av_malloc(IO_BUFFER);
    AVIOContext *pb = buffer ? avio_alloc_context(buffer, IO_BUFFER, 0, in, read_cb, NULL, seek_cb) : NULL;
    *ic = avformat_alloc_context();
    if (!pb || !*ic) {
        if (pb) { av_freep(&pb->buffer); avio_context_free(&pb); } else av_free(buffer);
        return AVERROR(ENOMEM);
    }
    pb->seekable = AVIO_SEEKABLE_NORMAL;
    (*ic)->pb = pb;
    (*ic)->flags |= AVFMT_FLAG_CUSTOM_IO;
    return avformat_open_input(ic, src, av_find_input_format("mpegts"), NULL);
}

static void close_input(Io *in, AVFormatContext **ic) {
    AVIOContext *pb = *ic ? (*ic)->pb : NULL;
    avformat_close_input(ic);
    if (pb) { av_freep(&pb->buffer); avio_context_free(&pb); }
    if (in->fd >= 0) close(in->fd);
}

int main(int argc, char **argv) {
    const char *config = avformat_configuration();
    if (strncmp(av_version_info(), PINNED, strlen(PINNED)) || avformat_version() != LIBAVFORMAT_VERSION_INT ||
        strncmp(avformat_license(), "LGPL", 4) || strstr(config, "--enable-gpl") || strstr(config, "--enable-nonfree")) {
        fprintf(stderr, "lav_ts: not the pinned LGPL build %s (found %s, %s)\n", PINNED, av_version_info(),
                avformat_license());
        return 2;
    }
    if (argc == 2 && !strcmp(argv[1], "--identity")) {
        printf("{\"version\":\"%s\",\"license\":\"%s\"}\n", av_version_info(), avformat_license());
        return 0;
    }
    if (argc != 3 || strcmp(argv[1], "packets")) {
        fprintf(stderr, "usage: lav_ts packets SRC | lav_ts --identity\n");
        return 2;
    }
    av_log_set_level(AV_LOG_QUIET);
    Io in = {.fd = -1};
    AVFormatContext *ic = NULL;
    uint64_t count = 0, bytes = 0;
    uint64_t start = now_ns();
    int status = open_input(argv[2], &in, &ic);
    if (status >= 0) status = avformat_find_stream_info(ic, NULL);
    if (status >= 0) {
        AVPacket *pkt = av_packet_alloc();
        if (!pkt) {
            status = AVERROR(ENOMEM);
        } else {
            while ((status = av_read_frame(ic, pkt)) >= 0) {
                count++;
                bytes += (uint64_t)pkt->size;
                av_packet_unref(pkt);
            }
            if (status == AVERROR_EOF) status = 0;
            av_packet_free(&pkt);
        }
    }
    close_input(&in, &ic);
    uint64_t ns = now_ns() - start;
    char error[128] = "";
    if (status < 0) av_strerror(status, error, sizeof error);
    printf("{\"ok\":%s,\"ns\":%" PRIu64 ",\"count\":%" PRIu64 ",\"bytes\":%" PRIu64 ",\"error\":\"%s\"}\n",
           status >= 0 ? "true" : "false", ns, count, bytes, error);
    return 0;
}

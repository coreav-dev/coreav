/* SPDX-License-Identifier: MIT OR Apache-2.0 */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <signal.h>
#include <spawn.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>
#include <x264.h>

enum { MAX_PATH = 128, MAX_DATA = 1048576, MAX_DIM = 16384, MAX_DIAGNOSTICS = 8192 };

extern char **environ;

static int fail(const char *message)
{
    fprintf(stderr, "coreav-x264-shm: %s\n", message);
    return -1;
}

static int number(const char **text, uint32_t limit, uint32_t *value)
{
    const char *p = *text;
    uint32_t n = 0;
    if (*p < '0' || *p > '9')
        return -1;
    do {
        unsigned digit = (unsigned)(*p++ - '0');
        if (n > limit / 10 || (n == limit / 10 && digit > limit % 10))
            return -1;
        n = n * 10 + digit;
    } while (*p >= '0' && *p <= '9');
    *text = p;
    *value = n;
    return 0;
}

static int pair(const char *text, char separator, uint32_t limit,
                uint32_t *a, uint32_t *b)
{
    return number(&text, limit, a) || *text++ != separator ||
           number(&text, limit, b) || *text || !*a || !*b ? -1 : 0;
}

static int depth(const char *text)
{
    return !strcmp(text, "8") ? 8 : !strcmp(text, "10") ? 10 : 0;
}

static int csp(const char *text)
{
    if (!strcmp(text, "i420"))
        return X264_CSP_I420;
    if (!strcmp(text, "i422"))
        return X264_CSP_I422;
    return !strcmp(text, "i444") ? X264_CSP_I444 : 0;
}

static int clock_probe(const char *program, const char *timecodes,
                       uint32_t *numerator, uint32_t *denominator)
{
    int pipefd[2];
    if (pipe2(pipefd, O_CLOEXEC))
        return fail("cannot create clock diagnostic pipe");
    if (pipefd[0] < 3 || pipefd[1] < 3) {
        close(pipefd[0]);
        close(pipefd[1]);
        return fail("clock probe requires standard descriptors");
    }
    posix_spawn_file_actions_t actions;
    if (posix_spawn_file_actions_init(&actions)) {
        close(pipefd[0]);
        close(pipefd[1]);
        return fail("cannot prepare clock program");
    }
    int error = posix_spawn_file_actions_addopen(&actions, STDIN_FILENO, "/dev/null", O_RDONLY, 0);
    if (!error)
        error = posix_spawn_file_actions_addopen(&actions, STDOUT_FILENO, "/dev/null", O_WRONLY, 0);
    if (!error)
        error = posix_spawn_file_actions_adddup2(&actions, pipefd[1], STDERR_FILENO);
    if (!error)
        error = posix_spawn_file_actions_addclosefrom_np(&actions, 3);
    char *const args[] = {
        (char *)program, "--demuxer", "raw", "--input-res", "16x16",
        "--tcfile-in", (char *)timecodes, "--log-level", "info", "--no-progress",
        "--muxer", "raw", "--output", "-", "-", NULL
    };
    pid_t child = -1;
    if (!error)
        error = posix_spawn(&child, program, &actions, NULL, args, environ);
    posix_spawn_file_actions_destroy(&actions);
    close(pipefd[1]);
    if (error) {
        close(pipefd[0]);
        return fail("cannot execute clock program");
    }
    char diagnostic[MAX_DIAGNOSTICS + 2];
    size_t used = 0;
    while (used <= MAX_DIAGNOSTICS) {
        ssize_t count = read(pipefd[0], diagnostic + used, MAX_DIAGNOSTICS + 1 - used);
        if (count < 0 && errno == EINTR)
            continue;
        if (count < 0) {
            error = 1;
            break;
        }
        if (!count)
            break;
        used += (size_t)count;
    }
    close(pipefd[0]);
    if (error || used > MAX_DIAGNOSTICS)
        kill(child, SIGKILL);
    int status = 0;
    pid_t waited;
    do {
        waited = waitpid(child, &status, 0);
    } while (waited < 0 && errno == EINTR);
    if (error || used > MAX_DIAGNOSTICS || waited != child ||
        !WIFEXITED(status) || WEXITSTATUS(status))
        return fail("clock probe failed or exceeded diagnostic limit");
    if (!used || diagnostic[used - 1] != '\n' || memchr(diagnostic, '\0', used))
        return fail("clock probe diagnostic is absent or truncated");
    diagnostic[used] = '\0';
    const char prefix[] = "timecode [info]: automatic timebase generation ";
    unsigned found = 0;
    char *line = diagnostic;
    while (*line) {
        char *end = strchr(line, '\n');
        *end = '\0';
        if (!strncmp(line, prefix, sizeof(prefix) - 1) &&
            (found++ || pair(line + sizeof(prefix) - 1, '/', INT_MAX, numerator, denominator)))
            return fail("clock probe diagnostic is invalid or duplicated");
        line = end + 1;
    }
    return found == 1 ? 0 : fail("clock probe diagnostic is absent");
}

struct config {
    x264_param_t param;
    uint32_t input_timebase_num;
    uint32_t input_timebase_den;
    size_t size;
    size_t plane[3];
    int stride[3];
};

static int configure(int argc, char **argv, struct config *config)
{
    static const char *names[] = {
        "demuxer", "input-csp", "output-csp", "input-depth", "output-depth",
        "input-res", "fps", "tcfile-in", "colorprim", "transfer", "colormatrix",
        "input-range", "range", "sar", "preset", "crf", "bitrate", "threads",
        "min-keyint", "log-level", "muxer", "output", "coreav-timebase", "coreav-vfr",
        "coreav-clock-program"
    };
    enum { DEMUX, IN_CSP, OUT_CSP, IN_DEPTH, OUT_DEPTH, RES, FPS, TCFILE,
           COLOR, TRANSFER, MATRIX, IN_RANGE, RANGE, SAR, PRESET, CRF, BITRATE,
           THREADS, KEYINT, LOG, MUX, OUTPUT, TIMEBASE, VFR, CLOCK, OPTIONS };
    const char *values[OPTIONS] = {0};
    int shm = 0, progress = 0, input = 0;
    uint32_t width, height, fps_n = 25, fps_d = 1, tb_n, tb_d;
    if (argc > 96)
        return fail("too many arguments");
    for (int i = 1; i < argc; i++) {
        if (strnlen(argv[i], 4097) > 4096)
            return fail("argument too long");
        if (!strcmp(argv[i], "--coreav-shm")) {
            if (shm++)
                return fail("duplicate shared-memory option");
            continue;
        }
        if (!strcmp(argv[i], "--no-progress")) {
            if (progress++)
                return fail("duplicate progress option");
            continue;
        }
        if (!strcmp(argv[i], "-") && !input) {
            input = 1;
            continue;
        }
        int option = -1;
        for (int j = 0; j < OPTIONS; j++)
            if (!strncmp(argv[i], "--", 2) && !strcmp(argv[i] + 2, names[j]))
                option = j;
        if (option < 0 || values[option] || ++i == argc ||
            strnlen(argv[i], 4097) > 4096)
            return fail("unknown, duplicate or incomplete option");
        values[option] = argv[i];
    }
    const int required[] = {DEMUX, IN_CSP, OUT_CSP, IN_DEPTH, OUT_DEPTH,
                            RES, PRESET, LOG, MUX, OUTPUT, TIMEBASE, VFR};
    for (size_t i = 0; i < sizeof(required) / sizeof(required[0]); i++)
        if (!values[required[i]])
            return fail("missing required option");
    if (!shm || !progress || !input || strcmp(values[DEMUX], "raw") ||
        strcmp(values[MUX], "raw") || strcmp(values[OUTPUT], "-") ||
        strcmp(values[LOG], "error") ||
        (!values[FPS] && !values[TCFILE]) ||
        (!!values[CRF] == !!values[BITRATE]))
        return fail("unsupported input, output or rate configuration");
    int format = csp(values[IN_CSP]);
    int bits = depth(values[IN_DEPTH]);
    if (!format || !bits || format != csp(values[OUT_CSP]) ||
        bits != depth(values[OUT_DEPTH]))
        return fail("only matching planar i420/i422/i444 at 8 or 10 bits are supported");
    if (pair(values[RES], 'x', MAX_DIM, &width, &height) ||
        (format != X264_CSP_I444 && width % 2) ||
        (format == X264_CSP_I420 && height % 2))
        return fail("unsupported dimensions (maximum 16384; subsampled dimensions must be even)");
    if (pair(values[TIMEBASE], '/', INT_MAX, &tb_n, &tb_d) ||
        (values[FPS] && pair(values[FPS], '/', INT_MAX, &fps_n, &fps_d)) ||
        (strcmp(values[VFR], "0") && strcmp(values[VFR], "1")))
        return fail("invalid frame clock");
    if (!!values[TCFILE] != !!values[CLOCK] ||
        (values[CLOCK] && values[CLOCK][0] != '/') ||
        (values[TCFILE] && (fps_n != 25 || fps_d != 1 || strcmp(values[VFR], "1"))))
        return fail("timecodes require an absolute clock program, VFR and default 25/1 fps");
    if (!!values[IN_RANGE] != !!values[RANGE] ||
        (values[RANGE] && (strcmp(values[RANGE], values[IN_RANGE]) ||
         (strcmp(values[RANGE], "pc") && strcmp(values[RANGE], "tv")))))
        return fail("range conversion is not supported");
    x264_param_t *p = &config->param;
    if (x264_param_default_preset(p, values[PRESET], NULL) < 0)
        return fail("unsupported preset");
    p->i_width = (int)width;
    p->i_height = (int)height;
    p->i_csp = format;
    p->i_bitdepth = bits;
    p->i_fps_num = fps_n;
    p->i_fps_den = fps_d;
    config->input_timebase_num = tb_n;
    config->input_timebase_den = tb_d;
    if (values[TCFILE] && clock_probe(values[CLOCK], values[TCFILE], &tb_n, &tb_d)) {
        x264_param_cleanup(p);
        return -1;
    }
    p->i_timebase_num = tb_n;
    p->i_timebase_den = tb_d;
    p->b_vfr_input = !strcmp(values[VFR], "1");
    p->b_annexb = 1;
    p->b_repeat_headers = 1;
    p->i_log_level = X264_LOG_NONE;
    const int parsed[] = {COLOR, TRANSFER, MATRIX, SAR, CRF, BITRATE, THREADS, KEYINT};
    for (size_t i = 0; i < sizeof(parsed) / sizeof(parsed[0]); i++) {
        int index = parsed[i];
        if (values[index] && x264_param_parse(p, names[index], values[index]) < 0)
            return fail("unsupported encoder parameter");
    }
    if (values[RANGE])
        p->vui.b_fullrange = !strcmp(values[RANGE], "pc");
    size_t bytes = bits == 8 ? 1 : 2;
    size_t chroma_width = format == X264_CSP_I444 ? width : width / 2;
    size_t chroma_height = format == X264_CSP_I420 ? height / 2 : height;
    config->stride[0] = (int)(width * bytes);
    config->stride[1] = config->stride[2] = (int)(chroma_width * bytes);
    config->plane[0] = width * bytes * height;
    config->plane[1] = config->plane[2] = chroma_width * bytes * chroma_height;
    config->size = config->plane[0] + config->plane[1] + config->plane[2];
    return 0;
}

static int read_exact(void *buffer, size_t length, int allow_eof)
{
    unsigned char *p = buffer;
    size_t done = 0;
    while (done < length) {
        ssize_t n = read(STDIN_FILENO, p + done, length - done);
        if (n < 0 && errno == EINTR)
            continue;
        if (n == 0 && !done && allow_eof)
            return 0;
        if (n <= 0)
            return fail("truncated or unreadable input protocol");
        done += (size_t)n;
    }
    return 1;
}

static int write_exact(const void *buffer, size_t length)
{
    const unsigned char *p = buffer;
    while (length) {
        ssize_t n = write(STDOUT_FILENO, p, length);
        if (n < 0 && errno == EINTR)
            continue;
        if (n <= 0)
            return fail("cannot write output protocol");
        p += n;
        length -= (size_t)n;
    }
    return 0;
}

static uint64_t little(const unsigned char *bytes, size_t count)
{
    uint64_t value = 0;
    for (size_t i = 0; i < count; i++)
        value |= (uint64_t)bytes[i] << (i * 8);
    return value;
}

static int message(unsigned char tag, uint64_t value, size_t count)
{
    unsigned char bytes[9] = {tag};
    for (size_t i = 0; i < count; i++)
        bytes[i + 1] = (unsigned char)(value >> (i * 8));
    return write_exact(bytes, count + 1);
}

static int encoded(x264_t *encoder, x264_picture_t *picture)
{
    x264_nal_t *nals = NULL;
    int count = 0;
    x264_picture_t output;
    int size = x264_encoder_encode(encoder, &nals, &count, picture, &output);
    if (size < 0)
        return fail("encoder rejected a frame");
    size_t total = 0;
    for (int i = 0; i < count; i++) {
        if (nals[i].i_payload < 0 || (size_t)nals[i].i_payload > (size_t)size - total)
            return fail("invalid encoder output size");
        size_t remaining = (size_t)nals[i].i_payload;
        const unsigned char *p = nals[i].p_payload;
        total += remaining;
        while (remaining) {
            size_t chunk = remaining > MAX_DATA ? MAX_DATA : remaining;
            if (message(2, chunk, 4) || write_exact(p, chunk))
                return -1;
            p += chunk;
            remaining -= chunk;
        }
    }
    return total == (size_t)size ? 0 : fail("incomplete encoder output");
}

static int snapshot_path(const char *path)
{
    if (strncmp(path, "/proc/", 6))
        return -1;
    path += 6;
    uint32_t pid, fd;
    if (number(&path, INT_MAX, &pid) || !pid || strncmp(path, "/fd/", 4))
        return -1;
    path += 4;
    return number(&path, INT_MAX, &fd) || *path ? -1 : 0;
}

static int run(x264_t *encoder, const struct config *config)
{
    unsigned char magic[8];
    if (read_exact(magic, sizeof(magic), 0) < 0 || memcmp(magic, "CAVSHM01", 8))
        return fail("invalid input protocol magic");
    if (write_exact("CAVOUT01", 8))
        return -1;
    uint64_t previous = 0;
    for (;;) {
        unsigned char tag, fields[26];
        int result = read_exact(&tag, 1, 1);
        if (result < 0)
            return -1;
        if (!result)
            break;
        if (tag != 1 || read_exact(fields, sizeof(fields), 0) < 0)
            return fail("invalid frame message");
        uint64_t sequence = little(fields, 8);
        uint64_t ticks = little(fields + 8, 8);
        if (ticks > INT64_MAX)
            return fail("frame timestamp must be nonnegative");
        unsigned __int128 scaled = (unsigned __int128)ticks * config->input_timebase_num *
            config->param.i_timebase_den;
        uint64_t divisor = (uint64_t)config->input_timebase_den * config->param.i_timebase_num;
        scaled = (scaled + divisor / 2) / divisor;
        if (scaled > INT64_MAX)
            return fail("rescaled frame timestamp exceeds signed 64-bit range");
        int64_t pts = (int64_t)scaled;
        uint64_t size = little(fields + 16, 8);
        size_t length = (size_t)little(fields + 24, 2);
        if (previous == UINT64_MAX || sequence != previous + 1 || size != config->size ||
            !length || length > MAX_PATH)
            return fail("invalid sequence, frame size or path length");
        char path[MAX_PATH + 1];
        if (read_exact(path, length, 0) < 0)
            return -1;
        path[length] = '\0';
        if (memchr(path, '\0', length) || snapshot_path(path))
            return fail("snapshot path must be /proc/<pid>/fd/<fd>");
        int fd = open(path, O_RDONLY | O_CLOEXEC | O_NONBLOCK);
        if (fd < 0)
            return fail("cannot open immutable snapshot");
        struct stat st;
        int seals = fcntl(fd, F_GET_SEALS);
        int required = F_SEAL_WRITE | F_SEAL_GROW | F_SEAL_SHRINK | F_SEAL_SEAL;
        if (fstat(fd, &st) || !S_ISREG(st.st_mode) || st.st_size < 0 ||
            (uint64_t)st.st_size != size || seals < 0 || (seals & required) != required) {
            close(fd);
            return fail("snapshot must have exact size and WRITE/GROW/SHRINK/SEAL seals");
        }
        void *mapping = mmap(NULL, config->size, PROT_READ, MAP_SHARED, fd, 0);
        if (mapping == MAP_FAILED) {
            close(fd);
            return fail("cannot map immutable snapshot");
        }
        x264_picture_t picture;
        x264_picture_init(&picture);
        picture.i_pts = pts;
        picture.img.i_csp = config->param.i_csp |
            (config->param.i_bitdepth > 8 ? X264_CSP_HIGH_DEPTH : 0);
        picture.img.i_plane = 3;
        size_t offset = 0;
        for (int i = 0; i < 3; i++) {
            picture.img.plane[i] = (unsigned char *)mapping + offset;
            picture.img.i_stride[i] = config->stride[i];
            offset += config->plane[i];
        }
        result = encoded(encoder, &picture);
        int unmapped = munmap(mapping, config->size);
        int closed = close(fd);
        if (result || unmapped || closed)
            return fail("encoding or snapshot cleanup failed");
        if (message(1, sequence, 8))
            return -1;
        previous = sequence;
    }
    while (x264_encoder_delayed_frames(encoder))
        if (encoded(encoder, NULL))
            return -1;
    return 0;
}

int main(int argc, char **argv)
{
    signal(SIGPIPE, SIG_IGN);
    struct config config = {0};
    if (configure(argc, argv, &config))
        return EXIT_FAILURE;
    x264_t *encoder = x264_encoder_open(&config.param);
    if (!encoder) {
        fail("libx264 cannot open the requested configuration or bit depth");
        x264_param_cleanup(&config.param);
        return EXIT_FAILURE;
    }
    int result = run(encoder, &config);
    x264_encoder_close(encoder);
    x264_param_cleanup(&config.param);
    return result ? EXIT_FAILURE : EXIT_SUCCESS;
}

#include <EbSvtAv1Enc.h>
#include <stddef.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

_Static_assert(SVT_AV1_VERSION_MAJOR == 4 && SVT_AV1_VERSION_MINOR == 2 &&
               SVT_AV1_VERSION_PATCHLEVEL == 0 && SVT_AV1_ENC_ABI_VERSION == 0,
               "use the verified SVT-AV1 4.2.0 headers");
_Static_assert(sizeof(size_t) == 8, "native SVT-AV1 requires an LP64 release target");

typedef struct {
    uint32_t width, height, rate_num, rate_den, preset, crf, keyint;
    uint32_t primaries, transfer, matrix, full_range, chroma_position;
} CoreavSvtConfig;

typedef struct {
    EbBufferHeaderType header;
    EbSvtIOFormat picture;
} CoreavSvtInput;

typedef struct {
    const uint8_t *data;
    uint32_t length, capacity;
    int64_t pts, dts;
    uint32_t flags, picture_type;
    void *lease;
} CoreavSvtPacket;

_Static_assert(sizeof(CoreavSvtConfig) == 48, "check the Rust configuration ABI");
_Static_assert(sizeof(CoreavSvtPacket) == 48, "check the Rust packet ABI");
_Static_assert(offsetof(CoreavSvtPacket, pts) == 16, "check the Rust packet ABI");
_Static_assert(offsetof(CoreavSvtPacket, lease) == 40, "check the Rust packet ABI");

const char *coreav_svt_version(void) {
    return svt_av1_get_version();
}

size_t coreav_svt_input_bytes(void) {
    return sizeof(CoreavSvtInput);
}

void *coreav_svt_open(const CoreavSvtConfig *settings, uint32_t *status) {
    EbComponentType *handle = NULL;
    EbSvtAv1EncConfiguration config;
    memset(&config, 0, sizeof(config));
    EbErrorType result = svt_av1_enc_init_handle(&handle, &config);
    *status = (uint32_t)result;
    if (result != EB_ErrorNone)
        return NULL;

    config.source_width = settings->width;
    config.source_height = settings->height;
    config.forced_max_frame_width = settings->width;
    config.forced_max_frame_height = settings->height;
    config.frame_rate_numerator = settings->rate_num;
    config.frame_rate_denominator = settings->rate_den;
    config.enc_mode = (int8_t)settings->preset;
    config.rate_control_mode = SVT_AV1_RC_MODE_CQP_OR_CRF;
    config.qp = settings->crf;
    config.aq_mode = 2;
    config.intra_period_length = (int32_t)settings->keyint;
    config.multiply_keyint = false;
    config.intra_refresh_type = SVT_AV1_KF_REFRESH;
    config.pred_structure = RANDOM_ACCESS;
    config.level_of_parallelism = 1;
    config.encoder_bit_depth = 8;
    config.encoder_color_format = EB_YUV420;
    config.profile = MAIN_PROFILE;
    config.color_primaries = (EbColorPrimaries)settings->primaries;
    config.transfer_characteristics = (EbTransferCharacteristics)settings->transfer;
    config.matrix_coefficients = (EbMatrixCoefficients)settings->matrix;
    config.color_range = (EbColorRange)settings->full_range;
    config.chroma_sample_position = (EbChromaSamplePosition)settings->chroma_position;
    config.film_grain_denoise_strength = 0;
    config.film_grain_denoise_apply = 0;
    config.superres_mode = SUPERRES_NONE;
    config.resize_mode = RESIZE_NONE;
    config.tune = 1;
    config.tile_rows = 0;
    config.tile_columns = 0;

    result = svt_av1_enc_set_parameter(handle, &config);
    if (result != EB_ErrorNone) {
        *status = (uint32_t)result;
        (void)svt_av1_enc_deinit_handle(handle);
        return NULL;
    }
    result = svt_av1_enc_init(handle);
    if (result != EB_ErrorNone) {
        *status = (uint32_t)result;
        if (svt_av1_enc_deinit(handle) == EB_ErrorNone)
            (void)svt_av1_enc_deinit_handle(handle);
        return NULL;
    }
    *status = 0;
    return handle;
}

void *coreav_svt_input_new(uint8_t *data, uint32_t width, uint32_t height,
                         int64_t identity, uint32_t eof) {
    CoreavSvtInput *input = calloc(1, sizeof(*input));
    if (!input)
        return NULL;
    input->header.size = sizeof(input->header);
    if (eof) {
        input->header.flags = EB_BUFFERFLAG_EOS;
    } else {
        size_t luma = (size_t)width * height;
        input->picture.luma = data;
        input->picture.cb = data + luma;
        input->picture.cr = data + luma + luma / 4;
        input->picture.y_stride = width;
        input->picture.cb_stride = width / 2;
        input->picture.cr_stride = width / 2;
        input->header.p_buffer = (uint8_t *)&input->picture;
        input->header.n_filled_len = (uint32_t)(luma + luma / 2);
        input->header.n_alloc_len = input->header.n_filled_len;
        input->header.pts = identity;
        input->header.pic_type = EB_AV1_INVALID_PICTURE;
    }
    return input;
}

void coreav_svt_input_free(void *input) {
    free(input);
}

uint32_t coreav_svt_send(void *handle, void *input) {
    CoreavSvtInput *picture = input;
    return (uint32_t)svt_av1_enc_send_picture(handle, &picture->header);
}

uint32_t coreav_svt_receive(void *handle, uint8_t done, CoreavSvtPacket *packet) {
    EbBufferHeaderType *buffer = NULL;
    memset(packet, 0, sizeof(*packet));
    EbErrorType result = svt_av1_enc_get_packet(handle, &buffer, done);
    if (result != EB_ErrorNone)
        return (uint32_t)result;
    if (!buffer)
        return UINT32_MAX;
    packet->data = buffer->p_buffer;
    packet->length = buffer->n_filled_len;
    packet->capacity = buffer->n_alloc_len;
    packet->pts = buffer->pts;
    packet->dts = buffer->dts;
    packet->flags = buffer->flags;
    packet->picture_type = (uint32_t)buffer->pic_type;
    packet->lease = buffer;
    return 0;
}

void coreav_svt_packet_release(void *lease) {
    EbBufferHeaderType *buffer = lease;
    svt_av1_enc_release_out_buffer(&buffer);
}

uint32_t coreav_svt_close(void *handle) {
    EbErrorType result = svt_av1_enc_deinit(handle);
    if (result != EB_ErrorNone)
        return (uint32_t)result;
    return (uint32_t)svt_av1_enc_deinit_handle(handle);
}

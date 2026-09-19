/*
  analysis_probe.c — generates the committed Phase 9 analysis reference table
  used by tests/analysis.rs.

  The MusicPack reference computes waveform envelopes with
  core/libmusicpack/src/waveform.c and loudness with
  core/libmusicpack/src/loudness.c over vendored libebur128 1.2.6.  This probe
  compiles those exact translation units (plus the reference SHA-256) and
  prints compact deterministic records:

      waveform <label> points=<n> sha256=<hex> head=<16 hex> tail=<8 hex>
      loudness <label> lufs=<%.17g> tp=<%.17g>

  Inputs:
    flac:<path>                        decode a FLAC file with the reference's
                                       vendored dr_flac (identical PCM to the
                                       committed Phase 8 fixtures)
    synth:silence:<rate>:<ch>:<frames>
    synth:dc:<rate>:<ch>:<frames>       constant +0.5
    synth:impulse:<rate>:<ch>:<frames>  first frame +1.0, then silence
    synth:prng:<rate>:<ch>:<frames>     deterministic 32-bit LCG mapped to
                                       [-1, 1); bit-exact reproducible in Rust
                                       (integer ops + power-of-two scaling)

  Build & regenerate (from the musicpack-core root; the probe is Unix-only —
  it needs <sys/queue.h> for vendored ebur128, which the committed oracle in
  tests/data/analysis_c_reference.txt makes irrelevant to CI):

      REF=../musicpack
      cc -O2 -std=c11 \
        -I "$REF/core/libmusicpack/include" \
        -I "$REF/core/libmusicpack/src" \
        -I "$REF/core/libmusicpack/vendor" \
        -I "$REF/core/libmusicpack/vendor/cjson" \
        -I "$REF/core/libmusicpack/vendor/ebur128" \
        -I "$REF/codec/include" \
        -o /tmp/analysis_probe tools/analysis_probe.c \
        "$REF/core/libmusicpack/src/waveform.c" \
        "$REF/core/libmusicpack/src/loudness.c" \
        "$REF/core/libmusicpack/src/checksum.c" \
        "$REF/core/libmusicpack/vendor/ebur128/ebur128.c" \
        -lm

      : > tests/data/analysis_c_reference.txt
      AUDIO=fixtures/reference/audio
      for n in flac16-44k.flac flac24-48k.flac flac24-96k.flac \
               flac-mono-44k.flac; do
        /tmp/analysis_probe waveform "$n" "flac:$AUDIO/$n"
        /tmp/analysis_probe loudness "$n" "flac:$AUDIO/$n"
      done >> tests/data/analysis_c_reference.txt
      for s in synth:silence:44100:2:88200 synth:dc:44100:2:88200 \
               synth:impulse:44100:1:44100 synth:prng:44100:2:88200 \
               synth:prng:48000:1:144000 synth:prng:96000:2:192000 \
               synth:prng:96000:2:960000 synth:prng:192000:2:960000; do
        /tmp/analysis_probe waveform "$s" "$s"
        /tmp/analysis_probe loudness "$s" "$s"
      done >> tests/data/analysis_c_reference.txt

  The waveform records are exact-byte golden values; the loudness records are
  compared within +/-0.05.
*/

#include <math.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include <musicpack/checksum.h>
#include <musicpack/loudness.h>
#include <musicpack/waveform.h>

/* dr_flac, compiled TU-locally (same pattern as the reference audio.c). */
#if defined(__GNUC__) || defined(__clang__)
# pragma GCC diagnostic push
# pragma GCC diagnostic ignored "-Wunused-function"
#endif
#define DRFLAC_API static
#define DR_FLAC_IMPLEMENTATION
#include "dr_flac.h"
#if defined(__GNUC__) || defined(__clang__)
# pragma GCC diagnostic pop
#endif

#define CHUNK_FRAMES 1152u
#define MAX_CHANNELS 8u

static uint32_t g_rng;

static void
synth_reset(void)
{
    g_rng = 0x12345678u;
}

/* Deterministic LCG sample in [-1, 1); integer ops + 2^-23 scaling only, so
   C and Rust produce bit-identical f32 values. */
static float
synth_next(void)
{
    g_rng = g_rng * 1664525u + 1013904223u;
    return (float) (g_rng >> 8) / 8388608.0f - 1.0f;
}

/* A decoded/generated PCM source streamed in CHUNK_FRAMES blocks. */
typedef struct {
    unsigned rate;
    unsigned channels;
    uint64_t total_frames; /* synth only */
    uint64_t frame;        /* synth cursor */
    const char *kind;      /* synth kind */
    drflac *flac;
} source;

static int
source_open_flac(source *s, const char *path)
{
    drflac *d = drflac_open_file(path, 0);
    if (d == 0) {
        fprintf(stderr, "cannot open FLAC '%s'\n", path);
        return -1;
    }
    s->flac = d;
    s->rate = d->sampleRate;
    s->channels = d->channels;
    s->total_frames = d->totalPCMFrameCount;
    return 0;
}

/* Reads up to `frames` frames into `out`; returns the count (0 at EOF). */
static size_t
source_read(source *s, float *out, size_t frames)
{
    if (s->flac != 0) {
        drflac_uint64 got = drflac_read_pcm_frames_f32(s->flac, frames,
                                                       out);
        return (size_t) got;
    }
    if (s->frame >= s->total_frames)
        return 0;
    {
        uint64_t remaining = s->total_frames - s->frame;
        size_t n = remaining < (uint64_t) frames ? (size_t) remaining
                                                 : frames;
        size_t i;
        for (i = 0; i < n; i++) {
            unsigned c;
            for (c = 0; c < s->channels; c++) {
                float v;
                if (strcmp(s->kind, "silence") == 0)
                    v = 0.0f;
                else if (strcmp(s->kind, "dc") == 0)
                    v = 0.5f;
                else if (strcmp(s->kind, "impulse") == 0)
                    v = (s->frame == 0) ? 1.0f : 0.0f;
                else
                    v = synth_next();
                out[i * s->channels + c] = v;
            }
            s->frame++;
        }
        return n;
    }
}

static void
source_close(source *s)
{
    if (s->flac != 0)
        drflac_close(s->flac);
}

/* Configure `s` from a "<kind>:<rate>:<ch>:<frames>" spec. */
static int
source_open_synth(source *s, const char *spec)
{
    char kind[32];
    unsigned rate, channels;
    unsigned long long frames;

    if (sscanf(spec, "synth:%31[^:]:%u:%u:%llu", kind, &rate, &channels,
               &frames) != 4)
        return -1;
    if (channels < 1 || channels > MAX_CHANNELS || rate == 0)
        return -1;
    if (strcmp(kind, "prng") == 0)
        synth_reset();
    s->kind = strdup(kind);
    s->rate = rate;
    s->channels = channels;
    s->total_frames = frames;
    return 0;
}

static int
source_open(source *s, const char *spec)
{
    memset(s, 0, sizeof *s);
    if (strncmp(spec, "flac:", 5) == 0)
        return source_open_flac(s, spec + 5);
    if (strncmp(spec, "synth:", 6) == 0)
        return source_open_synth(s, spec);
    fprintf(stderr, "unknown source spec '%s'\n", spec);
    return -1;
}

static void
put_hex(const unsigned char *bytes, size_t n, char *out)
{
    static const char hexc[] = "0123456789abcdef";
    size_t i;
    for (i = 0; i < n; i++) {
        out[i * 2] = hexc[bytes[i] >> 4];
        out[i * 2 + 1] = hexc[bytes[i] & 0x0f];
    }
    out[n * 2] = '\0';
}

static int
emit_waveform(const char *label, const char *spec)
{
    source s;
    musicpack_waveform_acc *acc;
    musicpack_waveform_bucket *buckets = 0;
    size_t bucket_count = 0, payload_len = 0;
    unsigned char *payload = 0;
    float *pcm;
    char sha[65], head[17], tail[9];
    musicpack_status st;

    if (source_open(&s, spec) != 0)
        return 1;
    acc = musicpack_waveform_acc_new(s.rate, s.channels);
    if (acc == 0) {
        fprintf(stderr, "accumulator creation failed\n");
        source_close(&s);
        return 1;
    }
    pcm = (float *) malloc((size_t) CHUNK_FRAMES * s.channels * sizeof(float));
    if (pcm == 0) {
        source_close(&s);
        musicpack_waveform_acc_free(acc);
        return 1;
    }
    for (;;) {
        size_t got = source_read(&s, pcm, CHUNK_FRAMES);
        if (got == 0)
            break;
        st = musicpack_waveform_acc_feed_f32(acc, pcm, got);
        if (st != MUSICPACK_OK) {
            fprintf(stderr, "waveform feed failed\n");
            free(pcm);
            source_close(&s);
            musicpack_waveform_acc_free(acc);
            return 1;
        }
    }
    free(pcm);
    source_close(&s);
    st = musicpack_waveform_acc_finish(acc, &buckets, &bucket_count);
    musicpack_waveform_acc_free(acc);
    if (st != MUSICPACK_OK) {
        fprintf(stderr, "waveform finish failed\n");
        return 1;
    }
    st = musicpack_waveform_encode(buckets, bucket_count, &payload, &payload_len);
    free(buckets);
    if (st != MUSICPACK_OK) {
        fprintf(stderr, "waveform encode failed\n");
        return 1;
    }
    if (musicpack_sha256(payload, payload_len, sha, sizeof sha) != MUSICPACK_OK) {
        free(payload);
        return 1;
    }
    {
        unsigned char h[8] = { 0 }, t[4] = { 0 };
        size_t hn = payload_len < 8 ? payload_len : 8;
        size_t tn = payload_len < 4 ? payload_len : 4;
        if (hn > 0)
            memcpy(h, payload, hn);
        if (tn > 0)
            memcpy(t, payload + payload_len - tn, tn);
        put_hex(h, 8, head);
        put_hex(t, 4, tail);
    }
    printf("waveform %s points=%zu sha256=%s head=%s tail=%s\n", label,
           bucket_count, sha, head, tail);
    free(payload);
    return 0;
}

static int
emit_loudness(const char *label, const char *spec)
{
    source s;
    musicpack_meter *meter;
    float *pcm;
    double lufs = 0.0, tp = 0.0;

    if (source_open(&s, spec) != 0)
        return 1;
    meter = musicpack_meter_new(s.channels, s.rate, 0);
    if (meter == 0) {
        fprintf(stderr, "meter creation failed for '%s'\n", label);
        source_close(&s);
        return 1;
    }
    pcm = (float *) malloc((size_t) CHUNK_FRAMES * s.channels * sizeof(float));
    if (pcm == 0) {
        source_close(&s);
        musicpack_meter_free(meter);
        return 1;
    }
    for (;;) {
        size_t got = source_read(&s, pcm, CHUNK_FRAMES);
        if (got == 0)
            break;
        if (musicpack_meter_add_frames(meter, pcm, got) != MUSICPACK_OK) {
            fprintf(stderr, "meter feed failed\n");
            free(pcm);
            source_close(&s);
            musicpack_meter_free(meter);
            return 1;
        }
    }
    free(pcm);
    source_close(&s);
    if (musicpack_meter_result(meter, &lufs, &tp) != MUSICPACK_OK) {
        fprintf(stderr, "meter result failed\n");
        musicpack_meter_free(meter);
        return 1;
    }
    musicpack_meter_free(meter);
    printf("loudness %s lufs=%.17g tp=%.17g\n", label, lufs, tp);
    return 0;
}

int
main(int argc, char **argv)
{
    if (argc != 4) {
        fprintf(stderr,
                "usage: %s <waveform|loudness> <label> <source>\n", argv[0]);
        return 2;
    }
    if (strcmp(argv[1], "waveform") == 0)
        return emit_waveform(argv[2], argv[3]);
    if (strcmp(argv[1], "loudness") == 0)
        return emit_loudness(argv[2], argv[3]);
    fprintf(stderr, "mode must be waveform or loudness\n");
    return 2;
}

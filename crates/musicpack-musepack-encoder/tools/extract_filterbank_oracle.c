/*
 * Temporary migration tooling (Phase 15D), NOT part of the MusicPack build.
 *
 * Extracts bit-exact analysis-filterbank oracles from the existing C
 * implementation so the Rust port can be gated without the C encoder at test
 * time. It links the reference `libmpcenc_static.a` and calls the *production*
 * `Klemm` / `Analyse_Init` / `Analyse_Filter` (scalar kernels forced), so it
 * neither copies nor modifies encoder code.
 *
 * It is not built by Cargo and no Rust test invokes it.
 *
 * Build (from this directory), with a reference build present at ../musicpack:
 *
 *   cc -O0 -ffp-contract=off -std=gnu11 \
 *      -I <reference>/codec/include -I <reference>/codec/libmpcenc \
 *      extract_filterbank_oracle.c \
 *      <reference>/build/codec/libmpcenc/libmpcenc_static.a \
 *      -lm -o /tmp/extract_filterbank_oracle
 *
 * Usage:
 *   extract_filterbank_oracle cases  <outdir>      # write *.f32le + manifest
 *   extract_filterbank_oracle tables <outdir>      # write Ci_opt/M bit dumps
 *
 * Output representation: little-endian IEEE-754 f32 bit patterns, band-major
 * (for each band 0..=31: 36 L coefficients then 36 R coefficients), one frame
 * per Analyse_Filter call, concatenated in call order.
 *
 * SPDX-License-Identifier: LGPL-2.1-or-later
 */

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include <mpc/datatypes.h>

#include "libmpcenc.h"

#define MAXBAND 31
#define SUBBAND_SAMPLES 36

extern float Ci_opt[512];
extern float M[1024];

typedef enum {
    K_SILENCE,
    K_IMPULSE,
    K_CONSTANT,
    K_ALTERNATING,
    K_RAMP,
    K_TRANSIENT,
    K_STEREO,
    K_STATECARRY
} kind_t;

typedef struct {
    const char *name;
    kind_t kind;
    int frames;       /* number of Analyse_Filter calls */
    int capture_init; /* 1: capture Analyse_Init output and no frames */
    uint32_t seed_l;
    uint32_t seed_r;
} case_t;

static const case_t CASES[] = {
    {"init", K_CONSTANT, 0, 1, 0, 0},
    {"silence", K_SILENCE, 1, 0, 0, 0},
    {"impulse", K_IMPULSE, 1, 0, 0, 0},
    {"constant", K_CONSTANT, 1, 0, 0, 0},
    {"alternating", K_ALTERNATING, 1, 0, 0, 0},
    {"ramp", K_RAMP, 1, 0, 0, 0},
    {"transient", K_TRANSIENT, 1, 0, 0, 0},
    {"stereo", K_STEREO, 1, 0, 0x12345678u, 0x9ABCDEF0u},
    {"state_carry", K_STATECARRY, 3, 0, 0x0BADF00Du, 0xFEEDFACEu},
};
#define NCASES ((int)(sizeof(CASES) / sizeof(CASES[0])))

static uint32_t lcg(uint32_t *state)
{
    *state = *state * 1664525u + 1013904223u;
    return *state;
}

/* Exact in f32: (x>>9) is 23 bits, scaled by 2^-22, offset by -1. */
static float fval(uint32_t x)
{
    return (float)(x >> 9) * (1.0f / 4194304.0f) - 1.0f;
}

static void fill(PCMDataTyp *main, const case_t *c, int frame,
                 uint32_t *sl, uint32_t *sr)
{
    float *l = main->L;
    float *r = main->R;
    int j;

    memset(l, 0, ANABUFFER * sizeof(float));
    memset(r, 0, ANABUFFER * sizeof(float));

    switch (c->kind) {
    case K_SILENCE:
        break;
    case K_IMPULSE:
        l[CENTER] = 1.0f;
        break;
    case K_CONSTANT:
        for (j = 0; j < BLOCK; j++) {
            l[CENTER + j] = 0.5f;
            r[CENTER + j] = 0.5f;
        }
        break;
    case K_ALTERNATING:
        for (j = 0; j < BLOCK; j++) {
            float v = (j & 1) ? -0.5f : 0.5f;
            l[CENTER + j] = v;
            r[CENTER + j] = v;
        }
        break;
    case K_RAMP:
        for (j = 0; j < BLOCK; j++) {
            int t = (frame * BLOCK + j) % 512;
            float v = (float)(t - 256) * (1.0f / 512.0f);
            l[CENTER + j] = v;
            r[CENTER + j] = v;
        }
        break;
    case K_TRANSIENT:
        for (j = 0; j < BLOCK; j++) {
            float v = 0.0f;
            if (j == 100)
                v = 1.0f;
            else if (j == 500)
                v = -0.5f;
            l[CENTER + j] = v;
            r[CENTER + j] = v;
        }
        break;
    case K_STEREO:
    case K_STATECARRY:
        for (j = 0; j < BLOCK; j++) {
            l[CENTER + j] = fval(lcg(sl));
            r[CENTER + j] = fval(lcg(sr));
        }
        break;
    }
}

static int emit_frame(FILE *f, const SubbandFloatTyp *out)
{
    int b;
    for (b = 0; b <= MAXBAND; b++) {
        if (fwrite(out[b].L, sizeof(float), SUBBAND_SAMPLES, f) != SUBBAND_SAMPLES)
            return -1;
        if (fwrite(out[b].R, sizeof(float), SUBBAND_SAMPLES, f) != SUBBAND_SAMPLES)
            return -1;
    }
    return 0;
}

static uint32_t bits(float v)
{
    uint32_t u;
    memcpy(&u, &v, sizeof(u));
    return u;
}

static void dump_bits(FILE *f, const float *values, int count)
{
    int i;
    for (i = 0; i < count; i++)
        fprintf(f, "%08x\n", bits(values[i]));
}

int main(int argc, char **argv)
{
    const char *mode;
    const char *outdir;
    int c;

    if (argc < 3) {
        fprintf(stderr, "usage: %s cases|tables <outdir>\n", argv[0]);
        return 2;
    }
    mode = argv[1];
    outdir = argv[2];

    /* Exactly once: Klemm() mutates Ci_opt in place. */
    mpc_enc_set_impl(MPC_ENC_SCALAR);
    Klemm();

    if (strcmp(mode, "tables") == 0) {
        char path[4096];
        FILE *f;
        snprintf(path, sizeof(path), "%s/ci_opt_bits.txt", outdir);
        f = fopen(path, "w");
        if (!f) { perror(path); return 1; }
        dump_bits(f, Ci_opt, 512);
        fclose(f);
        snprintf(path, sizeof(path), "%s/modulation_bits.txt", outdir);
        f = fopen(path, "w");
        if (!f) { perror(path); return 1; }
        dump_bits(f, M, 1024);
        fclose(f);
        return 0;
    }

    if (strcmp(mode, "cases") != 0) {
        fprintf(stderr, "unknown mode '%s'\n", mode);
        return 2;
    }

    printf("# name kind frames calls max_band bytes init_l_bits init_r_bits input_xor\n");
    for (c = 0; c < NCASES; c++) {
        const case_t *cs = &CASES[c];
        PCMDataTyp main;
        SubbandFloatTyp out[32];
        uint32_t sl = cs->seed_l, sr = cs->seed_r;
        uint32_t input_xor = 0;
        uint32_t init_l, init_r;
        long bytes = 0;
        char path[4096];
        FILE *f;
        int frame, j;

        /* Each case is an independent stream from the reference start state. */
        mpc_enc_reset_filter();

        if (cs->capture_init) {
            Analyse_Init(0.25f, -0.5f, out, MAXBAND);
            init_l = bits(0.25f);
            init_r = bits(-0.5f);
        } else {
            fill(&main, cs, 0, &sl, &sr);
            init_l = bits(main.L[CENTER]);
            init_r = bits(main.R[CENTER]);
            Analyse_Init(main.L[CENTER], main.R[CENTER], out, MAXBAND);
        }

        snprintf(path, sizeof(path), "%s/%s.f32le", outdir, cs->name);
        f = fopen(path, "wb");
        if (!f) { perror(path); return 1; }

        if (cs->capture_init) {
            if (emit_frame(f, out) != 0) { perror(path); return 1; }
            bytes = (long)(MAXBAND + 1) * SUBBAND_SAMPLES * 2 * (long)sizeof(float);
        } else {
            for (frame = 0; frame < cs->frames; frame++) {
                if (frame > 0)
                    fill(&main, cs, frame, &sl, &sr);
                Analyse_Filter(&main, out, MAXBAND);
                if (emit_frame(f, out) != 0) { perror(path); return 1; }
                bytes += (long)(MAXBAND + 1) * SUBBAND_SAMPLES * 2 * (long)sizeof(float);
                for (j = 0; j < BLOCK; j++) {
                    input_xor ^= bits(main.L[CENTER + j]);
                    input_xor ^= bits(main.R[CENTER + j]);
                }
            }
        }
        fclose(f);
        printf("%s %d %d %d %d %ld %08x %08x %08x\n",
               cs->name, (int)cs->kind, cs->frames,
               cs->capture_init ? 1 : cs->frames, MAXBAND, bytes,
               init_l, init_r, input_xor);
    }
    return 0;
}

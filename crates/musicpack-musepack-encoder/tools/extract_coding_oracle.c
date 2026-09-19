/*
 * Temporary migration tooling (Phase 15E), NOT part of the MusicPack build.
 *
 * Extracts bit-exact coding-layer oracles from the existing C encoder.
 *
 * The coding decisions live partly in static functions of `codec/mpcenc/mpcenc.c`
 * (SCF_Extraktion, Allocate, PNS_SCF, Quantisierung). To reach them without
 * modifying the reference, this tool `#include`s the *unmodified* mpcenc.c with
 * its `main` renamed, then replays the reference frame loop over a real WAV
 * (through the reference WAV reader) and dumps the coding-boundary decisions
 * plus every stage's output.
 *
 * Build (from this directory), with the reference build at <REF>:
 *
 *   REF=/path/to/musicpack
 *   cc -O0 -ffp-contract=off -std=gnu11 -DFAST_MATH \
 *      -I "$REF/codec/include" -I "$REF/codec/libmpcenc" \
 *      -I "$REF/codec/libmpcpsy" -I "$REF/codec/mpcenc" \
 *      extract_coding_oracle.c \
 *      "$REF/codec/mpcenc/wave_in.c" "$REF/codec/mpcenc/keyboard.c" \
 *      "$REF/codec/mpcenc/pipeopen.c" "$REF/codec/mpcenc/stderr.c" \
 *      "$REF/codec/mpcenc/winmsg.c" "$REF/codec/common/tags.c" \
 *      "$REF/build/codec/libmpcenc/libmpcenc_static.a" \
 *      "$REF/build/codec/libmpcpsy/libmpcpsy.a" \
 *      -lm -o /tmp/extract_coding_oracle
 *
 * Usage: extract_coding_oracle <outdir> <input.wav> <quality> [frames]
 *
 * `coding.bin` is a flat little-endian record per frame (36864 bytes); see
 * tests/data/coding/README.md. `ap.bin` holds one record per AP block flush:
 * u32 marker 0x41505A00, u32 byte length, then the block bytes.
 *
 * SPDX-License-Identifier: LGPL-2.1-or-later
 */

#define main mpcenc_unused_main
#include "mpcenc.c"
#undef main

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define MAXBAND 31
#define FRAMES_PER_BLOCK_PWR 2 /* 4 frames per AP block: key + 3 delta frames */

static FILE *g_out;
static FILE *g_ap;

/* Not declared in the reference headers; defined in libmpcenc/quant.c. */
extern void Init_Skalenfaktoren(void);
extern float __SCF[128 + 6];
extern float __invSCF[128 + 6];

static void put_i32(int32_t v) { fwrite(&v, 4, 1, g_out); }
static void put_f32(float v)
{
    uint32_t u;
    memcpy(&u, &v, 4);
    fwrite(&u, 4, 1, g_out);
}
static void put_i16(int16_t v) { fwrite(&v, 2, 1, g_out); }
static void put_f32v(const float *p, int n)
{
    int i;
    for (i = 0; i < n; i++)
        put_f32(p[i]);
}
static void put_i32v(const int32_t *p, int n)
{
    int i;
    for (i = 0; i < n; i++)
        put_i32(p[i]);
}
/* Writes the bit pattern of a float array as i32 (bit-exact, no conversion). */
static void put_bits_float(const float *p, int n)
{
    int i;
    for (i = 0; i < n; i++) {
        uint32_t u;
        memcpy(&u, &p[i], 4);
        put_i32((int32_t)u);
    }
}

int main(int argc, char **argv)
{
    const char *outdir, *input;
    float quality;
    int maxframes, frame;
    PsyModel m;
    mpc_encoder_t e;
    PCMDataTyp Main;
    SubbandFloatTyp X[32];
    SMRTyp SMR;
    wave_t Wave;
    float Power_L[32][3], Power_R[32][3];
    int TransientL[PART_SHORT], TransientR[PART_SHORT], Transient[32];
    mpc_uint64_t AllSamplesRead = 0;
    unsigned int CurrentRead = 0;
    int Silence = 0;
    char path[4096];
    char *membuf = NULL;
    size_t memsize = 0;
    int i;

    if (argc < 3) {
        fprintf(stderr, "usage: %s <outdir> <input.wav> <quality> [frames]\n", argv[0]);
        fprintf(stderr, "       %s <outdir> --tables\n", argv[0]);
        return 2;
    }
    outdir = argv[1];
    if (strcmp(argv[2], "--tables") == 0) {
        FILE *tf;
        char tpath[4096];
        Init_FastMath();
        Init_Skalenfaktoren();
        snprintf(tpath, sizeof(tpath), "%s/scf_tables.txt", outdir);
        tf = fopen(tpath, "w");
        if (!tf) { perror(tpath); return 1; }
        for (i = 0; i < 134; i++) {
            uint32_t u;
            memcpy(&u, &__SCF[i], 4); fprintf(tf, "%08x\n", u);
        }
        for (i = 0; i < 134; i++) {
            uint32_t u;
            memcpy(&u, &__invSCF[i], 4); fprintf(tf, "%08x\n", u);
        }
        fclose(tf);
        snprintf(tpath, sizeof(tpath), "%s/penalty.txt", outdir);
        tf = fopen(tpath, "w");
        if (!tf) { perror(tpath); return 1; }
        for (i = 0; i < 256; i++) fprintf(tf, "%u\n", (unsigned)Penalty[i]);
        fclose(tf);
        return 0;
    }
    if (argc < 4) {
        fprintf(stderr, "usage: %s <outdir> <input.wav> <quality> [frames]\n", argv[0]);
        return 2;
    }
    input = argv[2];
    quality = (float)atof(argv[3]);
    maxframes = argc > 4 ? atoi(argv[4]) : 8;

    Init_FastMath();

    memset(&m, 0, sizeof(m));
    memset(&e, 0, sizeof(e));
    memset(&Main, 0, sizeof(Main));
    memset(&Wave, 0, sizeof(Wave));

    m.SCF_Index_L = e.SCF_Index_L;
    m.SCF_Index_R = e.SCF_Index_R;
    Init_Psychoakustik(&m);

    if (Open_WAV_Header(&Wave, input) < 0) {
        fprintf(stderr, "cannot open WAV '%s'\n", input);
        return 1;
    }
    if (Read_WAV_Header(&Wave) != 0) {
        fprintf(stderr, "invalid WAV header '%s'\n", input);
        return 1;
    }
    m.SampleFreq = Wave.SampleFreq;
    SamplesInWAVE = Wave.PCMSamples;
    SetQualityParams(&m, quality);

    FramesBlockPwr = FRAMES_PER_BLOCK_PWR;
    SeekDistance = 1;
    mpc_encoder_init(&e, SamplesInWAVE, FramesBlockPwr, SeekDistance);
    Init_Psychoakustiktabellen(&m);
    e.MS_Channelmode = m.MS_Channelmode;
    e.seek_ref = 0;

    fprintf(stderr,
            "cfg: wav=%s rate=%g ch=%u samples=%llu | quality=%.2f MainQual=%d "
            "FullQual=%.3f MaxBand=%d minSMR=%g NS_Order=%u PNS=%g MS=%u Comb=%d\n",
            input, (double)Wave.SampleFreq, (unsigned)Wave.Channels,
            (unsigned long long)SamplesInWAVE, quality, m.MainQual, m.FullQual,
            m.Max_Band, m.minSMR, m.NS_Order, m.PNS, m.MS_Channelmode,
            m.CombPenalities);

    snprintf(path, sizeof(path), "%s/coding.bin", outdir);
    g_out = fopen(path, "wb");
    if (!g_out) { perror(path); return 1; }
    snprintf(path, sizeof(path), "%s/ap.bin", outdir);
    g_ap = fopen(path, "wb");
    if (!g_ap) { perror(path); return 1; }

    memset(X, 0, sizeof(X));

    CurrentRead = (unsigned int)Read_WAV_Samples(
        &Wave, (int)mini(BLOCK, SamplesInWAVE - AllSamplesRead), &Main, CENTER,
        ScalingFactorl, ScalingFactorr, &Silence);
    AllSamplesRead += CurrentRead;
    if (CurrentRead > 0) {
        fill_float(Main.L, Main.L[CENTER], CENTER);
        fill_float(Main.R, Main.R[CENTER], CENTER);
        fill_float(Main.M, Main.M[CENTER], CENTER);
        fill_float(Main.S, Main.S[CENTER], CENTER);
    }
    Analyse_Init(Main.L[CENTER], Main.R[CENTER], X, m.Max_Band);

    for (frame = 0; frame < maxframes; frame++) {
        if (CurrentRead == 0 && frame > 0)
            break;
        if (CurrentRead < BLOCK && frame > 0) {
            fill_float(Main.L + (CENTER + CurrentRead), Main.L[CENTER + CurrentRead - 1], BLOCK - CurrentRead);
            fill_float(Main.R + (CENTER + CurrentRead), Main.R[CENTER + CurrentRead - 1], BLOCK - CurrentRead);
            fill_float(Main.M + (CENTER + CurrentRead), Main.M[CENTER + CurrentRead - 1], BLOCK - CurrentRead);
            fill_float(Main.S + (CENTER + CurrentRead), Main.S[CENTER + CurrentRead - 1], BLOCK - CurrentRead);
        }

        memset(e.Res_L, 0, sizeof(e.Res_L));
        memset(e.Res_R, 0, sizeof(e.Res_R));

        Analyse_Filter(&Main, X, m.Max_Band);
        SMR = Psychoakustisches_Modell(&m, 31, &Main, TransientL, TransientR);
        if (m.minSMR > 0)
            RaiseSMR(&m, m.Max_Band, &SMR);
        if (m.MS_Channelmode > 0)
            MS_LR_Entscheidung(m.Max_Band, e.MS_Flag, &SMR, X);
        TransientenCalc(Transient, TransientL, TransientR);

        /* ---- frozen decisions + post-MS X ---- */
        put_f32v(SMR.L, 32); put_f32v(SMR.R, 32);
        put_f32v(SMR.M, 32); put_f32v(SMR.S, 32);
        put_bits_float(m.state.ANSspec_L, 512);
        put_bits_float(m.state.ANSspec_R, 512);
        put_bits_float(m.state.ANSspec_M, 512);
        put_bits_float(m.state.ANSspec_S, 512);
        put_i32v(Transient, 32);
        for (i = 0; i < 32; i++) put_i32(e.MS_Flag[i] ? 1 : 0);
        { int b, n; for (b = 0; b <= MAXBAND; b++) for (n = 0; n < 36; n++) { put_f32(X[b].L[n]); put_f32(X[b].R[n]); } }

        /* ---- SCF ---- */
        SCF_Extraktion(&m, &e, m.Max_Band, X, Power_L, Power_R);
        { int b, k; for (b = 0; b <= MAXBAND; b++) for (k = 0; k < 3; k++) { put_f32(Power_L[b][k]); put_f32(Power_R[b][k]); } }
        { int b, k; for (b = 0; b <= MAXBAND; b++) for (k = 0; k < 3; k++) { put_i32(e.SCF_Index_L[b][k]); put_i32(e.SCF_Index_R[b][k]); } }
        put_f32v(m.SNR_comp_L, 32); put_f32v(m.SNR_comp_R, 32);

        /* ---- NS ---- */
        if (m.NS_Order > 0)
            NS_Analyse(&m, m.Max_Band, e.MS_Flag, SMR, Transient);
        { int b; for (b = 0; b <= MAXBAND; b++) { put_i32((int32_t)m.NS_Order_L[b]); put_i32((int32_t)m.NS_Order_R[b]); } }
        { int b, k; for (b = 0; b <= MAXBAND; b++) for (k = 0; k < MAX_NS_ORDER; k++) { put_f32(m.FIR_L[b][k]); put_f32(m.FIR_R[b][k]); } }
        put_f32v(m.SNR_comp_L, 32); put_f32v(m.SNR_comp_R, 32);

        /* ---- Allocate ---- */
        Allocate(m.Max_Band, e.Res_L, X[0].L, e.SCF_Index_L[0], m.SNR_comp_L, SMR.L, Power_L, Transient, m.PNS);
        Allocate(m.Max_Band, e.Res_R, X[0].R, e.SCF_Index_R[0], m.SNR_comp_R, SMR.R, Power_R, Transient, m.PNS);
        { int b; for (b = 0; b <= MAXBAND; b++) { put_i32(e.Res_L[b]); put_i32(e.Res_R[b]); } }
        { int b, k; for (b = 0; b <= MAXBAND; b++) for (k = 0; k < 3; k++) { put_i32(e.SCF_Index_L[b][k]); put_i32(e.SCF_Index_R[b][k]); } }
        { int b, n; for (b = 0; b <= MAXBAND; b++) for (n = 0; n < 36; n++) { put_f32(X[b].L[n]); put_f32(X[b].R[n]); } }

        /* ---- Quantisierung ---- */
        Quantisierung(&m, m.Max_Band, e.Res_L, e.Res_R, X, e.Q);
        { int b, n; for (b = 0; b <= MAXBAND; b++) for (n = 0; n < 36; n++) { put_i16(e.Q[b].L[n]); put_i16(e.Q[b].R[n]); } }

        /* ---- Huffman / AP ---- */
        free(membuf); membuf = NULL; memsize = 0;
        e.outputFile = open_memstream(&membuf, &memsize);
        if (!e.outputFile) { perror("open_memstream"); return 1; }
        writeBitstream_SV8(&e, m.Max_Band);
        fflush(e.outputFile);
        fclose(e.outputFile);
        e.outputFile = NULL;
        if (memsize > 0) {
            uint32_t marker = 0x41505A00u, size = (uint32_t)memsize;
            fwrite(&marker, 4, 1, g_ap);
            fwrite(&size, 4, 1, g_ap);
            fwrite(membuf, 1, memsize, g_ap);
        }

        memmove(Main.L, Main.L + BLOCK, CENTER * sizeof(float));
        memmove(Main.R, Main.R + BLOCK, CENTER * sizeof(float));
        memmove(Main.M, Main.M + BLOCK, CENTER * sizeof(float));
        memmove(Main.S, Main.S + BLOCK, CENTER * sizeof(float));
        CurrentRead = (unsigned int)Read_WAV_Samples(
            &Wave, (int)mini(BLOCK, SamplesInWAVE - AllSamplesRead), &Main, CENTER,
            ScalingFactorl, ScalingFactorr, &Silence);
        AllSamplesRead += CurrentRead;
    }

    fclose(g_out);
    fclose(g_ap);
    free(membuf);

    printf("%s %s q=%.2f frames=%d block_pwr=%d max_band=%d\n",
           input, outdir, quality, frame, FRAMES_PER_BLOCK_PWR, m.Max_Band);
    return 0;
}

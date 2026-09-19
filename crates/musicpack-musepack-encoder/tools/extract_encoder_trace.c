/*
 * Temporary migration tooling (Phase 15G), NOT part of the MusicPack build.
 *
 * Replays the exact `mpcenc.c` frame loop (including the decoder-delay frames)
 * for one WAV and dumps a per-frame diagnostic trace: Res, SCF, MS flags, Q and
 * the raw stage outputs. Used to locate the first divergence between the Rust
 * encoder and the C reference. It `#include`s the unmodified `mpcenc.c` with
 * `main` renamed, so it neither copies nor modifies encoder code.
 *
 * Build (from this directory):
 *
 *   REF=/path/to/musicpack
 *   cc -O0 -ffp-contract=off -std=gnu11 -DFAST_MATH \
 *      -I "$REF/codec/include" -I "$REF/codec/libmpcenc" \
 *      -I "$REF/codec/libmpcpsy" -I "$REF/codec/mpcenc" \
 *      extract_encoder_trace.c \
 *      "$REF/codec/mpcenc/wave_in.c" "$REF/codec/mpcenc/keyboard.c" \
 *      "$REF/codec/mpcenc/pipeopen.c" "$REF/codec/mpcenc/stderr.c" \
 *      "$REF/codec/mpcenc/winmsg.c" "$REF/codec/common/tags.c" \
 *      "$REF/build/codec/libmpcenc/libmpcenc_static.a" \
 *      "$REF/build/codec/libmpcpsy/libmpcpsy.a" \
 *      -lm -o /tmp/extract_encoder_trace
 *
 * Usage: extract_encoder_trace <out.bin> <input.wav> <quality>
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

extern const int wl[PART_LONG], wh[PART_LONG];
extern const float iw[PART_LONG];

/* Diagnostic copy of the reference `PartitionEnergy` (psy.c), used only to
 * expose `Ls` for divergence tracing. */
static void partition_energy_tool(float *erg0, float *erg1, const float *spec0, const float *spec1)
{
    unsigned int n, k;
    float e0, e1;
    n = 0;
    for (; n < 23; n++) { k = wh[n] - wl[n]; e0 = *spec0++; e1 = *spec1++; while (k--) { e0 += *spec0++; e1 += *spec1++; } *erg0++ = e0; *erg1++ = e1; }
    for (; n < 48; n++) { k = wh[n] - wl[n]; e0 = sqrt(*spec0++); e1 = sqrt(*spec1++); while (k--) { e0 += sqrt(*spec0++); e1 += sqrt(*spec1++); } *erg0++ = e0 * e0 * iw[n]; *erg1++ = e1 * e1 * iw[n]; }
    for (; n < PART_LONG; n++) { k = wh[n] - wl[n]; e0 = *spec0++; e1 = *spec1++; while (k--) { e0 += *spec0++; e1 += *spec1++; } *erg0++ = e0; *erg1++ = e1; }
}

static FILE *g_out;

static void weighted_partition_energy_tool(float *erg0, float *erg1, const float *spec0, const float *spec1, const float *cw0, const float *cw1)
{
    unsigned int n, k;
    float e0, e1;
    n = 0;
    for (; n < 23; n++) { k = wh[n] - wl[n]; e0 = *spec0++ * *cw0++; e1 = *spec1++ * *cw1++; while (k--) { e0 += *spec0++ * *cw0++; e1 += *spec1++ * *cw1++; } *erg0++ = e0; *erg1++ = e1; }
    for (; n < 48; n++) { k = wh[n] - wl[n]; e0 = sqrt(*spec0++ * *cw0++); e1 = sqrt(*spec1++ * *cw1++); while (k--) { e0 += sqrt(*spec0++ * *cw0++); e1 += sqrt(*spec1++ * *cw1++); } *erg0++ = e0 * e0 * iw[n]; *erg1++ = e1 * e1 * iw[n]; }
    for (; n < PART_LONG; n++) { k = wh[n] - wl[n]; e0 = *spec0++ * *cw0++; e1 = *spec1++ * *cw1++; while (k--) { e0 += *spec0++ * *cw0++; e1 += *spec1++ * *cw1++; } *erg0++ = e0; *erg1++ = e1; }
}

static void put_i32(int32_t v) { fwrite(&v, 4, 1, g_out); }
static void put_i16(int16_t v) { fwrite(&v, 2, 1, g_out); }
static void put_f32(float v)
{
    uint32_t u;
    memcpy(&u, &v, 4);
    fwrite(&u, 4, 1, g_out);
}

int main(int argc, char **argv)
{
    PsyModel m;
    mpc_encoder_t e;
    PCMDataTyp Main;
    SubbandFloatTyp X[32];
    SubbandFloatTyp Xraw[32];
    SubbandFloatTyp Xms[32];
    float AnsL[512];
    float ErgL[512], ErgR[512], PhsL[512], PhsR[512];
    float ErgL2[512], ErgR2[512], PhsL2[512], PhsR2[512];
    float XergL[1024], XergR[1024];
    float CwL[512];
    float CwR[512];
    float Xs[3 * 512], Ys[3 * 512];
    float XRs[3 * 512], YRs[3 * 512];
    float LsLCap[PART_LONG], LsRCap[PART_LONG];
    float Power_L[32][3], Power_R[32][3];
    SMRTyp SMR;
    int TransientL[PART_SHORT], TransientR[PART_SHORT], Transient[32];
    wave_t Wave;
    unsigned int N;
    unsigned int maxframes;
    unsigned int CurrentRead = 0;
    mpc_uint64_t AllSamplesRead = 0;
    int Silence = 0;
    int OldSilence = 0;

    if (argc < 5) {
        fprintf(stderr, "usage: %s <trace.bin> <stream.mpc|-> <input.wav> <quality> [maxframes]\n", argv[0]);
        return 2;
    }
    maxframes = argc > 5 ? (unsigned)atoi(argv[5]) : 1000000u;

    Init_FastMath();

    memset(&m, 0, sizeof(m));
    memset(&e, 0, sizeof(e));
    memset(&Main, 0, sizeof(Main));
    memset(&Wave, 0, sizeof(Wave));
    memset(X, 0, sizeof(X));

    m.SCF_Index_L = e.SCF_Index_L;
    m.SCF_Index_R = e.SCF_Index_R;
    Init_Psychoakustik(&m);
    mpc_enc_set_impl(MPC_ENC_SCALAR);
    mpc_psy_set_impl(MPC_PSY_SCALAR);

    if (Open_WAV_Header(&Wave, argv[3]) < 0) { fprintf(stderr, "cannot open WAV\n"); return 1; }
    if (Read_WAV_Header(&Wave) != 0) { fprintf(stderr, "invalid WAV header\n"); return 1; }
    m.SampleFreq = Wave.SampleFreq;
    SamplesInWAVE = Wave.PCMSamples;
    SetQualityParams(&m, (float)atof(argv[4]));

    FramesBlockPwr = 6;
    SeekDistance = 1;
    mpc_encoder_init(&e, SamplesInWAVE, FramesBlockPwr, SeekDistance);
    Init_Psychoakustiktabellen(&m);
    e.MS_Channelmode = m.MS_Channelmode;
    e.seek_ref = 0;

    g_out = fopen(argv[1], "wb");
    if (!g_out) { perror(argv[1]); return 1; }

    /* Optional full scalar-forced SV8 stream. */
    if (strcmp(argv[2], "-") != 0) {
        e.outputFile = fopen(argv[2], "wb");
        if (!e.outputFile) { perror(argv[2]); return 1; }
        e.seek_ref = mpc_file_tell(e.outputFile);
        writeMagic(&e);
        writeStreamInfo(&e, m.Max_Band, m.MS_Channelmode > 0, SamplesInWAVE, 0, m.SampleFreq, Wave.Channels);
        writeBlock(&e, "SH", MPC_TRUE, 0);
        writeGainInfo(&e, 0, 0, 0, 0);
        writeBlock(&e, "RG", MPC_FALSE, 0);
        writeEncoderInfo(&e, m.FullQual, m.PNS > 0, MPCENC_MAJOR, MPCENC_MINOR, MPCENC_BUILD);
        writeBlock(&e, "EI", MPC_FALSE, 0);
        e.seek_ptr = mpc_file_tell(e.outputFile);
        writeBits(&e, 0, 16);
        writeBits(&e, 0, 24);
        writeBlock(&e, "SO", MPC_FALSE, 0);
    }

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

    for (N = 0; (mpc_uint64_t)N * BLOCK < SamplesInWAVE + DECODER_DELAY && N < maxframes; N++) {
        int processed = 0;
        if (CurrentRead < BLOCK && N > 0) {
            fill_float(Main.L + (CENTER + CurrentRead), Main.L[CENTER + CurrentRead - 1], BLOCK - CurrentRead);
            fill_float(Main.R + (CENTER + CurrentRead), Main.R[CENTER + CurrentRead - 1], BLOCK - CurrentRead);
            fill_float(Main.M + (CENTER + CurrentRead), Main.M[CENTER + CurrentRead - 1], BLOCK - CurrentRead);
            fill_float(Main.S + (CENTER + CurrentRead), Main.S[CENTER + CurrentRead - 1], BLOCK - CurrentRead);
        }

        memset(e.Res_L, 0, sizeof(e.Res_L));
        memset(e.Res_R, 0, sizeof(e.Res_R));

        if (!Silence || !OldSilence) {
            processed = 1;
            memcpy(Xs, m.state.Xsave_L, sizeof(Xs));
            memcpy(Ys, m.state.Ysave_L, sizeof(Ys));
            memcpy(XRs, m.state.Xsave_R, sizeof(XRs));
            memcpy(YRs, m.state.Ysave_R, sizeof(YRs));
            Analyse_Filter(&Main, X, m.Max_Band);
            memcpy(Xraw, X, sizeof(X));
            SMR = Psychoakustisches_Modell(&m, 31, &Main, TransientL, TransientR);
            if (m.minSMR > 0) RaiseSMR(&m, m.Max_Band, &SMR);
            if (m.MS_Channelmode > 0) MS_LR_Entscheidung(m.Max_Band, e.MS_Flag, &SMR, X);
            memcpy(Xms, X, sizeof(X));
            memcpy(AnsL, m.state.ANSspec_L, sizeof(AnsL));
            SCF_Extraktion(&m, &e, m.Max_Band, X, Power_L, Power_R);
            TransientenCalc(Transient, TransientL, TransientR);
            if (m.NS_Order > 0) NS_Analyse(&m, m.Max_Band, e.MS_Flag, SMR, Transient);
            Allocate(m.Max_Band, e.Res_L, X[0].L, e.SCF_Index_L[0], m.SNR_comp_L, SMR.L, Power_L, Transient, m.PNS);
            Allocate(m.Max_Band, e.Res_R, X[0].R, e.SCF_Index_R[0], m.SNR_comp_R, SMR.R, Power_R, Transient, m.PNS);
            Quantisierung(&m, m.Max_Band, e.Res_L, e.Res_R, X, e.Q);
        }
        OldSilence = Silence;

        if (e.outputFile != NULL)
            writeBitstream_SV8(&e, m.Max_Band);

        PolarSpec1024_2(Main.L, Main.R, ErgL, ErgR, PhsL, PhsR);
        PolarSpec1024_2(Main.L + 576, Main.R + 576, ErgL2, ErgR2, PhsL2, PhsR2);
        PowSpec2048_2(Main.L, Main.R, XergL, XergR);
        /* Diagnostic copy of CalcUnpred for window 1 (L). */
        memmove(Xs + 512, Xs, 1024 * sizeof(float));
        memmove(Ys + 512, Ys, 1024 * sizeof(float));
        { int n; for (n = 0; n < 512; n++) {
            float tmp = COSF((Ys[n] = PhsL[n]) - 2 * Ys[512 + n] + Ys[1024 + n]);
            Xs[n] = SQRTF(ErgL[n]);
            float amp = 2 * Xs[512 + n] - Xs[1024 + n];
            CwL[n] = SQRTF(ErgL[n] + amp * (amp - 2 * Xs[n] * tmp)) / (Xs[n] + FABS(amp));
        } }
        /* Diagnostic copy of CalcUnpred for window 1 (R). */
        memmove(XRs + 512, XRs, 1024 * sizeof(float));
        memmove(YRs + 512, YRs, 1024 * sizeof(float));
        { int n; for (n = 0; n < 512; n++) {
            float tmp = COSF((YRs[n] = PhsR[n]) - 2 * YRs[512 + n] + YRs[1024 + n]);
            XRs[n] = SQRTF(ErgR[n]);
            float amp = 2 * XRs[512 + n] - XRs[1024 + n];
            CwR[n] = SQRTF(ErgR[n] + amp * (amp - 2 * XRs[n] * tmp)) / (XRs[n] + FABS(amp));
        } }

        /* per-frame trace */
        put_i32((int32_t)N);
        put_i32((int32_t)CurrentRead);
        put_i32((int32_t)Silence);
        put_i32(processed);
        { int b; for (b = 0; b < 32; b++) put_i32(e.Res_L[b]); for (b = 0; b < 32; b++) put_i32(e.Res_R[b]); }
        { int b, k; for (b = 0; b < 32; b++) for (k = 0; k < 3; k++) put_i32(e.SCF_Index_L[b][k]);
          for (b = 0; b < 32; b++) for (k = 0; k < 3; k++) put_i32(e.SCF_Index_R[b][k]); }
        { int b; for (b = 0; b < 32; b++) put_i32(e.MS_Flag[b] ? 1 : 0); }
        { int b, n; for (b = 0; b < 32; b++) for (n = 0; n < 36; n++) { put_f32(X[b].L[n]); put_f32(X[b].R[n]); } }
        { int b; for (b = 0; b < 32; b++) put_f32(SMR.L[b]); for (b = 0; b < 32; b++) put_f32(SMR.R[b]); }
        { int i; for (i = 0; i < ANABUFFER; i++) put_f32(Main.L[i]); for (i = 0; i < ANABUFFER; i++) put_f32(Main.R[i]); }
        { int b, n; for (b = 0; b < 32; b++) for (n = 0; n < 36; n++) { put_f32(Xraw[b].L[n]); put_f32(Xraw[b].R[n]); } }
        { int b, n; for (b = 0; b < 32; b++) for (n = 0; n < 36; n++) { put_f32(Xms[b].L[n]); put_f32(Xms[b].R[n]); } }
        { int i; for (i = 0; i < 512; i++) put_f32(AnsL[i]); }
        put_f32(m.state.loud);
        { int i; for (i = 0; i < 8; i++) put_f32(m.state.Xsave_L[i]); }
        { int i; for (i = 0; i < 8; i++) put_f32(m.state.PreThr_L[i]); }
        { int i; for (i = 0; i < 8; i++) put_f32(m.state.tmp_Mask_L[i]); }
        { int i; for (i = 0; i < 4; i++) put_f32(m.state.a[i]); }
        { int i; for (i = 0; i < 4; i++) put_f32(m.state.b[i]); }
        { int i; for (i = 0; i < 512; i++) put_f32(ErgL[i]); }
        { float LsL[PART_LONG], LsR[PART_LONG]; int i; partition_energy_tool(LsL, LsR, ErgL, ErgR); for (i = 0; i < PART_LONG; i++) put_f32(LsL[i]); memcpy(LsLCap, LsL, sizeof(LsL)); memcpy(LsRCap, LsR, sizeof(LsR)); }
        { int i; for (i = 0; i < 512; i++) put_f32(PhsL[i]); }
        { int i; for (i = 0; i < 16; i++) put_i32(m.state.Vocal_L[i]); }
        { int i; for (i = 0; i < 512; i++) put_f32(ErgL2[i]); }
        { int i; for (i = 0; i < 512; i++) put_f32(CwL[i]); }
        { float WergL[PART_LONG], WergR[PART_LONG]; int i; weighted_partition_energy_tool(WergL, WergR, ErgL, ErgR, CwL, CwL); for (i = 0; i < PART_LONG; i++) put_f32(WergL[i]); }
        { int i; for (i = 0; i < PART_SHORT; i++) put_i32(TransientL[i]); }
        /* Extended full-state dump (Phase 15G.1): persistent state, R-side
         * chains, NS analysis and quantiser outputs. */
        { int i; for (i = 0; i < PART_SHORT; i++) put_i32(TransientR[i]); }
        { int i; for (i = 0; i < PART_LONG; i++) put_f32(m.state.PreThr_L[i]); for (i = 0; i < PART_LONG; i++) put_f32(m.state.PreThr_R[i]); }
        { int i; for (i = 0; i < PART_LONG; i++) put_f32(m.state.tmp_Mask_L[i]); for (i = 0; i < PART_LONG; i++) put_f32(m.state.tmp_Mask_R[i]); }
        { int i; for (i = 0; i < PART_LONG; i++) put_f32(m.state.T_L[i]); for (i = 0; i < PART_LONG; i++) put_f32(m.state.T_R[i]); }
        { int i; for (i = 0; i < PART_LONG; i++) put_f32(m.state.a[i]); for (i = 0; i < PART_LONG; i++) put_f32(m.state.b[i]); for (i = 0; i < PART_LONG; i++) put_f32(m.state.c[i]); for (i = 0; i < PART_LONG; i++) put_f32(m.state.d[i]); }
        { int i, k; for (k = 0; k < 2; k++) for (i = 0; i < PART_SHORT; i++) put_f32(m.state.pre_erg_L[k][i]); for (k = 0; k < 2; k++) for (i = 0; i < PART_SHORT; i++) put_f32(m.state.pre_erg_R[k][i]); }
        { int i; for (i = 0; i < 3 * 512; i++) put_f32(m.state.Xsave_L[i]); for (i = 0; i < 3 * 512; i++) put_f32(m.state.Xsave_R[i]); }
        { int i; for (i = 0; i < 3 * 512; i++) put_f32(m.state.Ysave_L[i]); for (i = 0; i < 3 * 512; i++) put_f32(m.state.Ysave_R[i]); }
        { int i; for (i = 0; i < MAX_CVD_LINE + 4; i++) put_i32(m.state.Vocal_L[i]); for (i = 0; i < MAX_CVD_LINE + 4; i++) put_i32(m.state.Vocal_R[i]); }
        { int i; for (i = 0; i < 512; i++) put_f32(m.state.ANSspec_R[i]); for (i = 0; i < 512; i++) put_f32(m.state.ANSspec_M[i]); for (i = 0; i < 512; i++) put_f32(m.state.ANSspec_S[i]); }
        { int i; for (i = 0; i < 512; i++) put_f32(ErgR[i]); }
        { int i; for (i = 0; i < 512; i++) put_f32(PhsR[i]); }
        { int i; for (i = 0; i < 512; i++) put_f32(CwR[i]); }
        { int i; for (i = 0; i < PART_LONG; i++) put_f32(LsRCap[i]); }
        { int b; for (b = 0; b < 32; b++) put_i32((int32_t)m.NS_Order_L[b]); for (b = 0; b < 32; b++) put_i32((int32_t)m.NS_Order_R[b]); }
        { int b, k; for (b = 0; b < 32; b++) for (k = 0; k < MAX_NS_ORDER; k++) put_f32(m.FIR_L[b][k]); for (b = 0; b < 32; b++) for (k = 0; k < MAX_NS_ORDER; k++) put_f32(m.FIR_R[b][k]); }
        { int b; for (b = 0; b < 32; b++) put_f32(m.SNR_comp_L[b]); for (b = 0; b < 32; b++) put_f32(m.SNR_comp_R[b]); }
        { int b, n; for (b = 0; b < 32; b++) for (n = 0; n < 36; n++) put_i16(e.Q[b].L[n]); for (b = 0; b < 32; b++) for (n = 0; n < 36; n++) put_i16(e.Q[b].R[n]); }
        { int i; for (i = 0; i < 1024; i++) put_f32(XergL[i]); }
        { int i; for (i = 0; i < 1024; i++) put_f32(XergR[i]); }

        memmove(Main.L, Main.L + BLOCK, CENTER * sizeof(float));
        memmove(Main.R, Main.R + BLOCK, CENTER * sizeof(float));
        memmove(Main.M, Main.M + BLOCK, CENTER * sizeof(float));
        memmove(Main.S, Main.S + BLOCK, CENTER * sizeof(float));

        CurrentRead = (unsigned int)Read_WAV_Samples(
            &Wave, (int)mini(BLOCK, SamplesInWAVE - AllSamplesRead), &Main, CENTER,
            ScalingFactorl, ScalingFactorr, &Silence);
        AllSamplesRead += CurrentRead;
        if (myfeof(Wave.fp)) SamplesInWAVE = AllSamplesRead;
    }
    fclose(g_out);
    if (e.outputFile != NULL) {
        if (e.framesInBlock != 0) {
            if ((e.block_cnt & ((1 << e.seek_pwr) - 1)) == 0) {
                e.seek_table[e.seek_pos] = mpc_file_tell(e.outputFile);
                e.seek_pos++;
            }
            e.block_cnt++;
            writeBlock(&e, "AP", MPC_FALSE, 0);
        }
        writeSeekTable(&e);
        writeBlock(&e, "ST", MPC_FALSE, 0);
        writeBlock(&e, "SE", MPC_FALSE, 0);
        fclose(e.outputFile);
    }
    fprintf(stderr, "trace: %u frames, %llu samples\n", N, (unsigned long long)SamplesInWAVE);
    return 0;
}

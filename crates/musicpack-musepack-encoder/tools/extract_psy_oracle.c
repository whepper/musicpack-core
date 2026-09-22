/*
 * Temporary migration tooling (Phase 15F), NOT part of the MusicPack build.
 *
 * Extracts bit-exact psychoacoustic oracles from the existing C implementation
 * so the Rust port can be gated without the C encoder at test time. It links
 * the reference `libmpcpsy.a` and calls the *production* psychoacoustic entry
 * points (`Init_Psychoakustik`, `SetQualityParams`,
 * `Init_Psychoakustiktabellen`, `Psychoakustisches_Modell`, `RaiseSMR`,
 * `MS_LR_Entscheidung`, `TransientenCalc`, `PowSpec*`, `PolarSpec1024`,
 * `Cepstrum2048`) with the scalar spectrum kernels forced. It neither copies
 * nor modifies encoder code.
 *
 * It is not built by Cargo and no Rust test invokes it.
 *
 * Build (from this directory), with a reference build present at ../musicpack:
 *
 *   cc -O0 -ffp-contract=off -std=gnu11 \
 *      -I <reference>/codec/include -I <reference>/codec/libmpcpsy \
 *      extract_psy_oracle.c \
 *      <reference>/build/codec/libmpcpsy/libmpcpsy.a \
 *      -lm -o /tmp/extract_psy_oracle
 *
 * Usage:
 *   extract_psy_oracle tables <outdir>   # frozen numerical tables
 *   extract_psy_oracle fft    <outdir>   # level-1 spectrum primitives
 *   extract_psy_oracle math   <outdir>   # level-1 FAST_MATH primitives
 *   extract_psy_oracle model  <outdir>   # level-2/3 model boundaries
 *   extract_psy_oracle ms     <outdir>   # MS_LR_Entscheidung sub-oracle
 *   extract_psy_oracle bases  <outdir>   # J.2: ATH base arrays (psy_bases.txt)
 *   extract_psy_oracle selfcheck <outdir># J.2: bases reconstruct live fft/part/inv
 *   extract_psy_oracle frac <q> <rate> <outdir> # J.2: one fractional dump + params
 *
 * All floating-point values are written as little-endian IEEE-754 f32 bit
 * patterns. Integer fields are written as their f32 value where exactly
 * representable (0/1 flags), or as explicit `u32le` files where noted.
 *
 * SPDX-License-Identifier: LGPL-2.1-or-later
 */

#include <math.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "libmpcpsy.h"

#include "mpc/datatypes.h"
#include "mpc/minimax.h"
#include "mpc/mpcmath.h"

#include <limits.h>

/* Production psychoacoustic entry points (declared in mpcenc.h). */
void   Init_Psychoakustik          ( PsyModel* );
void   Init_Psychoakustiktabellen  ( PsyModel* );
void   SetQualityParams            ( PsyModel*, float );
SMRTyp Psychoakustisches_Modell    ( PsyModel*, const int, const PCMDataTyp*, int*, int* );
void   RaiseSMR                    ( PsyModel*, const int, SMRTyp* );
void   TransientenCalc             ( int*, const int*, const int* );
void   MS_LR_Entscheidung          ( const int, unsigned char*, SMRTyp*, SubbandFloatTyp* );

#define FSUB 36
#define NFRAMES_MAX 4

/* ------------------------------------------------------------------ */
/* deterministic PCM generation                                        */
/* ------------------------------------------------------------------ */

typedef enum {
    K_SILENCE = 0,
    K_IMPULSE,
    K_CONSTANT,
    K_ALTERNATING,
    K_SINE,
    K_MULTITONE,
    K_LOWFREQ,
    K_HIGHFREQ,
    K_NOISE,
    K_TRANSIENT,
    K_STEREO_SAME,
    K_STEREO_DIFF,
    K_LEFT_ONLY,
    K_RIGHT_ONLY,
    K_PHASE_INVERT,
    K_RAMP,
} kind_t;

static uint32_t
lcg ( uint32_t* state )
{
    *state = *state * 1664525u + 1013904223u;
    return *state;
}

/* Exact in f32: (x>>9) has 23 bits, scaled by 2^-22 and biased by -1. */
static float
fval ( uint32_t x )
{
    return (float) (x >> 9) * (1.0f / 4194304.0f) - 1.0f;
}

static float
pcm_left ( int kind, double i, uint32_t* sl )
{
    switch ( kind ) {
    case K_SILENCE:      return 0.0f;
    case K_IMPULSE:      return (i >= 400.0 && i < 401.0) ? 1.0f : 0.0f;
    case K_CONSTANT:     return 0.5f;
    case K_ALTERNATING:  return (((long) i) & 1) ? -0.5f : 0.5f;
    case K_SINE:         return 0.5f * (float) sin ( 2.0 * M_PI * 1000.0 * i / 44100.0 );
    case K_MULTITONE:    return 0.25f * (float) sin ( 2.0 * M_PI * 440.0 * i / 44100.0 )
                              + 0.25f * (float) sin ( 2.0 * M_PI * 3000.0 * i / 44100.0 );
    case K_LOWFREQ:      return 0.6f * (float) sin ( 2.0 * M_PI * 60.0 * i / 44100.0 );
    case K_HIGHFREQ:     return 0.6f * (float) sin ( 2.0 * M_PI * 12000.0 * i / 44100.0 );
    case K_NOISE:        return fval ( lcg ( sl ) );
    case K_TRANSIENT:
        if ( i >= 0.0 && i < 1.0 )       return 1.0f;
        if ( i >= 500.0 && i < 501.0 )   return -0.5f;
        if ( i >= 1000.0 && i < 1001.0 ) return 0.75f;
        return 0.0f;
    case K_STEREO_SAME:  return 0.4f * (float) sin ( 2.0 * M_PI * 700.0 * i / 44100.0 );
    case K_STEREO_DIFF:  return 0.4f * (float) sin ( 2.0 * M_PI * 700.0 * i / 44100.0 );
    case K_LEFT_ONLY:    return 0.5f * (float) sin ( 2.0 * M_PI * 900.0 * i / 44100.0 );
    case K_RIGHT_ONLY:   return 0.0f;
    case K_PHASE_INVERT: return 0.5f * (float) sin ( 2.0 * M_PI * 900.0 * i / 44100.0 );
    case K_RAMP:         return (float) ( ( ( (long) i ) % 512 ) - 256 ) / 512.0f;
    }
    return 0.0f;
}

static float
pcm_right ( int kind, double i, uint32_t* sr )
{
    switch ( kind ) {
    case K_SILENCE:
    case K_IMPULSE:
    case K_CONSTANT:
    case K_ALTERNATING:
    case K_SINE:
    case K_MULTITONE:
    case K_LOWFREQ:
    case K_HIGHFREQ:
    case K_TRANSIENT:
    case K_RAMP:
        return pcm_left ( kind, i, sr );
    case K_NOISE:
        return fval ( lcg ( sr ) );
    case K_STEREO_SAME:
        return 0.4f * (float) sin ( 2.0 * M_PI * 700.0 * i / 44100.0 );
    case K_STEREO_DIFF:
        return 0.4f * (float) sin ( 2.0 * M_PI * 1100.0 * i / 44100.0 );
    case K_LEFT_ONLY:
        return 0.0f;
    case K_RIGHT_ONLY:
        return 0.5f * (float) sin ( 2.0 * M_PI * 900.0 * i / 44100.0 );
    case K_PHASE_INVERT:
        return -0.5f * (float) sin ( 2.0 * M_PI * 900.0 * i / 44100.0 );
    }
    return 0.0f;
}

/* ------------------------------------------------------------------ */
/* output helpers                                                      */
/* ------------------------------------------------------------------ */

static void
put_f32 ( FILE* f, float v )
{
    uint32_t u;
    memcpy ( &u, &v, sizeof u );
    fwrite ( &u, sizeof u, 1, f );
}

static void
put_f32_n ( FILE* f, const float* v, int n )
{
    int i;
    for ( i = 0; i < n; i++ )
        put_f32 ( f, v [i] );
}

static void
put_u32 ( FILE* f, uint32_t v )
{
    fwrite ( &v, sizeof v, 1, f );
}

static void
put_hex_n ( FILE* f, const float* v, int n )
{
    int i;
    for ( i = 0; i < n; i++ ) {
        uint32_t u;
        memcpy ( &u, &v [i], sizeof u );
        fprintf ( f, "%08x\n", u );
    }
}

/* J.2: base arrays are f64; write each value as16 hex digits (IEEE-754
 * bit pattern, most-significant byte first for readability). */
static void
put_hex64_n ( FILE* f, const double* v, int n )
{
    int i;
    for ( i = 0; i < n; i++ ) {
        uint64_t u;
        memcpy ( &u, &v [i], sizeof u );
        fprintf ( f, "%016llx\n", (unsigned long long) u );
    }
}

/* ------------------------------------------------------------------ */
/* configuration                                                       */
/* ------------------------------------------------------------------ */

typedef struct {
    const char* name;
    float       qual;
    double      rate;
} config_t;

static const config_t CONFIGS [] = {
    /* The original seven oracle configurations (Phase 15F), unchanged. */
    { "q4-44100", 4.0f, 44100.0 },
    { "q5-44100", 5.0f, 44100.0 },
    { "q6-44100", 6.0f, 44100.0 },
    { "q7-44100", 7.0f, 44100.0 },
    { "q5-48000", 5.0f, 48000.0 },
    { "q5-37800", 5.0f, 37800.0 },
    { "q5-32000", 5.0f, 32000.0 },
    /* Encoder-parity slice J.1: the remaining integer quality x sample-rate
     * combinations accepted by the reference mpcenc (quality 0..=10 is
     * clipped/integer-accepted at all four SV8 rates; profile.c
     * SetQualityParams + psy_tab.c Init_Psychoakustiktabellen handle every
     * pair). Added so the Rust gate can cover the full 44-configuration
     * integer matrix with oracle-exact frozen tables. */
    { "q0-44100",  0.0f, 44100.0 },
    { "q0-48000",  0.0f, 48000.0 },
    { "q0-37800",  0.0f, 37800.0 },
    { "q0-32000",  0.0f, 32000.0 },
    { "q1-44100",  1.0f, 44100.0 },
    { "q1-48000",  1.0f, 48000.0 },
    { "q1-37800",  1.0f, 37800.0 },
    { "q1-32000",  1.0f, 32000.0 },
    { "q2-44100",  2.0f, 44100.0 },
    { "q2-48000",  2.0f, 48000.0 },
    { "q2-37800",  2.0f, 37800.0 },
    { "q2-32000",  2.0f, 32000.0 },
    { "q3-44100",  3.0f, 44100.0 },
    { "q3-48000",  3.0f, 48000.0 },
    { "q3-37800",  3.0f, 37800.0 },
    { "q3-32000",  3.0f, 32000.0 },
    { "q4-48000",  4.0f, 48000.0 },
    { "q4-37800",  4.0f, 37800.0 },
    { "q4-32000",  4.0f, 32000.0 },
    { "q6-48000",  6.0f, 48000.0 },
    { "q6-37800",  6.0f, 37800.0 },
    { "q6-32000",  6.0f, 32000.0 },
    { "q7-48000",  7.0f, 48000.0 },
    { "q7-37800",  7.0f, 37800.0 },
    { "q7-32000",  7.0f, 32000.0 },
    { "q8-44100",  8.0f, 44100.0 },
    { "q8-48000",  8.0f, 48000.0 },
    { "q8-37800",  8.0f, 37800.0 },
    { "q8-32000",  8.0f, 32000.0 },
    { "q9-44100",  9.0f, 44100.0 },
    { "q9-48000",  9.0f, 48000.0 },
    { "q9-37800",  9.0f, 37800.0 },
    { "q9-32000",  9.0f, 32000.0 },
    { "q10-44100", 10.0f, 44100.0 },
    { "q10-48000", 10.0f, 48000.0 },
    { "q10-37800", 10.0f, 37800.0 },
    { "q10-32000", 10.0f, 32000.0 },
};
#define NCONFIGS ((int) (sizeof CONFIGS / sizeof CONFIGS[0]))

/* Mirrors mpcenc's PsyModel lifecycle: Init_Psychoakustik() runs with
 * SampleFreq/BandWidth 0 (so the pre-echo/post-mask state is seeded from the
 * degenerate threshold), then the profile and real sample rate are applied and
 * Init_Psychoakustiktabellen() is called once more. */
static void
config_init ( PsyModel* m, const config_t* c )
{
    memset ( m, 0, sizeof *m );
    Init_Psychoakustik ( m );
    SetQualityParams ( m, c->qual );
    m->SampleFreq = (float) c->rate;
    Init_Psychoakustiktabellen ( m );
    mpc_psy_set_impl ( MPC_PSY_SCALAR );
}

/* ------------------------------------------------------------------ */
/* J.2: fractional-quality ATH base extraction and self-check.         */
/*                                                                     */
/* TheATH base for one (rate, EarModelFlag) pair is the value of       */
/* `tmp` inside psy_tab.c Ruhehoerschwelle AFTER the per-flag roll-off  */
/* line and BEFORE `mind(tmp, Ltq_max)` / `+= Ltq_offset - 23` /       */
/* POW10. Those remaining steps depend only on values derived from     */
/* the profile row (discrete flag, (int)Ltq_offset, (int)Ltq_max), so   */
/* freezing the base per (rate, flag) reproduces fftLtq for ANY        */
/* fractional quality by deterministic arithmetic. Proven by the J.2   */
/* experiment and enforced below by `selfcheck` (the production        */
/* equivalent of the experiment's72/72 reconstruction check).          */
/*                                                                     */
/* `ATHformula_Frank` and the switch/roll-off are verbatim copies of   */
/* psy_tab.c (the production functions are `static` and not linkable); */
/* `selfcheck` re-derives production output from the written dump so   */
/* any transcription error fails loudly.                               */

extern const int wl [PART_LONG];
extern const int wh [PART_LONG];

/* The fractional (quality, rate) oracle pairs (J.2 corpus). */
static const config_t FRACS [] = {
    /* Interiors x four rates. */
    { "q4.25-44100", 4.25f,  44100.0 },
    { "q4.25-48000", 4.25f,  48000.0 },
    { "q4.25-37800", 4.25f,  37800.0 },
    { "q4.25-32000", 4.25f,  32000.0 },
    { "q5.5-44100",  5.5f,   44100.0 },
    { "q5.5-48000",  5.5f,   48000.0 },
    { "q5.5-37800",  5.5f,   37800.0 },
    { "q5.5-32000",  5.5f,   32000.0 },
    { "q6.5-44100",  6.5f,   44100.0 },
    { "q6.5-48000",  6.5f,   48000.0 },
    { "q6.5-37800",  6.5f,   37800.0 },
    { "q6.5-32000",  6.5f,   32000.0 },
    { "q8.5-44100",  8.5f,   44100.0 },
    { "q8.5-48000",  8.5f,   48000.0 },
    { "q8.5-37800",  8.5f,   37800.0 },
    { "q8.5-32000",  8.5f,   32000.0 },
    /* Around integer boundaries (f32 parse-merge checks). */
    { "q4.9999999-44100", 4.9999999f, 44100.0 },
    { "q5.0000001-44100", 5.0000001f, 44100.0 },
    { "q5.9999999-44100", 5.9999999f, 44100.0 },
    { "q6.0000001-44100", 6.0000001f, 44100.0 },
    /* Arbitrary precision. */
    { "q4.2501-44100", 4.2501f,  44100.0 },
    { "q6.0000005-44100", 6.0000005f, 44100.0 },
    /* Just below the clip boundary:9.9999 != q10. */
    { "q9.9999-44100", 9.9999f, 44100.0 },
};
#define NFRACS ((int) (sizeof FRACS / sizeof FRACS[0]))

/* Verbatim copy of psy_tab.c ATHformula_Frank (static, not linkable). */
static float
ATHformula_Frank ( float freq )
{
    static short tab [] = {
        9669, 9669, 9626, 9512,  9353, 9113, 8882, 8676,
        8469, 8243, 7997, 7748,  7492, 7239, 7000, 6762,
        6529, 6302, 6084, 5900,  5717, 5534, 5351, 5167,
        5004, 4812, 4638, 4466,  4310, 4173, 4050, 3922,
        3723, 3577, 3451, 3281,  3132, 3036, 2902, 2760,
        2658, 2591, 2441, 2301,  2212, 2125, 2018, 1900,
        1770, 1682, 1594, 1512,  1430, 1341, 1260, 1198,
        1136, 1057,  998,  943,   887,  846,  744,  712,
         693,  668,  637,  606,   580,  555,  529,  502,
         475,  448,  422,  398,   375,  351,  327,  322,
         312,  301,  291,  268,   246,  215,  182,  146,
         107,   61,   13,  -35,   -96, -156, -179, -235,
        -295, -350, -401, -421,  -446, -499, -532, -535,
        -513, -476, -431, -313,  -179,    8,  203,  403,
         580,  736,  881, 1022,  1154, 1251, 1348, 1421,
        1479, 1399, 1285, 1193,  1287, 1519, 1914, 2369,
        3352, 4352, 5352, 6352,  7352, 8352, 9352, 9999,
        9999, 9999, 9999, 9999,
    };
    double    freq_log;
    unsigned  index;

    if ( freq <    10. ) freq =    10.;
    if ( freq > 29853. ) freq = 29853.;

    freq_log = 40. * log10 (0.1 * freq);
    index    = (unsigned) freq_log;
    return 0.01 * (tab [index] * (1 + index - freq_log) + tab [index+1] * (freq_log - index));
}

/* Verbatim psy_tab.c Ruhehoerschwelle switch + roll-off, stopping AFTER
 * `tmp -= f * f * (int)(EarModelFlag % 100 - 50) * 0.0015` and BEFORE
 * `mind(tmp, Ltq_max)`. `Ltq_max` is only referenced by case2
 * (flag/100 == 2, unreachable for the profile flags300..599); it is
 * passed INT_MAX and `selfcheck` would expose any effect. */
static void
ruhe_base ( unsigned EarModelFlag, float SampleFreq, int Ltq_max, double out [512] )
{
    int     n;
    float   f;
    double  tmp;

    for ( n = 0; n < 512; n++ ) {
        f = (float) ( (n+1) * (float)(SampleFreq / 2000.) / 512 );

        switch ( EarModelFlag / 100 ) {
        case 0:
            tmp  = 3.64*pow (f,-0.8) -  6.5*exp (-0.6*(f-3.3)*(f-3.3)) + 0.001*pow (f, 4.0);
            break;
        default:
        case 1:
            tmp  = 3.00*pow (f,-0.8) -  5.0*exp (-0.1*(f-3.0)*(f-3.0)) + 0.0000015022693846297*pow (f, 6.0) + 10.*exp (-(f-0.1)*(f-0.1));
            break;
        case 2:
            tmp  = 9.00*pow (f,-0.5) - 15.0*exp (-0.1*(f-4.0)*(f-4.0)) + 0.0341796875*pow (f, 2.5)          + 15.*exp (-(f-0.1)*(f-0.1)) - 18;
            tmp  = mind ( tmp, Ltq_max - 18 );
            break;
        case 3:
            tmp  = ATHformula_Frank ( 1.e3 * f );
            break;
        case 4:
            tmp  = ATHformula_Frank ( 1.e3 * f );
            if ( f > 4.8 ) {
                tmp += 3.00*pow (f,-0.8) -  5.0*exp (-0.1*(f-3.0)*(f-3.0)) + 0.0000015022693846297*pow (f, 6.0) + 10.*exp (-(f-0.1)*(f-0.1));
                tmp *= 0.5 ;
            }
            break;
        case 5:
            tmp  = ATHformula_Frank ( 1.e3 * f );
            if ( f > 4.8 ) {
                tmp = 3.00*pow (f,-0.8) -  5.0*exp (-0.1*(f-3.0)*(f-3.0)) + 0.0000015022693846297*pow (f, 6.0) + 10.*exp (-(f-0.1)*(f-0.1));
            }
            break;
        }

        tmp -= f * f * (int)(EarModelFlag % 100 - 50) * 0.0015;   /* BASE */
        out [n] = tmp;
    }
}

/* Tail of psy_tab.c Ruhehoerschwelle: clamp, offset, POW10, partitions. */
static void
ruhe_tail ( const double base [512], int Ltq_offset, int Ltq_max,
            float fft [512], float part [PART_LONG], float inv [PART_LONG] )
{
    int   n, k;
    float erg;
    float absLtq [512];

    for ( n = 0; n < 512; n++ ) {
        double tmp = base [n];
        tmp        = mind ( tmp, Ltq_max );
        tmp       += Ltq_offset - 23;
        fft [n] = absLtq [n] = POW10 ( 0.1 * tmp );
    }
    for ( n = 0; n < PART_LONG; n++ ) {
        erg = 1.e20f;
        for ( k = wl [n]; k <= wh [n]; k++ )
            erg = minf ( erg, absLtq [k]);
        part [n] = erg;
        inv  [n] = 1.f / part [n];
    }
}

/* ------------------------------------------------------------------ */
/* tables mode                                                         */
/* ------------------------------------------------------------------ */

extern float  MinVal   [PART_LONG];
extern float  Loudness [PART_LONG];
extern float  SPRD     [PART_LONG][PART_LONG];
extern float  O_MAX;
extern float  O_MIN;
extern float  FAC1;
extern float  FAC2;
extern float  partLtq  [PART_LONG];
extern float  invLtq   [PART_LONG];
extern float  fftLtq   [512];
extern float  w        [4096];
extern float  Hann_256 [256];
extern float  Hann_1024[1024];
extern float  Hann_1600[1600];
extern float  tabcos   [][2];
extern float  tabatan2 [][2];
extern int    ip      [4096];
void   Generate_FFT_Tables ( const int, int*, float* );
void   rdft                ( const int, float*, int*, float* );
void   Cepstrum2048        ( float*, const int );
void   Init_FastMath       ( void );

/* Writes the committed `psy_<name>.txt` dump format for one initialised
 * model. Shared by `tables` mode and the J.2 `frac` mode so both emit
 * byte-identical section layouts. */
static int
write_psy_dump ( const char* path, const PsyModel* m )
{
    float scalars [4];
    FILE* f = fopen ( path, "w" );
    if ( !f ) { perror ( path ); return 1; }
    fprintf ( f, "max_band %d\n", m->Max_Band );
    fprintf ( f, "fftLtq 512\n" );     put_hex_n ( f, fftLtq, 512 );
    fprintf ( f, "partLtq 57\n" );     put_hex_n ( f, partLtq, PART_LONG );
    fprintf ( f, "invLtq 57\n" );      put_hex_n ( f, invLtq, PART_LONG );
    fprintf ( f, "MinVal 57\n" );      put_hex_n ( f, MinVal, PART_LONG );
    fprintf ( f, "Loudness 57\n" );    put_hex_n ( f, Loudness, PART_LONG );
    fprintf ( f, "SPRD 3249\n" );      put_hex_n ( f, (const float*) SPRD, PART_LONG * PART_LONG );
    scalars [0] = O_MAX; scalars [1] = O_MIN; scalars [2] = FAC1; scalars [3] = FAC2;
    fprintf ( f, "scalars 4\n" );      put_hex_n ( f, scalars, 4 );
    fclose ( f );
    return 0;
}

static int
mode_tables ( const char* outdir )
{
    int c;
    char path [4096];
    FILE* f;
    PsyModel boot;

    /* `Init_FFT` (via `Init_Psychoakustik`) generates the Hann windows and the
     * FFT twiddle table; it must run before the kernels are dumped. */
    config_init ( &boot, &CONFIGS [0] );
    Init_FastMath ();

    /* Rate-independent frozen kernels. */
    snprintf ( path, sizeof path, "%s/kernels.txt", outdir );
    f = fopen ( path, "w" );
    if ( !f ) { perror ( path ); return 1; }
    fprintf ( f, "w 4096\n" );         put_hex_n ( f, w, 4096 );
    fprintf ( f, "Hann_256 256\n" );   put_hex_n ( f, Hann_256, 256 );
    fprintf ( f, "Hann_1024 1024\n" ); put_hex_n ( f, Hann_1024, 1024 );
    fprintf ( f, "Hann_1600 1600\n" ); put_hex_n ( f, Hann_1600, 1600 );
    fprintf ( f, "tabcos 3330\n" );    put_hex_n ( f, (const float*) tabcos, 3330 );
    fprintf ( f, "tabatan2 258\n" );   put_hex_n ( f, (const float*) tabatan2, 258 );
    fclose ( f );

    /* Per-configuration psychoacoustic tables. */
    for ( c = 0; c < NCONFIGS; c++ ) {
        PsyModel m;
        config_init ( &m, &CONFIGS [c] );
        snprintf ( path, sizeof path, "%s/psy_%s.txt", outdir, CONFIGS [c].name );
        if ( write_psy_dump ( path, &m ) ) return 1;
    }
    printf ( "tables: %d configs + kernels\n", NCONFIGS );
    return 0;
}

/* ------------------------------------------------------------------ */
/* fft mode                                                            */
/* ------------------------------------------------------------------ */

static void
gen_signal ( float* x, int n, int kind )
{
    int i;
    uint32_t s = 0x12345678u;
    for ( i = 0; i < n; i++ ) {
        double v;
        switch ( kind ) {
        case 0: v = 0.0; break;
        case 1: v = ( i == n / 3 ) ? 1.0 : 0.0; break;
        case 2: v = 0.5; break;
        case 3: v = ( i & 1 ) ? -0.5 : 0.5; break;
        default: v = fval ( lcg ( &s ) ); break;
        }
        x [i] = (float) v;
    }
}

static int
mode_fft ( const char* outdir )
{
    static const int SIZES [] = { 256, 1024, 2048 };
    int si, kind;
    char path [4096];
    FILE* f;
    PsyModel boot;

    memset ( &boot, 0, sizeof boot );
    Init_Psychoakustik ( &boot );
    mpc_psy_set_impl ( MPC_PSY_SCALAR );
    Init_FastMath ();
    Generate_FFT_Tables ( 2048, ip, w );

    for ( si = 0; si < 3; si++ ) {
        int n = SIZES [si];
        for ( kind = 0; kind < 5; kind++ ) {
            float x [2048];
            float erg [1024];
            float phs [1024];
            gen_signal ( x, n, kind );
            snprintf ( path, sizeof path, "%s/powspec%d_k%d.txt", outdir, n, kind );
            f = fopen ( path, "w" );
            if ( !f ) { perror ( path ); return 1; }
            if ( n == 256 )       PowSpec256 ( x, erg );
            else if ( n == 1024 ) PowSpec1024 ( x, erg );
            else                  PowSpec2048 ( x, erg );
            fprintf ( f, "erg %d\n", n / 2 );
            put_hex_n ( f, erg, n / 2 );
            fclose ( f );
        }
    }

    for ( kind = 0; kind < 5; kind++ ) {
        float x [1024];
        float erg [512];
        float phs [512];
        gen_signal ( x, 1024, kind );
        snprintf ( path, sizeof path, "%s/polar1024_k%d.txt", outdir, kind );
        f = fopen ( path, "w" );
        if ( !f ) { perror ( path ); return 1; }
        PolarSpec1024 ( x, erg, phs );
        fprintf ( f, "erg 512\n" ); put_hex_n ( f, erg, 512 );
        fprintf ( f, "phs 512\n" ); put_hex_n ( f, phs, 512 );
        fclose ( f );
    }

    /* Cepstrum2048 on a deterministic log-windowed spectrum. */
    {
        float cep [4096];
        int i, j;
        uint32_t s = 0x0BADF00Du;
        for ( i = 0; i < 512; i++ )
            cep [i] = fval ( lcg ( &s ) ) * 10.0f;
        for ( i = 512; i < 1025; i++ )
            cep [i] = 0.0f;
        snprintf ( path, sizeof path, "%s/cepstrum2048.txt", outdir );
        f = fopen ( path, "w" );
        if ( !f ) { perror ( path ); return 1; }
        for ( i = 0, j = 1024; i < 1024; ++i, --j )
            cep [1024 + j] = cep [i];
        fprintf ( f, "cep_in 2048\n" ); put_hex_n ( f, cep, 2048 );
        rdft ( 2048, cep, ip, w );
        fprintf ( f, "cep_raw 2048\n" ); put_hex_n ( f, cep, 2048 );
        for ( i = 0; i < MAX_ANALYZED_IDX + 1; ++i )
            cep [i] = cep [i * 2] * (float) (0.9888 / 2048);
        fprintf ( f, "cep 901\n" ); put_hex_n ( f, cep, 901 );
        fclose ( f );
    }

    /* Raw forward rdft on an arbitrary deterministic input. */
    {
        float a [2048];
        int i;
        uint32_t s = 0xDEADBEEFu;
        snprintf ( path, sizeof path, "%s/rdft2048.txt", outdir );
        f = fopen ( path, "w" );
        if ( !f ) { perror ( path ); return 1; }
        for ( i = 0; i < 2048; i++ )
            a [i] = fval ( lcg ( &s ) ) * 10.0f;
        fprintf ( f, "in 2048\n" ); put_hex_n ( f, a, 2048 );
        rdft ( 2048, a, ip, w );
        fprintf ( f, "out 2048\n" ); put_hex_n ( f, a, 2048 );
        fclose ( f );
    }
    printf ( "fft: primitives written\n" );
    return 0;
}

/* ------------------------------------------------------------------ */
/* model mode                                                          */
/* ------------------------------------------------------------------ */

typedef struct {
    const char* name;
    int         kind;
    int         frames;
} pcmcase_t;

static const pcmcase_t PCMCASES [] = {
    { "silence",     K_SILENCE,     3 },
    { "impulse",     K_IMPULSE,     3 },
    { "constant",    K_CONSTANT,    3 },
    { "alternating", K_ALTERNATING, 3 },
    { "sine",        K_SINE,        3 },
    { "multitone",   K_MULTITONE,   3 },
    { "lowfreq",     K_LOWFREQ,     3 },
    { "highfreq",    K_HIGHFREQ,    3 },
    { "noise",       K_NOISE,       3 },
    { "transient",   K_TRANSIENT,   3 },
    { "stereo_same", K_STEREO_SAME, 3 },
    { "stereo_diff", K_STEREO_DIFF, 3 },
    { "left_only",   K_LEFT_ONLY,   3 },
    { "right_only",  K_RIGHT_ONLY,  3 },
    { "phase_invert",K_PHASE_INVERT,3 },
    { "ramp",        K_RAMP,        3 },
};
#define NPCM ((int) (sizeof PCMCASES / sizeof PCMCASES[0]))

static void
fill_float ( float* dst, float value, int count )
{
    int i;
    for ( i = 0; i < count; i++ )
        dst [i] = value;
}

static int
mode_model ( const char* outdir )
{
    char manifest [4096];
    FILE* mf;
    int c, pc;

    snprintf ( manifest, sizeof manifest, "%s/manifest.txt", outdir );
    mf = fopen ( manifest, "w" );
    if ( !mf ) { perror ( manifest ); return 1; }
    fprintf ( mf, "# psy model oracle v1\n" );
    fprintf ( mf, "# record layout per frame (all f32le): "
                 "SMR_L(32) SMR_R(32) SMR_M(32) SMR_S(32) "
                 "SMRr_L(32) SMRr_R(32) SMRr_M(32) SMRr_S(32) "
                 "TransientL(19) TransientR(19) Transient(32) ANSspec_L(512) ANSspec_R(512) "
                 "ANSspec_M(512) ANSspec_S(512)\n" );
    fprintf ( mf, "# case config qual rate frames max_band ms_mode cvd ns_order min_smr bytes\n" );

    Init_FastMath ();

    for ( c = 0; c < NCONFIGS; c++ ) {
        for ( pc = 0; pc < NPCM; pc++ ) {
            const pcmcase_t* k = &PCMCASES [pc];
            PsyModel m;
            PCMDataTyp main;
            int transientL [PART_SHORT];
            int transientR [PART_SHORT];
            int transient [32];
            float* gL;
            float* gR;
            uint32_t sl = 0x12345678u, sr = 0x9ABCDEF0u;
            long total = (long) k->frames * BLOCK + BLOCK;
            long i;
            int f;
            char path [4096];
            FILE* out;

            gL = malloc ( (size_t) total * sizeof (float) );
            gR = malloc ( (size_t) total * sizeof (float) );
            for ( i = 0; i < total; i++ ) {
                gL [i] = pcm_left ( k->kind, (double) i, &sl );
                gR [i] = pcm_right ( k->kind, (double) i, &sr );
            }

            config_init ( &m, &CONFIGS [c] );
            memset ( &main, 0, sizeof main );

            /* Seed the analysis buffer exactly like mpcenc's first read. */
            for ( i = 0; i < BLOCK; i++ ) {
                main.L [CENTER + i] = gL [i];
                main.R [CENTER + i] = gR [i];
                main.M [CENTER + i] = (main.L [CENTER + i] + main.R [CENTER + i]) * 0.5f;
                main.S [CENTER + i] = (main.L [CENTER + i] - main.R [CENTER + i]) * 0.5f;
            }
            fill_float ( main.L, main.L [CENTER], CENTER );
            fill_float ( main.R, main.R [CENTER], CENTER );
            fill_float ( main.M, main.M [CENTER], CENTER );
            fill_float ( main.S, main.S [CENTER], CENTER );

            snprintf ( path, sizeof path, "%s/%s_%s.f32le", outdir, CONFIGS [c].name, k->name );
            out = fopen ( path, "wb" );
            if ( !out ) { perror ( path ); return 1; }

            for ( f = 0; f < k->frames; f++ ) {
                SMRTyp raw;
                SMRTyp raised;
                raw = Psychoakustisches_Modell ( &m, 31, &main, transientL, transientR );
                raised = raw;
                if ( m.minSMR > 0 )
                    RaiseSMR ( &m, m.Max_Band, &raised );
                TransientenCalc ( transient, transientL, transientR );

                put_f32_n ( out, raw.L, 32 );
                put_f32_n ( out, raw.R, 32 );
                put_f32_n ( out, raw.M, 32 );
                put_f32_n ( out, raw.S, 32 );
                put_f32_n ( out, raised.L, 32 );
                put_f32_n ( out, raised.R, 32 );
                put_f32_n ( out, raised.M, 32 );
                put_f32_n ( out, raised.S, 32 );
                { int t; float tv [19];
                  for ( t = 0; t < PART_SHORT; t++ ) tv [t] = (float) transientL [t];
                  put_f32_n ( out, tv, PART_SHORT ); }
                { int t; float tv [19];
                  for ( t = 0; t < PART_SHORT; t++ ) tv [t] = (float) transientR [t];
                  put_f32_n ( out, tv, PART_SHORT ); }
                { int t; float tv [32];
                  for ( t = 0; t < 32; t++ ) tv [t] = (float) transient [t];
                  put_f32_n ( out, tv, 32 ); }
                put_f32_n ( out, m.state.ANSspec_L, MAX_ANS_LINES );
                put_f32_n ( out, m.state.ANSspec_R, MAX_ANS_LINES );
                put_f32_n ( out, m.state.ANSspec_M, MAX_ANS_LINES );
                put_f32_n ( out, m.state.ANSspec_S, MAX_ANS_LINES );

                /* advance the analysis buffer like mpcenc */
                memmove ( main.L, main.L + BLOCK, CENTER * sizeof (float) );
                memmove ( main.R, main.R + BLOCK, CENTER * sizeof (float) );
                memmove ( main.M, main.M + BLOCK, CENTER * sizeof (float) );
                memmove ( main.S, main.S + BLOCK, CENTER * sizeof (float) );
                for ( i = 0; i < BLOCK; i++ ) {
                    long gi = (long) (f + 1) * BLOCK + i;
                    float l = ( gi < total ) ? gL [gi] : 0.0f;
                    float r = ( gi < total ) ? gR [gi] : 0.0f;
                    main.L [CENTER + i] = l;
                    main.R [CENTER + i] = r;
                    main.M [CENTER + i] = (l + r) * 0.5f;
                    main.S [CENTER + i] = (l - r) * 0.5f;
                }
                fill_float ( main.L, main.L [CENTER], CENTER );
                fill_float ( main.R, main.R [CENTER], CENTER );
                fill_float ( main.M, main.M [CENTER], CENTER );
                fill_float ( main.S, main.S [CENTER], CENTER );
            }
            fclose ( out );

            fprintf ( mf, "%s_%s %s %g %g %d %d %d %d %d %g %ld\n",
                      CONFIGS [c].name, k->name, CONFIGS [c].name,
                      CONFIGS [c].qual, CONFIGS [c].rate, k->frames, m.Max_Band,
                      m.MS_Channelmode, m.CVD_used, m.NS_Order, m.minSMR,
                      (long) k->frames * 2374L * 4L );
            free ( gL );
            free ( gR );
        }
    }
    fclose ( mf );
    printf ( "model: %d configs x %d cases\n", NCONFIGS, NPCM );
    return 0;
}

/* ------------------------------------------------------------------ */
/* math mode (level 1 primitives)                                      */
/* ------------------------------------------------------------------ */

static int
mode_math ( const char* outdir )
{
    char path [4096];
    FILE* f;
    int i;
    uint32_t s = 0x13579BDFu;
    float in [2048];
    float yv [2048];

    for ( i = 0; i < 2048; i++ ) {
        uint32_t r = lcg ( &s );
        in [i] = (float) ( (int) (r >> 8) - 8388608 ) * (13.0f / 8388608.0f);
    }
    for ( i = 0; i < 2048; i++ )
        yv [i] = in [(i * 7 + 13) % 2048];

    Init_FastMath ();

    snprintf ( path, sizeof path, "%s/math.txt", outdir );
    f = fopen ( path, "w" );
    if ( !f ) { perror ( path ); return 1; }

    fprintf ( f, "in 2048\n" );
    for ( i = 0; i < 2048; i++ ) { uint32_t u; memcpy ( &u, &in [i], 4 ); fprintf ( f, "%08x\n", u ); }
    fprintf ( f, "my_cos 2048\n" );
    for ( i = 0; i < 2048; i++ ) { float v = my_cos ( in [i] ); put_hex_n ( f, &v, 1 ); }
    fprintf ( f, "my_atan2 2048\n" );
    for ( i = 0; i < 2048; i++ ) { float v = my_atan2 ( in [i], yv [i] ); put_hex_n ( f, &v, 1 ); }
    fprintf ( f, "my_ifloor 2048\n" );
    for ( i = 0; i < 2048; i++ ) fprintf ( f, "%08x\n", (uint32_t) my_ifloor ( in [i] ) );
    fprintf ( f, "lrintf 2048\n" );
    for ( i = 0; i < 2048; i++ ) fprintf ( f, "%08x\n", (uint32_t) mpc_lrintf ( in [i] ) );
    fprintf ( f, "pow10 2048\n" );
    for ( i = 0; i < 2048; i++ ) { float v = POW10 ( in [i] ); put_hex_n ( f, &v, 1 ); }
    fprintf ( f, "pow 2048\n" );
    for ( i = 0; i < 2048; i++ ) { float v = POW ( fabsf ( in [i] ) + 0.0001f, yv [i] ); put_hex_n ( f, &v, 1 ); }
    fprintf ( f, "log10 2048\n" );
    for ( i = 0; i < 2048; i++ ) { float v = LOG10 ( fabsf ( in [i] ) + 0.0001f ); put_hex_n ( f, &v, 1 ); }
    fprintf ( f, "sqrtf 2048\n" );
    for ( i = 0; i < 2048; i++ ) { float v = SQRTF ( fabsf ( in [i] ) ); put_hex_n ( f, &v, 1 ); }
    fclose ( f );
    printf ( "math: primitives written\n" );
    return 0;
}

/* ------------------------------------------------------------------ */
/* ms mode                                                             */
/* ------------------------------------------------------------------ */

static int
mode_ms ( const char* outdir )
{
    char path [4096];
    FILE* f;
    int c, k;

    for ( c = 0; c < NCONFIGS; c++ ) {
        PsyModel m;
        config_init ( &m, &CONFIGS [c] );
        for ( k = 0; k < 8; k++ ) {
            SMRTyp smr;
            SubbandFloatTyp x [32];
            unsigned char ms [32];
            int b, n;
            uint32_t s = 0xC0FFEE11u + (uint32_t) k * 7919u;
            for ( b = 0; b < 32; b++ ) {
                for ( n = 0; n < FSUB; n++ ) {
                    x [b].L [n] = fval ( lcg ( &s ) );
                    x [b].R [n] = fval ( lcg ( &s ) );
                }
                smr.L [b] = fabsf ( fval ( lcg ( &s ) ) ) * 2000.0f;
                smr.R [b] = fabsf ( fval ( lcg ( &s ) ) ) * 2000.0f;
                smr.M [b] = fabsf ( fval ( lcg ( &s ) ) ) * 2000.0f;
                smr.S [b] = fabsf ( fval ( lcg ( &s ) ) ) * 2000.0f;
            }
            MS_LR_Entscheidung ( m.Max_Band, ms, &smr, x );
            snprintf ( path, sizeof path, "%s/ms_%s_k%d.txt", outdir, CONFIGS [c].name, k );
            f = fopen ( path, "w" );
            if ( !f ) { perror ( path ); return 1; }
            { float mv [32]; int b2; for ( b2 = 0; b2 < 32; b2++ ) mv [b2] = (float) ms [b2];
              fprintf ( f, "ms 32\n" ); put_hex_n ( f, mv, 32 ); }
            fprintf ( f, "smr_L 32\n" ); put_hex_n ( f, smr.L, 32 );
            fprintf ( f, "smr_R 32\n" ); put_hex_n ( f, smr.R, 32 );
            fprintf ( f, "smr_M 32\n" ); put_hex_n ( f, smr.M, 32 );
            fprintf ( f, "smr_S 32\n" ); put_hex_n ( f, smr.S, 32 );
            { float xv [36]; int b2;
              fprintf ( f, "x_L 1152\n" );
              for ( b2 = 0; b2 < 32; b2++ ) { for ( n = 0; n < FSUB; n++ ) xv [n] = x [b2].L [n]; put_hex_n ( f, xv, FSUB ); }
              fprintf ( f, "x_R 1152\n" );
              for ( b2 = 0; b2 < 32; b2++ ) { for ( n = 0; n < FSUB; n++ ) xv [n] = x [b2].R [n]; put_hex_n ( f, xv, FSUB ); } }
            fclose ( f );
        }
    }
    printf ( "ms: %d configs x 8 cases\n", NCONFIGS );
    return 0;
}

/* ------------------------------------------------------------------ */
/* J.2 modes: bases, selfcheck, frac                                   */
/* ------------------------------------------------------------------ */

static const double J2_RATES [4] = { 44100.0, 48000.0, 37800.0, 32000.0 };

static uint32_t
j2_bits ( float x )
{
    uint32_t u;
    memcpy ( &u, &x, sizeof u );
    return u;
}

/* bases: the40 f64 ATH base arrays -> outdir/psy_bases.txt. */
static int
mode_bases ( const char* outdir )
{
    char path [4096];
    FILE* f;
    unsigned flags [16];
    int nf = 0, q, i, r;

    /* Derive the distinct EarModelFlags from the profile rows themselves
     * (no hand-maintained list): SetQualityParams selects the flag
     * discretely from the integer quality row. */
    for ( q = 0; q <= 10; q++ ) {
        PsyModel m;
        memset ( &m, 0, sizeof m );
        Init_Psychoakustik ( &m );
        SetQualityParams ( &m, (float) q );
        for ( i = 0; i < nf; i++ )
            if ( flags [i] == m.EarModelFlag ) break;
        if ( i == nf ) flags [nf++] = m.EarModelFlag;
    }
    if ( nf != 10 ) {
        fprintf ( stderr, "bases: expected10 profile EarModelFlags, got %d\n", nf );
        return 1;
    }

    snprintf ( path, sizeof path, "%s/psy_bases.txt", outdir );
    f = fopen ( path, "w" );
    if ( !f ) { perror ( path ); return 1; }
    fprintf ( f, "# psy_bases.txt - ATH base arrays (J.2); generated by\n" );
    fprintf ( f, "# `extract_psy_oracle bases <outdir>` from the pinned scalar C reference\n" );
    fprintf ( f, "# (-O0 -ffp-contract=off -DFAST_MATH -DCVD_FASTLOG, Apple/libm oracle).\n" );
    fprintf ( f, "# Each value is `tmp` (f64) inside psy_tab.c Ruhehoerschwelle AFTER the\n" );
    fprintf ( f, "# EarModelFlag roll-off subtraction and BEFORE mind(tmp, Ltq_max), written\n" );
    fprintf ( f, "# as16-hex-digit IEEE-754 bit patterns.4 rates x %d flags x 512 values.\n", nf );
    for ( r = 0; r < 4; r++ ) {
        fprintf ( f, "rate %d\n", (int) J2_RATES [r] );
        for ( i = 0; i < nf; i++ ) {
            double base [512];
            ruhe_base ( flags [i], (float) J2_RATES [r], INT_MAX, base );
            fprintf ( f, "flag %u\n", flags [i] );
            put_hex64_n ( f, base, 512 );
        }
    }
    fclose ( f );
    printf ( "bases:4 rates x %d flags x512 f64 -> %s\n", nf, path );
    return 0;
}

#define NBASE_MAX 64
typedef struct {
    double   rate;
    unsigned flag;
    double   v [512];
} base_entry;

/* Parses psy_bases.txt back into memory (the exact artifact the Rust
 * transcription reads, so self-check covers parse + tail + compare). */
static int
load_bases ( const char* outdir, base_entry* out, int* n_out )
{
    char path [4096], line [128];
    FILE* f;
    double cur_rate = -1.0;
    int have_rate = 0, n = 0;

    snprintf ( path, sizeof path, "%s/psy_bases.txt", outdir );
    f = fopen ( path, "r" );
    if ( !f ) { perror ( path ); return -1; }
    while ( fgets ( line, sizeof line, f ) ) {
        if ( line [0] == '#' || line [0] == '\n' ) continue;
        if ( strncmp ( line, "rate ", 5 ) == 0 ) {
            cur_rate = atof ( line + 5 );
            have_rate = 1;
        } else if ( strncmp ( line, "flag ", 5 ) == 0 ) {
            int k;
            if ( !have_rate || n >= NBASE_MAX ) { fclose ( f ); return -1; }
            out [n].rate = cur_rate;
            out [n].flag = (unsigned) strtoul ( line + 5, NULL, 10 );
            for ( k = 0; k < 512; k++ ) {
                uint64_t u;
                if ( !fgets ( line, sizeof line, f ) ) { fclose ( f ); return -1; }
                u = strtoull ( line, NULL, 16 );
                memcpy ( &out [n].v [k], &u, 8 );
            }
            n++;
        } else {
            fclose ( f );
            return -1;
        }
    }
    fclose ( f );
    *n_out = n;
    return 0;
}

/* selfcheck: for all44 integer configs plus the23 J.2 fractional oracle
 * pairs, reconstruct fftLtq/partLtq/invLtq from the WRITTEN base dump and
 * require bit equality with the live production Init_Psychoakustiktabellen
 * output (the production equivalent of the experiment's72/72 check; here
 *67 configs because the committed fractional corpus has23 pairs). */
static int
mode_selfcheck ( const char* outdir )
{
    base_entry bases [NBASE_MAX];
    int nb = 0, c, ok = 0, bad = 0;

    if ( load_bases ( outdir, bases, &nb ) != 0 ) {
        fprintf ( stderr, "selfcheck: cannot load psy_bases.txt (run `bases` first)\n" );
        return 1;
    }
    printf ( "selfcheck: loaded %d base arrays\n", nb );

    for ( c = 0; c < NCONFIGS + NFRACS; c++ ) {
        config_t cfg;
        PsyModel m;
        const double* base = NULL;
        float fft [512], part [PART_LONG], inv [PART_LONG];
        int n, diffs = 0;

        if ( c < NCONFIGS ) cfg = CONFIGS [c];
        else                cfg = FRACS [c - NCONFIGS];
        config_init ( &m, &cfg );

        for ( n = 0; n < nb; n++ )
            if ( bases [n].rate == (double) m.SampleFreq
              && bases [n].flag == m.EarModelFlag ) {
                base = bases [n].v;
                break;
            }
        if ( !base ) {
            fprintf ( stderr, "selfcheck FAIL %s: no base for rate %g flag %u\n",
                      cfg.name, cfg.rate, m.EarModelFlag );
            bad++;
            continue;
        }
        ruhe_tail ( base, (int) m.Ltq_offset, (int) m.Ltq_max, fft, part, inv );

        for ( n = 0; n < 512; n++ )
            if ( j2_bits ( fft [n] ) != j2_bits ( fftLtq [n] ) ) {
                if ( diffs == 0 )
                    fprintf ( stderr, "selfcheck FAIL %s fftLtq[%d]: prod=%08x mine=%08x\n",
                              cfg.name, n, j2_bits ( fftLtq [n] ), j2_bits ( fft [n] ) );
                diffs++;
            }
        for ( n = 0; n < PART_LONG; n++ )
            if ( j2_bits ( part [n] ) != j2_bits ( partLtq [n] ) ) diffs++;
        for ( n = 0; n < PART_LONG; n++ )
            if ( j2_bits ( inv [n] ) != j2_bits ( invLtq [n] ) ) diffs++;

        if ( diffs ) bad++;
        else         ok++;
    }
    printf ( "selfcheck: %d/%d configs reconstruct fft/part/inv bit-exactly (%d failed)\n",
             ok, NCONFIGS + NFRACS, bad );
    return bad ? 1 : 0;
}

/* Prints the `params` line (same key layout as the J.2 experiment) so the
 * committed fractional params oracle can be captured verbatim. */
static void
print_params_line ( const char* qin, float qual, const PsyModel* m )
{
    float qc = clip ( qual, 0., 10. );
    printf ( "params qin=%s qf_bits=%08x int_part=%d MainQual=%d FullQual=%.9g "
             "tmn=%08x nmt=%08x bw=%08x pns=%08x shortthr=%08x transdet=%08x "
             "varltq=%08x off=%08x off_i=%d lmax=%08x lmax_i=%d ear=%u "
             "minval_choice=%d minsmr=%08x tmpmask=%d cvd=%d ms=%d ns=%u comb=%d\n",
             qin, j2_bits ( qual ), (int) qc, m->MainQual, m->FullQual,
             j2_bits ( m->TMN ), j2_bits ( m->NMT ), j2_bits ( m->BandWidth ),
             j2_bits ( m->PNS ), j2_bits ( m->ShortThr ), j2_bits ( m->TransDetect ),
             j2_bits ( m->varLtq ), j2_bits ( m->Ltq_offset ), (int) m->Ltq_offset,
             j2_bits ( m->Ltq_max ), (int) m->Ltq_max,
             m->EarModelFlag, m->MinValChoice, j2_bits ( m->minSMR ),
             (int) m->tmpMask_used, (int) m->CVD_used, (int) m->MS_Channelmode,
             (unsigned) m->NS_Order, (int) m->CombPenalities );
}

/* frac: one fractional (quality, rate) dump + params line on stdout. */
static int
mode_frac ( const char* qstr, const char* rstr, const char* outdir )
{
    config_t c;
    PsyModel m;
    char path [4096];
    float qual = (float) atof ( qstr );

    c.name = "-";
    c.qual = qual;
    c.rate = atof ( rstr );
    config_init ( &m, &c );
    snprintf ( path, sizeof path, "%s/psy_q%s-%s.txt", outdir, qstr, rstr );
    if ( write_psy_dump ( path, &m ) ) return 1;
    print_params_line ( qstr, qual, &m );
    fprintf ( stderr, "frac: wrote %s\n", path );
    return 0;
}

/* ------------------------------------------------------------------ */
/* main                                                                */
/* ------------------------------------------------------------------ */

int
main ( int argc, char** argv )
{
    const char* mode;
    const char* outdir;

    if ( argc < 3 ) {
        fprintf ( stderr, "usage: %s tables|fft|model|ms|bases|selfcheck <outdir> "
                          "| frac <qual> <rate> <outdir>\n", argv [0] );
        return 2;
    }
    mode = argv [1];
    outdir = argv [2];

    if ( strcmp ( mode, "tables" ) == 0 ) return mode_tables ( outdir );
    if ( strcmp ( mode, "fft" ) == 0 )    return mode_fft ( outdir );
    if ( strcmp ( mode, "math" ) == 0 )   return mode_math ( outdir );
    if ( strcmp ( mode, "model" ) == 0 )  return mode_model ( outdir );
    if ( strcmp ( mode, "ms" ) == 0 )     return mode_ms ( outdir );
    if ( strcmp ( mode, "bases" ) == 0 )     return mode_bases ( outdir );
    if ( strcmp ( mode, "selfcheck" ) == 0 ) return mode_selfcheck ( outdir );
    if ( strcmp ( mode, "frac" ) == 0 ) {
        if ( argc != 5 ) {
            fprintf ( stderr, "usage: %s frac <qual> <rate> <outdir>\n", argv [0] );
            return 2;
        }
        return mode_frac ( argv [2], argv [3], argv [4] );
    }

    fprintf ( stderr, "unknown mode '%s'\n", mode );
    return 2;
}

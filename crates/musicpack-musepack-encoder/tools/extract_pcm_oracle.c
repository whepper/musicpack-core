/*
 * Temporary migration tooling (J.6), NOT part of the MusicPack build.
 *
 * Extracts the reference's PCM input conversion as an intermediate-value
 * oracle, so a passing whole-stream differential cannot conceal an
 * accidental conversion coincidence. For the first N frames of each input
 * WAV it records, via the *unmodified* reference code path
 * (`Read_WAV_Samples` in `wave_in.c`):
 *
 *   * the left-aligned `i32` value each sample has after sign extension
 *     and left-alignment (the representation the Rust `encode_s32` input
 *     path receives from `musicpack_core::audio`'s `read_s32`),
 *   * the pre-fix `f16`/`f24`/`f32` conversion value (`f(c)`; recorded
 *     for diagnostics — for the 24/32-bit oracle inputs the production
 *     path converts through exactly these static functions),
 *   * the final `L`/`R`/`M`/`S` `f32` bit patterns that reach the
 *     encoder's analysis buffers (denormal fix + mid/side included).
 *
 * The Rust side replays the recorded inputs through its private input
 * conversion (`PcmSource::sample` + `read_block`) and requires bit-equal
 * results; see the `read_block_matches_the_c_pcm_conversion_oracle` test.
 *
 * To reach the static conversion helpers this tool `#include`s the
 * *unmodified* `wave_in.c`, the same pattern as `extract_coding_oracle.c`.
 * It is temporary migration tooling: not built by Cargo, never invoked by
 * Rust tests.
 *
 * Build (from this directory), with the reference repository at <REF>:
 *
 *   REF=/path/to/musicpack
 *   cc -O0 -ffp-contract=off -std=gnu11 -DFAST_MATH \
 *      -I "$REF/codec/include" -I "$REF/codec/libmpcenc" \
 *      -I "$REF/codec/libmpcpsy" -I "$REF/codec/mpcenc" \
 *      extract_pcm_oracle.c \
 *      "$REF/codec/mpcenc/pipeopen.c" "$REF/codec/mpcenc/stderr.c" \
 *      -lm -o /tmp/extract_pcm_oracle
 *
 * Usage: extract_pcm_oracle <frames> <input.wav> [input.wav ...]
 *   stdout is the oracle text (commit it as
 *   tests/data/encoder/pcm_conversion_oracle.txt).
 *
 * The inputs are the wide WAVs kept by `gen_encoder_fixtures.py` in
 * /tmp/mpj6-wav/ (re-run that tool first if the directory is missing).
 *
 * SPDX-License-Identifier: LGPL-2.1-or-later
 */

#include "wave_in.c"

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* Bit pattern of a float (oracle values are compared as bits). */
static uint32_t
fbits (float v)
{
    uint32_t u;
    memcpy (&u, &v, 4);
    return u;
}

/*
 * Left-aligned s32 of one raw little-endian sample: u8 (v-128) << 24,
 * i16 << 16, i24 sign-extend then << 8, i32 verbatim — the documented
 * `musicpack_core::audio` `read_s32` contract and the input `encode_s32`
 * receives. Shifts are done on unsigned values (well-defined mod 2^32)
 * and the final uint32 -> int32 conversion wraps, as both compilers do.
 *
 * This only *records* the input; no oracle value is derived from it.
 */
static int32_t
left_aligned (const unsigned char* p, unsigned bytes)
{
    switch ( bytes ) {
    case 1:
        return (int32_t) ( ((uint32_t) p[0] - 0x80u) << 24 );
    case 2:
        return (int32_t) ( ((uint32_t) p[0] | ((uint32_t) p[1] << 8)) << 16 );
    case 3: {
        uint32_t u = (uint32_t) p[0] | ((uint32_t) p[1] << 8) | ((uint32_t) p[2] << 16);
        if ( u & 0x00800000u )
            u |= 0xFF000000u;
        return (int32_t) (u << 8);
    }
    default:
        return (int32_t) ((uint32_t) p[0] | ((uint32_t) p[1] << 8) |
                          ((uint32_t) p[2] << 16) | ((uint32_t) p[3] << 24));
    }
}

/* The reference pre-fix conversion `f(c)` for the file's byte width. */
static float
pre_fix (const void* p, unsigned bytes)
{
    switch ( bytes ) {
    case 1:  return f8 (p);
    case 2:  return f16 (p);
    case 3:  return f24 (p);
    default: return f32 (p);
    }
}

int
main (int argc, char** argv)
{
    int frames_arg, a;

    if ( argc < 3 ) {
        fprintf (stderr, "usage: %s <frames> <input.wav> [input.wav ...]\n", argv[0]);
        return 2;
    }
    frames_arg = atoi (argv[1]);
    if ( frames_arg <= 0 || frames_arg > BLOCK ) {
        fprintf (stderr, "frames must be in 1..%d (one Rust read_block call)\n", BLOCK);
        return 2;
    }

    printf ("# PCM conversion oracle (J.6) -- tools/extract_pcm_oracle.c\n");
    printf ("# produced by the scalar C reference mpcenc 1.32.0 input path"
            " (Read_WAV_Samples)\n");
    printf ("# all columns after the index are hex32 bit patterns"
            " (inputs: two's complement)\n");

    for ( a = 2; a < argc; a++ ) {
        wave_t          w;
        PCMDataTyp      buf;
        unsigned char*  raw;
        size_t          frames, got, i;
        unsigned        bytes, ch;
        int             silence = 0;
        const char*     name = argv[a];
        const char*     base;

        memset (&w, 0, sizeof w);
        memset (&buf, 0, sizeof buf);
        if ( Open_WAV_Header (&w, name) < 0 || Read_WAV_Header (&w) != 0 ) {
            fprintf (stderr, "cannot open/parse WAV '%s'\n", name);
            return 1;
        }
        bytes = (unsigned) w.BytesPerSample;
        ch = (unsigned) w.Channels;
        if ( ch < 1 || ch > 2 || (bytes != 2 && bytes != 3 && bytes != 4) ) {
            fprintf (stderr, "'%s': oracle covers 24/32-bit (and 16-bit) mono/stereo WAVs\n", name);
            return 1;
        }
        frames = (size_t) frames_arg;
        if ( frames > (size_t) w.PCMSamples )
            frames = (size_t) w.PCMSamples;

        /* Record the raw inputs first, rewind to the start of the PCM
         * data, then let the reference convert the same `frames`
         * frames. */
        raw = (unsigned char*) malloc (frames * ch * bytes);
        if ( raw == NULL || fread (raw, ch * bytes, frames, w.fp) != frames ) {
            fprintf (stderr, "'%s': cannot read %u raw frames\n", name, (unsigned) frames);
            return 1;
        }
        if ( fseek (w.fp, (long) w.PCMOffset, SEEK_SET) != 0 ) {
            fprintf (stderr, "'%s': cannot rewind to PCM data\n", name);
            return 1;
        }
        got = Read_WAV_Samples (&w, frames, &buf, CENTER, 1.0f, 1.0f, &silence);
        if ( got != frames ) {
            fprintf (stderr, "'%s': short reference read (%u of %u)\n",
                     name, (unsigned) got, (unsigned) frames);
            return 1;
        }

        base = strrchr (name, '/');
        base = base ? base + 1 : name;
        printf ("# file %s channels=%u depth=%u frames=%u\n",
                base, ch, bytes * 8, (unsigned) frames);

        for ( i = 0; i < frames; i++ ) {
            const unsigned char* p = raw + i * ch * bytes;
            if ( ch == 2 ) {
                printf ("%u %08x %08x %08x %08x %08x %08x %08x %08x\n",
                        (unsigned) i,
                        (uint32_t) left_aligned (p, bytes),
                        (uint32_t) left_aligned (p + bytes, bytes),
                        fbits (pre_fix (p, bytes)),
                        fbits (pre_fix (p + bytes, bytes)),
                        fbits (buf.L[CENTER + i]), fbits (buf.R[CENTER + i]),
                        fbits (buf.M[CENTER + i]), fbits (buf.S[CENTER + i]));
            } else {
                printf ("%u %08x %08x %08x %08x %08x %08x\n",
                        (unsigned) i,
                        (uint32_t) left_aligned (p, bytes),
                        fbits (pre_fix (p, bytes)),
                        fbits (buf.L[CENTER + i]), fbits (buf.R[CENTER + i]),
                        fbits (buf.M[CENTER + i]), fbits (buf.S[CENTER + i]));
            }
        }
        free (raw);
        fclose (w.fp);
    }
    return 0;
}

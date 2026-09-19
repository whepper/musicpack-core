/*
  audio_probe.c — generates the committed FLAC PCM reference digest table
  used by tests/audio.rs.

  The MusicPack reference decoder (core/libmusicpack/src/audio.c) decodes FLAC
  through the vendored dr_flac header.  This probe links that exact same
  vendored decoder standalone and writes the decoded PCM as raw
  little-endian bytes to stdout, so its SHA-256 is a decoder-level golden
  digest artefact: the Rust `audio::flac::FlacDecoder` (claxon-backed) must
  reproduce the same PCM as the reference dr_flac build.

  Build & regenerate (from the musicpack-core root; requires the reference
  checkout, default sibling ../musicpack):

      cc -O2 -I../musicpack/core/libmusicpack/vendor \
         -o /tmp/audio_probe tools/audio_probe.c

      : > tests/data/audio_c_reference.txt
      for f in fixtures/reference/audio/flac16-44k.flac \
               fixtures/reference/audio/flac24-48k.flac \
               fixtures/reference/audio/flac24-96k.flac \
               fixtures/reference/audio/flac-mono-44k.flac; do
        for m in s32 f32; do
          printf '%s %s %s\n' "$(basename "$f")" "$m" \
            "$(/tmp/audio_probe "$f" "$m" | shasum -a 256 | cut -d' ' -f1)"
        done
      done >> tests/data/audio_c_reference.txt

  The output byte order is explicitly little-endian, so the committed table
  is independent of the host the probe is built on.  The s32 stream is the
  reference's left-aligned 32-bit representation; the f32 stream is its
  normalized float output.
*/

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define DRFLAC_API static
#define DR_FLAC_IMPLEMENTATION
#include "dr_flac.h"

static void
put_u32le(uint32_t v)
{
    unsigned char b[4];
    b[0] = (unsigned char) (v & 0xffu);
    b[1] = (unsigned char) ((v >> 8) & 0xffu);
    b[2] = (unsigned char) ((v >> 16) & 0xffu);
    b[3] = (unsigned char) ((v >> 24) & 0xffu);
    fwrite(b, 1, sizeof b, stdout);
}

int
main(int argc, char **argv)
{
    drflac *d;
    unsigned channels;
    int as_s32;

    if (argc != 3) {
        fprintf(stderr, "usage: %s <file.flac> <s32|f32>\n", argv[0]);
        return 2;
    }
    if (strcmp(argv[2], "s32") == 0)
        as_s32 = 1;
    else if (strcmp(argv[2], "f32") == 0)
        as_s32 = 0;
    else {
        fprintf(stderr, "mode must be s32 or f32\n");
        return 2;
    }

    d = drflac_open_file(argv[1], 0);
    if (d == 0) {
        fprintf(stderr, "dr_flac could not open %s\n", argv[1]);
        return 1;
    }
    channels = d->channels;

    for (;;) {
        if (as_s32) {
            drflac_int32 buf[1152 * 8];
            drflac_uint64 got = drflac_read_pcm_frames_s32(d, 1152, buf);
            drflac_uint64 i, n;
            if (got == 0)
                break;
            n = got * channels;
            for (i = 0; i < n; i++)
                put_u32le((uint32_t) buf[i]);
        } else {
            float buf[1152 * 8];
            drflac_uint64 got = drflac_read_pcm_frames_f32(d, 1152, buf);
            drflac_uint64 i, n;
            if (got == 0)
                break;
            n = got * channels;
            for (i = 0; i < n; i++) {
                uint32_t bits;
                memcpy(&bits, &buf[i], sizeof bits);
                put_u32le(bits);
            }
        }
    }

    drflac_close(d);
    return 0;
}

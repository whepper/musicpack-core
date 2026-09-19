/*
 * Temporary migration tooling (Phase 15E.1), NOT part of the MusicPack build.
 *
 * `Init_ANS()` fills three file-static tables in `libmpcpsy/ans.c`
 * (`InvFourier`, `Cos_Tab`, `Sin_Tab`) from libm `cos`/`sin`. Those tables are
 * not exported, so this dumper `#include`s the unmodified `ans.c` and prints
 * their exact bit patterns (plus `maxANSOrder`) for Rust to freeze.
 *
 * Build:
 *   cc -O0 -ffp-contract=off -std=gnu11 -DFAST_MATH -DCVD_FASTLOG \
 *      -I <REF>/codec/include -I <REF>/codec/libmpcpsy \
 *      extract_ans_tables.c -lm -o /tmp/extract_ans_tables
 *
 * Usage: extract_ans_tables <outdir>
 *
 * SPDX-License-Identifier: LGPL-2.1-or-later
 */

#include "ans.c"

#include <stdint.h>
#include <stdio.h>
#include <string.h>

static void put_f32(FILE *f, float v)
{
    uint32_t u;
    memcpy(&u, &v, 4);
    fprintf(f, "%08x\n", u);
}

int main(int argc, char **argv)
{
    char path[4096];
    FILE *f;
    int k, n;

    if (argc < 2) {
        fprintf(stderr, "usage: %s <outdir>\n", argv[0]);
        return 2;
    }

    Init_ANS();

    snprintf(path, sizeof(path), "%s/ans_tables.txt", argv[1]);
    f = fopen(path, "w");
    if (!f) { perror(path); return 1; }

    /* InvFourier[MAX_NS_ORDER+1][16] */
    for (k = 0; k <= MAX_NS_ORDER; k++)
        for (n = 0; n < 16; n++)
            put_f32(f, InvFourier[k][n]);
    /* Cos_Tab[16][MAX_NS_ORDER+1], Sin_Tab[16][MAX_NS_ORDER+1] */
    for (n = 0; n < 16; n++)
        for (k = 0; k <= MAX_NS_ORDER; k++)
            put_f32(f, Cos_Tab[n][k]);
    for (n = 0; n < 16; n++)
        for (k = 0; k <= MAX_NS_ORDER; k++)
            put_f32(f, Sin_Tab[n][k]);
    /* maxANSOrder[32] */
    for (k = 0; k < 32; k++)
        fprintf(f, "%u\n", (unsigned)maxANSOrder[k]);

    fclose(f);
    return 0;
}

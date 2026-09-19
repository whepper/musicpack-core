/*
  g8_probe.c — generates the committed %.8g / json_number reference vector
  table used by tests/numeric_format.rs.

  Build & run (from the musicpack-core root):
      cc -O2 -o g8_probe tools/g8_probe.c && ./g8_probe > tests/data/g8_c_reference.txt

  The probe prints one "<hex bits> <output>" pair per line, where output is
  exactly what the C reference's json_number() (manifest.c) produces:
  integral values within ±9.2e18 via "%lld", everything else via "%.8g".
*/
#include <stdio.h>
#include <stdint.h>
#include <string.h>
#include <math.h>

static void emit(double v)
{
    char tmp[64];
    /* json_number's integral-branch condition casts v to long long BEFORE
       the range guard: for integral |v| beyond ~9.2e18 (i64 range) the cast
       is undefined behaviour and the branch decision is not deterministic
       (observed: the same bits produced different outputs at different
       call sites in one process). Those values are excluded from the
       committed table; musicpack-core deterministically uses %.8g there. */
    if (v == floor(v) && fabs(v) > 9.2e18)
        return;
    if (v == (double)(long long)v && v >= -9.2e18 && v <= 9.2e18)
        snprintf(tmp, sizeof tmp, "%lld", (long long)v);
    else
        snprintf(tmp, sizeof tmp, "%.8g", v);
    uint64_t bits;
    memcpy(&bits, &v, sizeof bits);
    printf("%016lx %s\n", (unsigned long)bits, tmp);
}

static uint64_t rng_state = 0x9E3779B97F4A7C15ull;
static uint64_t rng(void)
{
    rng_state ^= rng_state << 13;
    rng_state ^= rng_state >> 7;
    rng_state ^= rng_state << 17;
    return rng_state;
}

int main(void)
{
    /* Manifest-realistic values. */
    emit(-7.1902902); emit(-12.0); emit(-1.0); emit(1.5); emit(0.0); emit(-0.0);
    emit(1.0); emit(100.0); emit(-60.0); emit(864000.0); emit(2147483647.0);
    emit(10.0); emit(2.0); emit(1.0 / 3.0); emit(2.0 / 3.0);

    /* Interesting %.8g boundaries. */
    emit(0.0001); emit(1e-5); emit(9.9999999e-5); emit(1e-4 - 1e-12);
    emit(1e7); emit(99999999.0); emit(1e8 / 1.0);
    emit(123456789.5); emit(12345678.5); emit(1234567894.5);
    emit(98765432.5); emit(98765433.5); emit(9876543.5); emit(987654.5);
    emit(9.999999995e-5); emit(0.123456789); emit(0.999999995);
    emit(0.500000015); emit(1.000000055);

    /* Extremes. */
    emit(5e-324); emit(2.2250738585072014e-308); emit(1.7976931348623157e308);
    emit(1e300); emit(-1e300); emit(1e-300); emit(1e18); emit(9.2e18);
    emit(9.3e18); emit(-9.2e18); emit(9219999999999999000.0);
    emit(9007199254740992.0); emit(9007199254740993.0); emit(9007199254740994.0);
    emit(1e16); emit(1e17); emit(1e19); emit(4503599627370496.0);

    /* Decimal sweep. */
    for (int i = 1; i <= 400; i++) {
        double v = i * 0.013;
        emit(v); emit(-v); emit(v * 1e-7); emit(v * 1e10);
    }
    /* Powers of ten and their neighbours. */
    for (int p = -323; p <= 308; p++) {
        emit(pow(10.0, p));
        emit(nextafter(pow(10.0, p), INFINITY));
        emit(nextafter(pow(10.0, p), 0.0));
    }
    /* Deterministic random doubles (normal range). */
    for (int i = 0; i < 300; i++) {
        uint64_t bits = rng();
        double v;
        do {
            bits = rng();
            memcpy(&v, &bits, sizeof v);
        } while (!isfinite(v) || v != v || fabs(v) < 1e-290 || fabs(v) > 1e290);
        emit(v);
        emit(v * 1e-100);
        emit(v * 1e-250);
    }
    /* Deterministic random subnormals / tiny values. */
    for (int i = 0; i < 60; i++) {
        double v = ldexp((double)(rng() % 1000000 + 1), -(1074 + (int)(rng() % 60)));
        emit(v);
    }
    return 0;
}

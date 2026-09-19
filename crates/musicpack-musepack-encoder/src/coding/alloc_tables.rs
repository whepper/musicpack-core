//! Frozen quantiser/allocation tables from the reference `quant.c` (Phase 15E.2).
//!
//! `A_TABLE`/`C_TABLE`/`D_TABLE` are the reference `A`/`C`/`D` arrays
//! (which are `__A`/`__C`/`__D` offset by one) indexed directly by `res`;
//! `NIC` is `NoiseInjectionCompensation1D`. Values are reproduced verbatim
//! from the source so their float rounding cannot drift.
#![allow(clippy::excessive_precision)] // reference float literals kept verbatim

/// `A[res]` step coefficients for `res` 0..=17.
pub(super) const A_TABLE: [f32; 18] = [
    0.0000000000000000f32,
    0.0000457763671875f32,
    0.0000762939453125f32,
    0.0001068115234375f32,
    0.0001373291015625f32,
    0.0002288818359375f32,
    0.0004730224609375f32,
    0.0009613037109375f32,
    0.0019378662109375f32,
    0.0038909912109375f32,
    0.0077972412109375f32,
    0.0156097412109375f32,
    0.0312347412109375f32,
    0.0624847412109375f32,
    0.1249847412109375f32,
    0.2499847412109375f32,
    0.4999847412109375f32,
    0.0000000000000000f32,
];

/// `C[res]` requantisation coefficients for `res` 0..=17.
pub(super) const C_TABLE: [f32; 18] = [
    65535.000000000000f32,
    21845.333333333332f32,
    13107.200000000001f32,
    9362.285714285713f32,
    7281.777777777777f32,
    4369.066666666666f32,
    2114.064516129032f32,
    1040.253968253968f32,
    516.031496062992f32,
    257.003921568627f32,
    128.250489236790f32,
    64.062561094819f32,
    32.015632633121f32,
    16.003907203907f32,
    8.000976681723f32,
    4.000244155527f32,
    2.000061037018f32,
    1.000015259022f32,
];

/// `NoiseInjectionCompensation1D[res]` for `res` 0..=17.
pub(super) const NIC: [f32; 18] = [
    1.0f32,
    0.884621f32,
    0.935711f32,
    0.970829f32,
    0.987941f32,
    0.994315f32,
    0.997826f32,
    0.999744f32,
    1.0f32,
    1.0f32,
    1.0f32,
    1.0f32,
    1.0f32,
    1.0f32,
    1.0f32,
    1.0f32,
    1.0f32,
    1.0f32,
];

/// `D[res]` quantiser offsets for `res` 0..=17.
pub(super) const D_TABLE: [i32; 18] = [
    0, 1, 2, 3, 4, 7, 15, 31, 63, 127, 255, 511, 1023, 2047, 4095, 8191, 16383, 32767,
];

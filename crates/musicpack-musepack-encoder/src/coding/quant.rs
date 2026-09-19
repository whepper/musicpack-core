//! Musepack quantisation — the reference `Quantisierung`, `QuantizeSubband`
//! and `QuantizeSubbandWithNoiseShaping`.
//!
//! Faithful scalar port from `codec/mpcenc/mpcenc.c` and
//! `codec/libmpcenc/quant.c`. It consumes the exact allocation outputs
//! (`Res`, post-allocation `X`, `NS_Order`, `FIR`) and produces the quantised
//! sample array `Q`. It does **not** implement Huffman coding or the `AP`
//! payload.
//!
//! # Q representation
//!
//! `Q` is `mpc_quantizer { i16 L[36]; i16 R[36] }` per band — the quantiser
//! bin indices, **not** PCM. Plain bands hold `0..=2·D[res]`; the noise-shaped
//! path holds `0..=2·D[res]` as well (the signed clamp `[-D[res], +D[res]]`
//! plus the `+D[res]` offset). Bands with `Res <= 0` are not written by the
//! reference, so `Q` is **stateful across frames** and retains its previous
//! value; [`Quantizer`] models that explicitly.
//!
//! # Numerical compatibility
//!
//! * `mpc_lrintf` is reused from the allocation stage (bit-for-bit);
//! * `QuantizeSubband` computes `(unsigned)(lrintf(x·mult) + offset)` and
//!   clamps with the reference's unsigned comparison (`quant < 0 → 0`,
//!   `quant > 2·offset → 2·offset`);
//! * `QuantizeSubbandWithNoiseShaping` resets its 6-sample error history with
//!   `memset(errors, 0, 6)` at the start of every call, so its state is local
//!   to the call (the reference's static `errorL`/`errorR` carry is dead);
//! * the error term is computed from the **unclamped** `lrintf` result, before
//!   the clamp, exactly as the reference orders it.

use super::alloc_tables::{A_TABLE, C_TABLE, D_TABLE, NIC};
use super::allocate::lrintf;
use crate::error::EncoderError;
use crate::filterbank::Subband;

const SUBBANDS: usize = 32;
const SAMPLES: usize = 36;
/// `MAX_NS_ORDER`; also the error-history size the reference keeps.
const MAX_NS_ORDER: usize = 6;

/// The reference `QuantizeSubband` for one 36-sample subband.
///
/// The reference splits the loop at `36 - MAX_NS_ORDER` only to write its
/// (unobserved) `errors` array; `Q` is identical in both halves, so this uses
/// one loop.
#[allow(clippy::needless_range_loop)]
fn quantize_subband(res: i32, input: &[f32; SAMPLES], out: &mut [i16; SAMPLES]) {
    let offset = D_TABLE[res as usize];
    let nic = NIC[res as usize];
    let mult = A_TABLE[res as usize] * nic;
    for n in 0..SAMPLES {
        let mut quant = lrintf(input[n] * mult) + offset;
        // The reference casts to `unsigned` and clamps with
        // `mini(quant, 2*offset)` then `maxi(quant, 0)`.
        if (quant as u32) > (offset as u32) * 2 {
            quant = quant.min(offset * 2);
            quant = quant.max(0);
        }
        out[n] = quant as i16;
    }
}

/// The reference `QuantizeSubbandWithNoiseShaping` for one subband.
#[allow(clippy::needless_range_loop)]
fn quantize_subband_with_noise_shaping(
    res: i32,
    input: &[f32; SAMPLES],
    fir: &[f32; MAX_NS_ORDER],
    out: &mut [i16; SAMPLES],
) {
    let offset = D_TABLE[res as usize];
    let nic = NIC[res as usize];
    let mult = A_TABLE[res as usize];
    let invmult = C_TABLE[res as usize];

    // `memset(errors, 0, 6)` — the history is local to this call.
    let mut errors = [0.0f32; SAMPLES + MAX_NS_ORDER];

    for n in 0..SAMPLES {
        let signal = input[n] * nic
            - (fir[5] * errors[n]
                + fir[4] * errors[n + 1]
                + fir[3] * errors[n + 2]
                + fir[2] * errors[n + 3]
                + fir[1] * errors[n + 4]
                + fir[0] * errors[n + 5]);
        let mut quant = lrintf(signal * mult);
        // The error uses the unclamped `quant`, before the clamp below.
        errors[n + 6] = invmult * (quant as f32) - signal * nic;
        if quant > offset {
            quant = offset;
        }
        if quant < -offset {
            quant = -offset;
        }
        out[n] = (quant + offset) as i16;
    }
}

/// Persistent quantised-sample state (`e.Q` in the reference).
///
/// The reference does not reset `Q` per frame, so bands with `Res <= 0` keep
/// their previous values. Create one, keep it across frames, and call
/// [`Quantizer::quantize`] once per frame.
#[derive(Clone, Debug)]
pub struct Quantizer {
    q_l: [[i16; SAMPLES]; SUBBANDS],
    q_r: [[i16; SAMPLES]; SUBBANDS],
}

impl Default for Quantizer {
    fn default() -> Self {
        Self::new()
    }
}

impl Quantizer {
    /// Creates a quantiser with a zeroed `Q` (the reference's initial state).
    pub fn new() -> Self {
        Self {
            q_l: [[0; SAMPLES]; SUBBANDS],
            q_r: [[0; SAMPLES]; SUBBANDS],
        }
    }

    /// Clears `Q`.
    pub fn reset(&mut self) {
        self.q_l = [[0; SAMPLES]; SUBBANDS];
        self.q_r = [[0; SAMPLES]; SUBBANDS];
    }

    /// The quantised left channel, per band.
    pub fn q_l(&self) -> &[[i16; SAMPLES]; SUBBANDS] {
        &self.q_l
    }

    /// The quantised right channel, per band.
    pub fn q_r(&self) -> &[[i16; SAMPLES]; SUBBANDS] {
        &self.q_r
    }

    /// Runs the reference `Quantisierung` for one frame.
    ///
    /// `res_l`/`res_r` are the allocation results; `ns_order_l`/`ns_order_r`
    /// and `fir_l`/`fir_r` are the noise-shaping analysis outputs; `x` is the
    /// post-allocation subband matrix.
    #[allow(clippy::too_many_arguments)]
    pub fn quantize(
        &mut self,
        max_band: usize,
        res_l: &[i32; SUBBANDS],
        res_r: &[i32; SUBBANDS],
        ns_order_l: &[u32; SUBBANDS],
        ns_order_r: &[u32; SUBBANDS],
        fir_l: &[[f32; MAX_NS_ORDER]; SUBBANDS],
        fir_r: &[[f32; MAX_NS_ORDER]; SUBBANDS],
        x: &[Subband; SUBBANDS],
    ) -> Result<(), EncoderError> {
        if max_band >= SUBBANDS {
            return Err(EncoderError::InvalidMaxBand(max_band));
        }
        for band in 0..=max_band {
            if res_l[band] > 0 {
                if ns_order_l[band] > 0 {
                    quantize_subband_with_noise_shaping(
                        res_l[band],
                        &x[band].left,
                        &fir_l[band],
                        &mut self.q_l[band],
                    );
                } else {
                    quantize_subband(res_l[band], &x[band].left, &mut self.q_l[band]);
                }
            }
            if res_r[band] > 0 {
                if ns_order_r[band] > 0 {
                    quantize_subband_with_noise_shaping(
                        res_r[band],
                        &x[band].right,
                        &fir_r[band],
                        &mut self.q_r[band],
                    );
                } else {
                    quantize_subband(res_r[band], &x[band].right, &mut self.q_r[band]);
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_input_quantises_to_the_centre_bin() {
        let mut out = [0i16; SAMPLES];
        quantize_subband(3, &[0.0; SAMPLES], &mut out);
        // offset = D[3] = 3, so a zero signal lands at bin 3.
        assert!(out.iter().all(|&q| q == 3), "{out:?}");
    }

    #[test]
    fn plain_quantiser_clamps_to_the_bin_range() {
        let mut out = [0i16; SAMPLES];
        let mut input = [0.0f32; SAMPLES];
        input[0] = 1.0e30; // enormous -> clamped high
        input[1] = -1.0e30; // enormous negative -> clamped to 0
        quantize_subband(3, &input, &mut out);
        assert_eq!(out[0], 6); // 2 * D[3]
        assert_eq!(out[1], 0);
    }

    #[test]
    fn noise_shaped_quantiser_stays_in_range() {
        let fir = [0.1f32; MAX_NS_ORDER];
        let input = [100.0f32; SAMPLES];
        let mut out = [0i16; SAMPLES];
        quantize_subband_with_noise_shaping(8, &input, &fir, &mut out);
        let offset = D_TABLE[8] as i16;
        for &q in &out {
            assert!((0..=2 * offset).contains(&q), "{q}");
        }
    }
}

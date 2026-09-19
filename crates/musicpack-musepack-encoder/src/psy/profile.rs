//! Encoder profile resolution (`profile.c`).
//!
//! `SetQualityParams` maps the floating quality (`0..=10`) onto the reference
//! `Profiles` table. The table is literal source data; the interpolation only
//! matters for non-integral qualities, but is reproduced in full.

/// One row of the reference `Profiles[16]` table.
#[derive(Clone, Copy, Default)]
struct ProfileSetting {
    short_thr: f32,
    min_val_choice: u8,
    ear_model_flag: u32,
    ltq_offset: i8,
    tmn: f32,
    nmt: f32,
    min_smr: i8,
    ltq_max: i8,
    band_width: u16,
    tmp_mask_used: u8,
    cvd_used: u8,
    var_ltq: f32,
    ms_channelmode: u8,
    comb_penalities: u8,
    ns_order: u8,
    pns: f32,
    trans_detect: f32,
}

macro_rules! profile {
    ($st:expr, $mv:expr, $em:expr, $lo:expr, $tmn:expr, $nmt:expr, $ms:expr, $lm:expr, $bw:expr,
     $tm:expr, $cvd:expr, $vl:expr, $msc:expr, $cp:expr, $ns:expr, $pns:expr, $td:expr) => {
        ProfileSetting {
            short_thr: $st,
            min_val_choice: $mv,
            ear_model_flag: $em,
            ltq_offset: $lo,
            tmn: $tmn,
            nmt: $nmt,
            min_smr: $ms,
            ltq_max: $lm,
            band_width: $bw,
            tmp_mask_used: $tm,
            cvd_used: $cvd,
            var_ltq: $vl,
            ms_channelmode: $msc,
            comb_penalities: $cp,
            ns_order: $ns,
            pns: $pns,
            trans_detect: $td,
        }
    };
}

/// The all-zero profile row used for indices `0..=4`.
const ZERO: ProfileSetting = profile!(
    0.0, 0, 0, 0, 0.0, 0.0, 0, 0, 0, 0, 0, 0.0, 0, 0, 0, 0.0, 0.0
);

/// The reference `Profiles[16]` table (`profile.c`). Indices `0..=4` are zero.
const PROFILES: [ProfileSetting; 16] = [
    ZERO,
    ZERO,
    ZERO,
    ZERO,
    ZERO,
    // 0: pre-Telephone
    profile!(
        1.0e9, 1, 300, 30, 3.0, -1.0, 0, 106, 4820, 1, 1, 1.0, 3, 24, 6, 1.09, 200.0
    ),
    // 1: pre-Telephone
    profile!(
        1.0e9, 1, 300, 24, 6.0, 0.5, 0, 100, 7570, 1, 1, 1.0, 3, 20, 6, 0.77, 180.0
    ),
    // 2: Telephone
    profile!(
        1.0e9, 1, 400, 18, 9.0, 2.0, 0, 94, 10300, 1, 1, 1.0, 4, 18, 6, 0.55, 160.0
    ),
    // 3: Thumb
    profile!(
        50.0, 2, 430, 12, 12.0, 3.5, 0, 88, 13090, 1, 1, 1.0, 5, 15, 6, 0.39, 140.0
    ),
    // 4: Radio
    profile!(
        15.0, 2, 440, 6, 15.0, 5.0, 0, 82, 15800, 1, 1, 1.0, 6, 10, 6, 0.27, 120.0
    ),
    // 5: Standard
    profile!(
        5.0, 2, 550, 0, 18.0, 6.5, 1, 76, 19980, 1, 2, 1.0, 11, 9, 6, 0.0, 100.0
    ),
    // 6: Xtreme
    profile!(
        4.0, 2, 560, -6, 21.0, 8.0, 2, 70, 22000, 1, 2, 1.0, 12, 7, 6, 0.0, 80.0
    ),
    // 7: Insane
    profile!(
        3.0, 2, 570, -12, 24.0, 9.5, 3, 64, 24000, 1, 2, 2.0, 13, 5, 6, 0.0, 60.0
    ),
    // 8: BrainDead
    profile!(
        2.8, 2, 580, -18, 27.0, 11.0, 4, 58, 26000, 1, 2, 4.0, 13, 4, 6, 0.0, 40.0
    ),
    // 9: post-BrainDead
    profile!(
        2.6, 2, 590, -24, 30.0, 12.5, 5, 52, 28000, 1, 2, 8.0, 13, 4, 6, 0.0, 20.0
    ),
    //10: post-BrainDead
    profile!(
        2.4, 2, 599, -30, 33.0, 14.0, 6, 46, 30000, 1, 2, 16.0, 15, 2, 6, 0.0, 10.0
    ),
];

/// `PROFILE_PRE2_TELEPHONE`.
const PROFILE_PRE2_TELEPHONE: i32 = 5;
/// `PROFILE_POST2_BRAINDEAD`.
const PROFILE_POST2_BRAINDEAD: i32 = 15;

/// Resolved profile parameters, mirroring the reference `PsyModel` fields set
/// by `SetQualityParams`.
#[derive(Clone, Copy, Debug)]
pub struct PsyParams {
    /// `MainQual`.
    pub main_qual: i32,
    /// `FullQual`.
    pub full_qual: f32,
    /// `ShortThr`.
    pub short_thr: f32,
    /// `MinValChoice`.
    pub min_val_choice: i32,
    /// `EarModelFlag`.
    pub ear_model_flag: u32,
    /// `Ltq_offset`.
    pub ltq_offset: f32,
    /// `TMN`.
    pub tmn: f32,
    /// `NMT`.
    pub nmt: f32,
    /// `minSMR`.
    pub min_smr: f32,
    /// `Ltq_max`.
    pub ltq_max: f32,
    /// `BandWidth`.
    pub band_width: f32,
    /// `tmpMask_used`.
    pub tmp_mask_used: u8,
    /// `CVD_used`.
    pub cvd_used: u8,
    /// `varLtq`.
    pub var_ltq: f32,
    /// `MS_Channelmode`.
    pub ms_channelmode: u8,
    /// `CombPenalities`.
    pub comb_penalities: i32,
    /// `NS_Order`.
    pub ns_order: u32,
    /// `PNS`.
    pub pns: f32,
    /// `TransDetect`.
    pub trans_detect: f32,
}

impl PsyParams {
    /// The reference `SetQualityParams`: maps a floating quality onto the
    /// profile table with the same interpolation.
    #[must_use]
    pub fn from_quality(qual: f32) -> Self {
        let qual = qual.clamp(0.0, 10.0);
        let i = (qual as i32) + PROFILE_PRE2_TELEPHONE;
        let mix = qual - (qual as i32) as f32;
        let j = if i == PROFILE_POST2_BRAINDEAD {
            i
        } else {
            i + 1
        };
        let a = &PROFILES[i as usize];
        let b = &PROFILES[j as usize];
        let lerp = |x: f32, y: f32| x * (1.0 - mix) + y * mix;
        Self {
            main_qual: i,
            full_qual: qual + PROFILE_PRE2_TELEPHONE as f32,
            short_thr: lerp(a.short_thr, b.short_thr),
            min_val_choice: a.min_val_choice as i32,
            ear_model_flag: a.ear_model_flag,
            ltq_offset: lerp(a.ltq_offset as f32, b.ltq_offset as f32),
            tmn: lerp(a.tmn, b.tmn),
            nmt: lerp(a.nmt, b.nmt),
            min_smr: a.min_smr as f32,
            ltq_max: lerp(a.ltq_max as f32, b.ltq_max as f32),
            band_width: lerp(a.band_width as f32, b.band_width as f32),
            tmp_mask_used: a.tmp_mask_used,
            cvd_used: a.cvd_used,
            var_ltq: lerp(a.var_ltq, b.var_ltq),
            ms_channelmode: a.ms_channelmode,
            comb_penalities: a.comb_penalities as i32,
            ns_order: a.ns_order as u32,
            pns: lerp(a.pns, b.pns),
            trans_detect: lerp(a.trans_detect, b.trans_detect),
        }
    }
}

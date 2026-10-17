//! pYIN's priors: a beta distribution over YIN thresholds and a Boltzmann
//! distribution over the troughs under each, which together give each period
//! of a frame the probability that it is the frame's period.
//!
//! As in the official pYIN software (and librosa), which differs from the
//! paper's method.

/// Thresholds spread evenly over 0 to 1.
pub const THRESHOLDS: usize = 100;

/// The Boltzmann prior's shape over troughs; larger favours shorter periods.
const BOLTZMANN: f64 = 2.0;

/// What goes to the global minimum for each threshold no trough falls under.
const NO_TROUGH: f64 = 0.01;

/// The Beta(2, 18) prior's mass between each pair of adjacent thresholds, as
/// the crate computed it with statrs 0.17.1 (`cdf((i + 1) / 100) - cdf(i / 100)`),
/// bit for bit. The closed form in the tests agrees to 1e-13; these are the
/// crate's own roundings, which its decisions were made with.
pub const BETA_PROBS: [f64; THRESHOLDS] = [
    f64::from_bits(0x3f8f_47d9_90a7_5761), // 1.5273761488895312e-2
    f64::from_bits(0x3fa4_24a8_ca4f_f16a), // 3.934218854553502e-2
    f64::from_bits(0x3fac_5568_41e8_3582), // 5.533910564927692e-2
    f64::from_bits(0x3fb0_ac2d_3a2f_bf96), // 6.512720747536913e-2
    f64::from_bits(0x3fb1_f951_41c9_c450), // 7.021053177028702e-2
    f64::from_bits(0x3fb2_612c_d56f_dea8), // 7.179527483810466e-2
    f64::from_bits(0x3fb2_22a8_719c_92c8), // 7.084133886071264e-2
    f64::from_bits(0x3fb1_6f63_3aa3_f594), // 6.81058900990677e-2
    f64::from_bits(0x3fb0_6e1a_82fb_55b4), // 6.418004701421048e-2
    f64::from_bits(0x3fae_795c_e6da_a540), // 5.9519675426893404e-2
    f64::from_bits(0x3fab_e3a4_3c5e_afe0), // 5.447114216744331e-2
    f64::from_bits(0x3fa9_3ce6_d164_dab0), // 4.9292767582689634e-2
    f64::from_bits(0x3fa6_9dcb_2616_3d10), // 4.417261924674609e-2
    f64::from_bits(0x3fa4_17af_6200_f020), // 3.924320288976468e-2
    f64::from_bits(0x3fa1_b63e_45b6_ce50), // 3.4593530664463246e-2
    f64::from_bits(0x3f9f_0173_f85e_3c40), // 3.0278980287729462e-2
    f64::from_bits(0x3f9a_f611_6ebb_2860), // 2.6329300292481483e-2
    f64::from_bits(0x3f97_4d1a_9bfb_9720), // 2.2755065699036625e-2
    f64::from_bits(0x3f94_05a9_2650_1600), // 1.9552844012670434e-2
    f64::from_bits(0x3f91_1c3d_a93c_05e0), // 1.6709292845012658e-2
    f64::from_bits(0x3f8d_172f_33fc_4980), // 1.420437695881982e-2
    f64::from_bits(0x3f88_9ab9_7857_29c0), // 1.2013863549232506e-2
    f64::from_bits(0x3f84_b532_4239_0180), // 1.0111229558902535e-2
    f64::from_bits(0x3f81_583e_75e0_39c0), // 8.46909329632417e-3
    f64::from_bits(0x3f7c_eb39_3a5d_0600), // 7.0602641552013345e-3
    f64::from_bits(0x3f77_ff12_039e_cc00), // 5.858488433591269e-3
    f64::from_bits(0x3f73_d203_4d5f_db00), // 4.838955783262433e-3
    f64::from_bits(0x3f70_4be2_81dc_2800), // 3.9786193790494195e-3
    f64::from_bits(0x3f6a_ad1c_0efc_1600), // 3.256373216461439e-3
    f64::from_bits(0x3f65_bbff_e9b8_8200), // 2.65312178613919e-3
    f64::from_bits(0x3f61_a097_014d_bd00), // 2.1517705267154463e-3
    f64::from_bits(0x3f5c_762d_1122_dc00), // 1.7371597391908633e-3
    f64::from_bits(0x3f56_df14_85fe_2800), // 1.3959598945540996e-3
    f64::from_bits(0x3f52_4b1e_35a2_0c00), // 1.116542340725557e-3
    f64::from_bits(0x3f4d_2019_2feb_7400), // 8.888361915883047e-4
    f64::from_bits(0x3f47_1316_1300_3000), // 7.041795524869165e-4
    f64::from_bits(0x3f42_311c_e168_4400), // 5.551711099675005e-4
    f64::from_bits(0x3f3c_8aeb_addc_2000), // 4.3552640805488707e-4
    f64::from_bits(0x3f36_4746_d365_0000), // 3.3994177956131466e-4
    f64::from_bits(0x3f31_4ca5_3c73_1000), // 2.639678391000988e-4
    f64::from_bits(0x3f2a_b988_e3dd_0000), // 2.038936235582156e-4
    f64::from_bits(0x3f24_8807_2dee_9000), // 1.5664184226438227e-4
    f64::from_bits(0x3f1f_5f44_e665_4000), // 1.1967523605460606e-4
    f64::from_bits(0x3f17_d51d_a82c_4000), // 9.091371070391041e-5
    f64::from_bits(0x3f11_ffce_ac70_c000), // 6.866167960661862e-5
    f64::from_bits(0x3f0b_063d_5274_0000), // 5.1544901277011945e-5
    f64::from_bits(0x3f04_297a_7aa2_0000), // 3.845601106977625e-5
    f64::from_bits(0x3efd_e488_8ee8_0000), // 2.850790854758145e-5
    f64::from_bits(0x3ef6_0393_af17_8000), // 2.0994159916187805e-5
    f64::from_bits(0x3ef0_19fc_bae5_8000), // 1.535559902643424e-5
    f64::from_bits(0x3ee7_635f_ad24_0000), // 1.1152352645371622e-5
    f64::from_bits(0x3ee0_dcbf_7d81_0000), // 8.040569739020675e-6
    f64::from_bits(0x3ed8_2172_554a_0000), // 5.753195485014295e-6
    f64::from_bits(0x3ed1_215e_ee2e_0000), // 4.084194844433675e-6
    f64::from_bits(0x3ec8_1f82_05bc_0000), // 2.875694958492936e-6
    f64::from_bits(0x3ec0_d73d_dab8_0000), // 2.00757832202747e-6
    f64::from_bits(0x3eb7_4e37_9360_0000), // 1.3891181658287621e-6
    f64::from_bits(0x3eaf_f438_1000_0000), // 9.523028268176859e-7
    f64::from_bits(0x3ea5_b1ca_c530_0000), // 6.465464913452124e-7
    f64::from_bits(0x3e9d_2928_4d60_0000), // 4.3452935238885715e-7
    f64::from_bits(0x3e93_6423_31c0_0000), // 2.889508310399691e-7
    f64::from_bits(0x3e89_80df_dcc0_0000), // 1.900152554190626e-7
    f64::from_bits(0x3e80_9371_aac0_0000), // 1.2350047506881623e-7
    f64::from_bits(0x3e75_4889_c500_0000), // 7.928666545531371e-8
    f64::from_bits(0x3e6a_f9ad_1200_0000), // 5.024540628006946e-8
    f64::from_bits(0x3e60_dcbc_5f00_0000), // 3.1408386891307316e-8
    f64::from_bits(0x3e54_c739_9000_0000), // 1.9351227287245365e-8
    f64::from_bits(0x3e49_36d2_5000_0000), // 1.1741251970676103e-8
    f64::from_bits(0x3e3e_1a81_9000_0000), // 7.009026470328195e-9
    f64::from_bits(0x3e31_a99c_5800_0000), // 4.1123809912946285e-9
    f64::from_bits(0x3e24_5915_9000_0000), // 2.368817253461941e-9
    f64::from_bits(0x3e16_fc42_0000_0000), // 1.3379253260836776e-9
    f64::from_bits(0x3e09_6c92_0000_0000), // 7.399387769169152e-10
    f64::from_bits(0x3dfb_7e78_8000_0000), // 4.000907383172603e-10
    f64::from_bits(0x3ded_04f6_0000_0000), // 2.1114376913544675e-10
    f64::from_bits(0x3ddd_d672_0000_0000), // 1.0854883658595327e-10
    f64::from_bits(0x3dcd_d26c_0000_0000), // 5.4245830050092536e-11
    f64::from_bits(0x3dbc_e738_0000_0000), // 2.6287305665562144e-11
    f64::from_bits(0x3dab_16e0_0000_0000), // 1.2318812636635812e-11
    f64::from_bits(0x3d98_79a0_0000_0000), // 5.564992910933597e-12
    f64::from_bits(0x3d85_3d80_0000_0000), // 2.4147350785597155e-12
    f64::from_bits(0x3d71_a180_0000_0000), // 1.0021983243291288e-12
    f64::from_bits(0x3d5b_dc00_0000_0000), // 3.959055305813308e-13
    f64::from_bits(0x3d44_d400_0000_0000), // 1.4799272918253337e-13
    f64::from_bits(0x3d2d_5000_0000_0000), // 5.206945985491984e-14
    f64::from_bits(0x3d13_4000_0000_0000), // 1.709743457922741e-14
    f64::from_bits(0x3cf7_0000_0000_0000), // 5.10702591327572e-15
    f64::from_bits(0x3cda_0000_0000_0000), // 1.4432899320127035e-15
    f64::from_bits(0x3cb8_0000_0000_0000), // 3.3306690738754696e-16
    f64::from_bits(0x3ca0_0000_0000_0000), // 1.1102230246251565e-16
    f64::from_bits(0x0000_0000_0000_0000), // 0e0
    f64::from_bits(0x0000_0000_0000_0000), // 0e0
    f64::from_bits(0x0000_0000_0000_0000), // 0e0
    f64::from_bits(0x0000_0000_0000_0000), // 0e0
    f64::from_bits(0x0000_0000_0000_0000), // 0e0
    f64::from_bits(0x0000_0000_0000_0000), // 0e0
    f64::from_bits(0x0000_0000_0000_0000), // 0e0
    f64::from_bits(0x0000_0000_0000_0000), // 0e0
    f64::from_bits(0x0000_0000_0000_0000), // 0e0
    f64::from_bits(0x0000_0000_0000_0000), // 0e0
];

/// A sum in the order ndarray 0.15 sums a contiguous run: eight lanes, each
/// summed in order, folded as (0 + 4) + (1 + 5) + (2 + 6) + (3 + 7), then the
/// tail of fewer than eight in order. The crate's row sums were these.
pub fn ndarray_sum(values: &[f64]) -> f64 {
    let mut lanes = [0.0_f64; 8];
    let mut chunks = values.chunks_exact(8);
    for chunk in &mut chunks {
        for (lane, &value) in lanes.iter_mut().zip(chunk) {
            *lane += value;
        }
    }
    let mut total = 0.0;
    for pair in 0..4 {
        total += lanes[pair] + lanes[pair + 4];
    }
    for &value in chunks.remainder() {
        total += value;
    }
    total
}

/// The chance of the trough at `position` among `troughs` under a Boltzmann
/// distribution truncated to them.
fn boltzmann(position: usize, troughs: usize) -> f64 {
    let fact = (1.0 - (-BOLTZMANN).exp()) / (1.0 - (-BOLTZMANN * troughs as f64).exp());
    fact * (-BOLTZMANN * position as f64).exp()
}

/// The threshold above the `index`th pair: ndarray's `linspace(0, 1, 101)`.
fn threshold(index: usize) -> f64 {
    (1.0 / THRESHOLDS as f64) * (index + 1) as f64
}

/// Each period's probability, from one frame's YIN values, into `out`:
/// zero but at the frame's troughs.
pub fn trough_probs(yin: &[f64], out: &mut [f64]) {
    out.fill(0.0);
    let troughs = troughs(yin);
    let Some(global_min) = first_min(troughs.iter().map(|&index| yin[index])) else {
        return;
    };

    // How many troughs fall under each threshold; then, trough by trough,
    // its place among those under it, which is how many before it were.
    let mut under = [0_usize; THRESHOLDS];
    for &index in &troughs {
        for (t, count) in under.iter_mut().enumerate() {
            if yin[index] < threshold(t) {
                *count += 1;
            }
        }
    }
    let mut before = [0_usize; THRESHOLDS];
    let mut row = [0.0; THRESHOLDS];
    let mut probs = Vec::with_capacity(troughs.len());
    for &index in &troughs {
        for (t, slot) in row.iter_mut().enumerate() {
            *slot = if yin[index] < threshold(t) {
                let p = boltzmann(before[t], under[t]) * BETA_PROBS[t];
                before[t] += 1;
                p
            } else {
                0.0
            };
        }
        probs.push(ndarray_sum(&row));
    }

    let over_min = (0..THRESHOLDS)
        .filter(|&t| yin[troughs[global_min]] >= threshold(t))
        .count();
    // Rounded twice, as the crate did: not fused.
    let more = NO_TROUGH * ndarray_sum(&BETA_PROBS[..over_min]);
    probs[global_min] += more;

    for (&index, &prob) in troughs.iter().zip(&probs) {
        out[index] = prob;
    }
}

/// The frame's troughs: the first period if it is under the second, each
/// period under the one before and not over the one after, and the last if
/// it is under the one before.
fn troughs(yin: &[f64]) -> Vec<usize> {
    let last = yin.len() - 1;
    let mut troughs = Vec::new();
    if yin[0] < yin[1] {
        troughs.push(0);
    }
    for (index, window) in yin.windows(3).enumerate() {
        if window[1] < window[0] && window[1] <= window[2] {
            troughs.push(index + 1);
        }
    }
    if yin[last] < yin[last - 1] {
        troughs.push(last);
    }
    troughs
}

/// Where the first of the smallest values is, skipping NaN.
fn first_min(values: impl Iterator<Item = f64>) -> Option<usize> {
    let mut best: Option<(usize, f64)> = None;
    for (index, value) in values.enumerate() {
        if value.is_nan() {
            continue;
        }
        match best {
            Some((_, least)) if least <= value => {}
            _ => best = Some((index, value)),
        }
    }
    best.map(|(index, _)| index)
}

#[cfg(test)]
mod tests;

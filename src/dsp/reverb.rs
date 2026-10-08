//! Algorithmic stereo reverb with multiple room types.
//!
//! Four configurations: Room, Hall, Plate, Spring.
//! Each uses different comb/allpass lengths and damping characteristics.
//! Supports freeze mode (infinite sustain, input cut).

use super::filters::OnePoleLP;

const NUM_COMBS: usize = 8;
const NUM_ALLPASS: usize = 4;
const STEREO_SPREAD: usize = 23;
const MAX_PREDELAY_SAMPLES: usize = 44100; // ~1s at 44.1k

/// Shortest reverb time the combs are tuned for, in seconds. The Decay control
/// stops at 0.1 s; this only keeps the feedback formula away from a division
/// by zero.
const MIN_RT60: f32 = 0.01;

/// Highest comb feedback outside freeze. The longest Decay (20 s) on the
/// shortest comb (Spring at Size 0) needs about 0.996, so this never shapes
/// the decay; it only keeps every loop below 1 whatever it is asked for.
const MAX_COMB_FEEDBACK: f32 = 0.998;

/// Per-type comb filter delay lengths (samples at 44.1 kHz).
/// Different lengths create different resonance patterns → different "room" characters.
///
/// The Decay control is the RT60 in every type, and every type is level-matched
/// to Room (see [`Reverb::set_params`]), so a type changes the character of the
/// space, not how long it rings or how loud it is.
struct ReverbTuning {
    comb_lengths: [usize; NUM_COMBS],
    allpass_lengths: [usize; NUM_ALLPASS],
    diffusion: f32,  // allpass feedback coefficient (0.3–0.7)
    damp_scale: f32, // multiplier for damping (higher = darker)
}

const ROOM_TUNING: ReverbTuning = ReverbTuning {
    comb_lengths: [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617],
    allpass_lengths: [556, 441, 341, 225],
    diffusion: 0.5,
    damp_scale: 1.0,
};

const HALL_TUNING: ReverbTuning = ReverbTuning {
    // Longer, more spread out → bigger space
    comb_lengths: [1557, 1617, 1733, 1861, 1993, 2131, 2269, 2399],
    allpass_lengths: [677, 557, 433, 311],
    diffusion: 0.6,
    damp_scale: 0.7, // brighter tails
};

const PLATE_TUNING: ReverbTuning = ReverbTuning {
    // Dense, bright, metallic character
    comb_lengths: [1051, 1123, 1187, 1259, 1321, 1381, 1447, 1511],
    allpass_lengths: [487, 379, 283, 197],
    diffusion: 0.7,  // high diffusion = smooth, dense
    damp_scale: 0.5, // very bright
};

const SPRING_TUNING: ReverbTuning = ReverbTuning {
    // Uneven spacing, boomy, drip character
    comb_lengths: [983, 1097, 1289, 1429, 1531, 1667, 1811, 1949],
    allpass_lengths: [631, 491, 367, 251],
    diffusion: 0.45, // less diffuse = more "boing"
    damp_scale: 1.4, // darker
};

/// Comb length in samples for a length given at 44.1 kHz, at the current
/// sample rate (`ratio` = sr / 44100) and room size.
fn scaled_comb_len(len_44k: usize, ratio: f32, size_factor: f32) -> usize {
    (((len_44k as f32 * ratio) * size_factor) as usize).max(1)
}

/// Feedback that makes a comb of `len` samples fall 60 dB in `rt60` seconds.
///
/// One trip round the loop takes len / sr seconds and multiplies the level by
/// the feedback g, so after rt60 seconds the level is g^(rt60 * sr / len).
/// Setting that to 10^-3 (60 dB) gives g = 10^(-3 * len / (sr * rt60)). The
/// old exp(-3 * len / (sr * rt60)) only falls 26 dB in rt60, so tails ran
/// about 2.3x longer than the control said.
fn comb_feedback(len: usize, sr: f32, rt60: f32) -> f32 {
    let loop_sec = len as f32 / sr.max(1.0);
    10.0_f32
        .powf(-3.0 * loop_sec / rt60.max(MIN_RT60))
        .min(MAX_COMB_FEEDBACK)
}

/// Power gain of one comb on white noise, ignoring damping. A comb passes
/// x, g·x, g²·x … so the powers add up to 1 / (1 - g²).
fn comb_power(len: usize, sr: f32, rt60: f32) -> f32 {
    let g = comb_feedback(len, sr, rt60);
    1.0 / (1.0 - g * g)
}

struct CombFilter {
    buffer: Vec<f32>,
    idx: usize,
    feedback: f32,
    damp: OnePoleLP,
}

impl CombFilter {
    fn new(len: usize) -> Self {
        Self {
            buffer: vec![0.0; len.max(1)],
            idx: 0,
            feedback: 0.5,
            damp: OnePoleLP::new(),
        }
    }

    fn resize(&mut self, len: usize) {
        self.buffer.resize(len.max(1), 0.0);
        self.idx %= self.buffer.len();
    }

    fn len(&self) -> usize {
        self.buffer.len()
    }

    fn process(&mut self, input: f32) -> f32 {
        let out = self.buffer[self.idx];
        let filtered = self.damp.process(out);
        self.buffer[self.idx] = input + filtered * self.feedback;
        self.idx = (self.idx + 1) % self.buffer.len();
        out
    }

    fn reset(&mut self) {
        self.buffer.fill(0.0);
        self.damp.reset();
    }
}

struct AllpassFilter {
    buffer: Vec<f32>,
    idx: usize,
    feedback: f32,
}

impl AllpassFilter {
    fn new(len: usize, feedback: f32) -> Self {
        Self {
            buffer: vec![0.0; len.max(1)],
            idx: 0,
            feedback,
        }
    }

    fn resize(&mut self, len: usize) {
        self.buffer.resize(len.max(1), 0.0);
        self.idx %= self.buffer.len();
    }

    /// Schroeder allpass: v = x + g·v[n-M], y = v[n-M] - g·v. Its transfer
    /// function is (z^-M - g) / (1 - g·z^-M), whose gain is exactly 1 at every
    /// frequency for any |g| < 1, so the diffusion setting changes the texture
    /// and never the level.
    ///
    /// This used to be y = v[n-M] - x, the form Freeverb made popular. That is
    /// only near-allpass at g = 0.5 and boosts the peaks of its own comb
    /// response above it; four in series at Hall's 0.6 and Plate's 0.7 put the
    /// wet 14 to 17 dB over the dry.
    fn process(&mut self, input: f32) -> f32 {
        let delayed = self.buffer[self.idx];
        let v = input + delayed * self.feedback;
        self.buffer[self.idx] = v;
        self.idx = (self.idx + 1) % self.buffer.len();
        delayed - v * self.feedback
    }

    fn reset(&mut self) {
        self.buffer.fill(0.0);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReverbType {
    Room,
    Hall,
    Plate,
    Spring,
}

pub struct Reverb {
    combs_l: Vec<CombFilter>,
    combs_r: Vec<CombFilter>,
    allpass_l: Vec<AllpassFilter>,
    allpass_r: Vec<AllpassFilter>,
    predelay_buf: Vec<f32>,
    predelay_idx: usize,
    predelay_len: usize,
    sr: f32,
    size_factor: f32,
    reverb_type: ReverbType,
    frozen: bool,
    /// Output gain that puts this type at the level Room has at the same
    /// settings. Set by `set_params`.
    level: f32,
    // Pre-EQ for coloring the reverb input
    eq_lp: OnePoleLP,
    eq_hp_prev_input: f32,
    eq_hp_state: f32,
    eq_hp_freq: f32,
}

impl Reverb {
    pub fn new(sr: f32) -> Self {
        let tuning = &ROOM_TUNING;
        let ratio = sr / 44100.0;

        let combs_l: Vec<_> = tuning
            .comb_lengths
            .iter()
            .map(|&len| CombFilter::new((len as f32 * ratio) as usize))
            .collect();
        let combs_r: Vec<_> = tuning
            .comb_lengths
            .iter()
            .map(|&len| CombFilter::new(((len + STEREO_SPREAD) as f32 * ratio) as usize))
            .collect();
        let allpass_l: Vec<_> = tuning
            .allpass_lengths
            .iter()
            .map(|&len| AllpassFilter::new((len as f32 * ratio) as usize, tuning.diffusion))
            .collect();
        let allpass_r: Vec<_> = tuning
            .allpass_lengths
            .iter()
            .map(|&len| {
                AllpassFilter::new(
                    ((len + STEREO_SPREAD) as f32 * ratio) as usize,
                    tuning.diffusion,
                )
            })
            .collect();

        Self {
            combs_l,
            combs_r,
            allpass_l,
            allpass_r,
            predelay_buf: vec![0.0; MAX_PREDELAY_SAMPLES],
            predelay_idx: 0,
            predelay_len: 0,
            sr,
            size_factor: 1.0,
            reverb_type: ReverbType::Room,
            frozen: false,
            level: 1.0,
            eq_lp: OnePoleLP::new(),
            eq_hp_prev_input: 0.0,
            eq_hp_state: 0.0,
            eq_hp_freq: 20.0,
        }
    }

    pub fn set_sample_rate(&mut self, sr: f32) {
        self.sr = sr;
        self.rebuild_for_type();
        self.predelay_buf.resize((sr as usize).max(1), 0.0);
    }

    /// Switch reverb algorithm type — rebuilds comb/allpass buffers.
    pub fn set_type(&mut self, rt: ReverbType) {
        if rt != self.reverb_type {
            self.reverb_type = rt;
            self.rebuild_for_type();
        }
    }

    /// Enable/disable freeze (infinite sustain).
    pub fn set_freeze(&mut self, freeze: bool) {
        self.frozen = freeze;
    }

    #[allow(dead_code)]
    pub fn is_frozen(&self) -> bool {
        self.frozen
    }

    fn get_tuning(&self) -> &'static ReverbTuning {
        match self.reverb_type {
            ReverbType::Room => &ROOM_TUNING,
            ReverbType::Hall => &HALL_TUNING,
            ReverbType::Plate => &PLATE_TUNING,
            ReverbType::Spring => &SPRING_TUNING,
        }
    }

    fn rebuild_for_type(&mut self) {
        let tuning = self.get_tuning();
        let ratio = self.sr / 44100.0;

        // Resize existing combs to new tuning lengths
        for (i, comb) in self.combs_l.iter_mut().enumerate() {
            comb.resize(scaled_comb_len(
                tuning.comb_lengths[i],
                ratio,
                self.size_factor,
            ));
        }
        for (i, comb) in self.combs_r.iter_mut().enumerate() {
            comb.resize(scaled_comb_len(
                tuning.comb_lengths[i] + STEREO_SPREAD,
                ratio,
                self.size_factor,
            ));
        }
        for (i, ap) in self.allpass_l.iter_mut().enumerate() {
            ap.resize((tuning.allpass_lengths[i] as f32 * ratio) as usize);
            ap.feedback = tuning.diffusion;
        }
        for (i, ap) in self.allpass_r.iter_mut().enumerate() {
            ap.resize(((tuning.allpass_lengths[i] + STEREO_SPREAD) as f32 * ratio) as usize);
            ap.feedback = tuning.diffusion;
        }
    }

    /// Update reverb parameters.
    /// - `size`: 0–100 (room size)
    /// - `decay`: 0.1–20.0 seconds, the RT60 in every type
    /// - `damp`: 0–100 (damping percentage)
    /// - `predelay_ms`: 0–1000 ms
    pub fn set_params(&mut self, size: f32, decay: f32, damp: f32, predelay_ms: f32) {
        let tuning = self.get_tuning();
        let ratio = self.sr / 44100.0;
        self.size_factor = 0.5 + (size / 100.0) * 1.5; // 0.5x – 2.0x

        // Resize comb filters based on size
        for (i, comb) in self.combs_l.iter_mut().enumerate() {
            comb.resize(scaled_comb_len(
                tuning.comb_lengths[i],
                ratio,
                self.size_factor,
            ));
        }
        for (i, comb) in self.combs_r.iter_mut().enumerate() {
            comb.resize(scaled_comb_len(
                tuning.comb_lengths[i] + STEREO_SPREAD,
                ratio,
                self.size_factor,
            ));
        }

        // Feedback from decay time. Each comb gets the feedback that takes its
        // own loop down 60 dB in `decay` seconds, from its real length at the
        // real sample rate, so every comb decays together and the tail is the
        // same length at 44.1 and 96 kHz. This used one feedback for all eight
        // from an average length counted at 44.1 kHz but divided by the real
        // sample rate, which made tails longer again at 96 kHz.
        let sr = self.sr.max(1.0);
        let rt60 = decay.max(MIN_RT60);

        // Damping frequency (damp 0=bright, 100=dark), scaled by type
        let effective_damp = (damp * tuning.damp_scale).min(100.0);
        let damp_freq = 20000.0 * (1.0 - effective_damp / 100.0 * 0.95).max(0.05);

        for comb in self.combs_l.iter_mut().chain(self.combs_r.iter_mut()) {
            comb.feedback = if self.frozen {
                0.999 // near-infinite sustain
            } else {
                comb_feedback(comb.len(), sr, rt60)
            };
            comb.damp.set_freq(damp_freq, self.sr);
        }

        // Level match against Room. Longer combs recirculate less often in the
        // same decay time, so a type's comb bank is louder or quieter than
        // Room's by the ratio of their power gains; scale it back to Room's.
        // The right bank is the left one plus a fixed spread, so the left
        // bank stands for both. Worked out from the decay setting rather than
        // the frozen feedback, so freeze does not jump the level.
        let room_power: f32 = ROOM_TUNING
            .comb_lengths
            .iter()
            .map(|&len| comb_power(scaled_comb_len(len, ratio, self.size_factor), sr, rt60))
            .sum();
        let own_power: f32 = self
            .combs_l
            .iter()
            .map(|comb| comb_power(comb.len(), sr, rt60))
            .sum();
        // Each comb's power gain is at least 1, so own_power is at least 8.
        self.level = (room_power / own_power.max(1.0)).sqrt();

        // Pre-delay
        self.predelay_len =
            ((predelay_ms / 1000.0 * self.sr) as usize).min(self.predelay_buf.len() - 1);
    }

    /// Set pre-EQ frequencies for coloring the reverb input.
    pub fn set_eq(&mut self, hp_freq: f32, lp_freq: f32) {
        self.eq_hp_freq = hp_freq.max(20.0);
        self.eq_lp.set_freq(lp_freq.min(20000.0), self.sr);
    }

    /// Process a mono input into stereo reverb output (wet only).
    /// Returns (left_wet, right_wet).
    pub fn process(&mut self, input: f32, width: f32) -> (f32, f32) {
        // In freeze mode, cut the input to sustain existing tail
        let effective_input = if self.frozen { 0.0 } else { input };

        // Pre-EQ: one-pole HP then LP
        let hp_rc = 1.0 / (std::f32::consts::PI * 2.0 * self.eq_hp_freq.max(1.0));
        let hp_dt = 1.0 / self.sr;
        let hp_alpha = hp_rc / (hp_rc + hp_dt);
        self.eq_hp_state = hp_alpha * (self.eq_hp_state + effective_input - self.eq_hp_prev_input);
        self.eq_hp_prev_input = effective_input;
        let eq_input = self.eq_lp.process(self.eq_hp_state);

        // Pre-delay
        let pd_read = (self.predelay_idx + self.predelay_buf.len() - self.predelay_len)
            % self.predelay_buf.len();
        let delayed_input = self.predelay_buf[pd_read];
        self.predelay_buf[self.predelay_idx] = eq_input;
        self.predelay_idx = (self.predelay_idx + 1) % self.predelay_buf.len();

        // Parallel comb filters
        let mut out_l = 0.0_f32;
        let mut out_r = 0.0_f32;
        for comb in self.combs_l.iter_mut() {
            out_l += comb.process(delayed_input);
        }
        for comb in self.combs_r.iter_mut() {
            out_r += comb.process(delayed_input);
        }
        let gain = self.level / NUM_COMBS as f32;
        out_l *= gain;
        out_r *= gain;

        // Series allpass filters
        for ap in self.allpass_l.iter_mut() {
            out_l = ap.process(out_l);
        }
        for ap in self.allpass_r.iter_mut() {
            out_r = ap.process(out_r);
        }

        // Stereo width (0=mono, 100=normal, 200=extra wide)
        let w = (width / 100.0).clamp(0.0, 2.0);
        let mid = (out_l + out_r) * 0.5;
        let side = (out_l - out_r) * 0.5;
        let final_l = mid + side * w;
        let final_r = mid - side * w;

        (final_l, final_r)
    }

    pub fn reset(&mut self) {
        for comb in self.combs_l.iter_mut().chain(self.combs_r.iter_mut()) {
            comb.reset();
        }
        for ap in self.allpass_l.iter_mut().chain(self.allpass_r.iter_mut()) {
            ap.reset();
        }
        self.predelay_buf.fill(0.0);
        self.predelay_idx = 0;
        self.eq_hp_prev_input = 0.0;
        self.eq_hp_state = 0.0;
        self.eq_lp.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comb_feedback_falls_60_db_in_the_rt60() {
        for sr in [44_100.0_f32, 96_000.0] {
            // The shortest, a middling and the longest comb the plug-in builds:
            // Spring at Size 0, Room at Size 65, Hall at Size 100.
            for loop_sec in [0.0112_f32, 0.046, 0.11] {
                let len = (loop_sec * sr) as usize;
                for rt60 in [0.5_f32, 2.4, 6.0, 20.0] {
                    let g = comb_feedback(len, sr, rt60);
                    let passes = rt60 * sr / len as f32;
                    let db = 20.0 * g.powf(passes).log10();
                    assert!(
                        (db + 60.0).abs() < 0.1,
                        "len {len} at {sr} Hz, rt60 {rt60} s: {db:.2} dB after rt60"
                    );
                }
            }
        }
    }

    /// The ceiling on the feedback is a guard, not part of the decay: no comb
    /// of any type, at any size, sample rate or Decay setting, reaches it.
    #[test]
    fn the_feedback_ceiling_is_never_reached_in_range() {
        for tuning in [&ROOM_TUNING, &HALL_TUNING, &PLATE_TUNING, &SPRING_TUNING] {
            for sr in [44_100.0_f32, 48_000.0, 96_000.0, 192_000.0] {
                for size_factor in [0.5_f32, 2.0] {
                    for &len in tuning.comb_lengths.iter() {
                        let len = scaled_comb_len(len, sr / 44_100.0, size_factor);
                        let g = comb_feedback(len, sr, 20.0);
                        assert!(
                            g < MAX_COMB_FEEDBACK,
                            "comb of {len} at {sr} Hz needs {g}, at the ceiling"
                        );
                    }
                }
            }
        }
    }

    /// An allpass passes all the energy of an impulse, no more and no less,
    /// whatever its coefficient. The old form passed 1 + 1 / (1 - g²) of it:
    /// +3.7 dB a stage at 0.5, +4.7 dB at 0.7.
    #[test]
    fn allpass_has_unity_gain_at_every_coefficient() {
        for g in [0.45_f32, 0.5, 0.6, 0.7] {
            let mut ap = AllpassFilter::new(97, g);
            let mut energy = 0.0_f64;
            for i in 0..97 * 400 {
                let y = ap.process(if i == 0 { 1.0 } else { 0.0 });
                energy += (y as f64).powi(2);
            }
            assert!(
                (energy - 1.0).abs() < 1.0e-4,
                "g {g}: impulse energy {energy}, not 1"
            );

            // A sine on one of its own comb peaks, where the old form boosted
            // by g / (1 - g), comes out at the level it went in.
            let mut ap = AllpassFilter::new(100, g);
            let f = 4.0 / 100.0; // cycles per sample: 4 periods per loop
            let (mut e_in, mut e_out) = (0.0_f64, 0.0_f64);
            for i in 0..100_000 {
                let x = (std::f32::consts::TAU * f * i as f32).sin();
                let y = ap.process(x);
                if i >= 50_000 {
                    e_in += (x as f64).powi(2);
                    e_out += (y as f64).powi(2);
                }
            }
            let db = 10.0 * (e_out / e_in).log10();
            assert!(db.abs() < 0.05, "g {g}: {db:+.2} dB at a comb peak");
        }
    }
}

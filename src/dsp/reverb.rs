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

/// Per-type comb filter delay lengths (samples at 44.1 kHz).
/// Different lengths create different resonance patterns → different "room" characters.
struct ReverbTuning {
    comb_lengths: [usize; NUM_COMBS],
    allpass_lengths: [usize; NUM_ALLPASS],
    diffusion: f32,      // allpass feedback coefficient (0.3–0.7)
    damp_scale: f32,     // multiplier for damping (higher = darker)
    decay_scale: f32,    // multiplier for decay time
    density: f32,        // scales comb filter count contribution
}

const ROOM_TUNING: ReverbTuning = ReverbTuning {
    comb_lengths: [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617],
    allpass_lengths: [556, 441, 341, 225],
    diffusion: 0.5,
    damp_scale: 1.0,
    decay_scale: 1.0,
    density: 1.0,
};

const HALL_TUNING: ReverbTuning = ReverbTuning {
    // Longer, more spread out → bigger space
    comb_lengths: [1557, 1617, 1733, 1861, 1993, 2131, 2269, 2399],
    allpass_lengths: [677, 557, 433, 311],
    diffusion: 0.6,
    damp_scale: 0.7,   // brighter tails
    decay_scale: 1.5,   // longer natural decay
    density: 0.85,
};

const PLATE_TUNING: ReverbTuning = ReverbTuning {
    // Dense, bright, metallic character
    comb_lengths: [1051, 1123, 1187, 1259, 1321, 1381, 1447, 1511],
    allpass_lengths: [487, 379, 283, 197],
    diffusion: 0.7,     // high diffusion = smooth, dense
    damp_scale: 0.5,    // very bright
    decay_scale: 1.2,
    density: 1.2,       // denser reflections
};

const SPRING_TUNING: ReverbTuning = ReverbTuning {
    // Uneven spacing, boomy, drip character
    comb_lengths: [983, 1097, 1289, 1429, 1531, 1667, 1811, 1949],
    allpass_lengths: [631, 491, 367, 251],
    diffusion: 0.45,    // less diffuse = more "boing"
    damp_scale: 1.4,    // darker
    decay_scale: 0.8,   // shorter
    density: 0.9,
};

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

    fn process(&mut self, input: f32) -> f32 {
        let delayed = self.buffer[self.idx];
        let output = delayed - input;
        self.buffer[self.idx] = input + delayed * self.feedback;
        self.idx = (self.idx + 1) % self.buffer.len();
        output
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

        let combs_l: Vec<_> = tuning.comb_lengths
            .iter()
            .map(|&len| CombFilter::new((len as f32 * ratio) as usize))
            .collect();
        let combs_r: Vec<_> = tuning.comb_lengths
            .iter()
            .map(|&len| CombFilter::new(((len + STEREO_SPREAD) as f32 * ratio) as usize))
            .collect();
        let allpass_l: Vec<_> = tuning.allpass_lengths
            .iter()
            .map(|&len| AllpassFilter::new((len as f32 * ratio) as usize, tuning.diffusion))
            .collect();
        let allpass_r: Vec<_> = tuning.allpass_lengths
            .iter()
            .map(|&len| AllpassFilter::new(((len + STEREO_SPREAD) as f32 * ratio) as usize, tuning.diffusion))
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
            let new_len = ((tuning.comb_lengths[i] as f32 * ratio) * self.size_factor) as usize;
            comb.resize(new_len.max(1));
        }
        for (i, comb) in self.combs_r.iter_mut().enumerate() {
            let new_len = (((tuning.comb_lengths[i] + STEREO_SPREAD) as f32 * ratio) * self.size_factor) as usize;
            comb.resize(new_len.max(1));
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
    /// - `decay`: 0.1–20.0 seconds (maps to feedback)
    /// - `damp`: 0–100 (damping percentage)
    /// - `predelay_ms`: 0–1000 ms
    pub fn set_params(&mut self, size: f32, decay: f32, damp: f32, predelay_ms: f32) {
        let tuning = self.get_tuning();
        let ratio = self.sr / 44100.0;
        self.size_factor = 0.5 + (size / 100.0) * 1.5; // 0.5x – 2.0x

        // Resize comb filters based on size
        for (i, comb) in self.combs_l.iter_mut().enumerate() {
            let new_len = ((tuning.comb_lengths[i] as f32 * ratio) * self.size_factor) as usize;
            comb.resize(new_len.max(1));
        }
        for (i, comb) in self.combs_r.iter_mut().enumerate() {
            let new_len = (((tuning.comb_lengths[i] + STEREO_SPREAD) as f32 * ratio) * self.size_factor) as usize;
            comb.resize(new_len.max(1));
        }

        // Feedback from decay time (scaled by type)
        let avg_comb_len = tuning.comb_lengths.iter().sum::<usize>() as f32 / NUM_COMBS as f32;
        let avg_delay_sec = (avg_comb_len * self.size_factor) / self.sr;
        let target_rt60 = decay * tuning.decay_scale;
        let feedback = if self.frozen {
            0.999 // near-infinite sustain
        } else if avg_delay_sec > 0.0 {
            (-3.0 * avg_delay_sec / target_rt60).exp().clamp(0.0, 0.98)
        } else {
            0.5
        };

        // Damping frequency (damp 0=bright, 100=dark), scaled by type
        let effective_damp = (damp * tuning.damp_scale).min(100.0);
        let damp_freq = 20000.0 * (1.0 - effective_damp / 100.0 * 0.95).max(0.05);

        for comb in self.combs_l.iter_mut().chain(self.combs_r.iter_mut()) {
            comb.feedback = feedback;
            comb.damp.set_freq(damp_freq, self.sr);
        }

        // Pre-delay
        self.predelay_len = ((predelay_ms / 1000.0 * self.sr) as usize)
            .min(self.predelay_buf.len() - 1);
    }

    /// Set pre-EQ frequencies for coloring the reverb input.
    pub fn set_eq(&mut self, hp_freq: f32, lp_freq: f32) {
        self.eq_hp_freq = hp_freq.max(20.0);
        self.eq_lp.set_freq(lp_freq.min(20000.0), self.sr);
    }

    /// Process a mono input into stereo reverb output (wet only).
    /// Returns (left_wet, right_wet).
    pub fn process(&mut self, input: f32, width: f32) -> (f32, f32) {
        let tuning = self.get_tuning();

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
        out_l /= NUM_COMBS as f32 * tuning.density;
        out_r /= NUM_COMBS as f32 * tuning.density;

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

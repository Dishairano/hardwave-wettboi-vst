//! Algorithmic stereo reverb based on Freeverb (Schroeder–Moorer).
//!
//! Four parallel comb filters per channel + two series allpass filters,
//! with damping, size control, and stereo width spread.

use super::filters::OnePoleLP;

const NUM_COMBS: usize = 8;
const NUM_ALLPASS: usize = 4;

// Comb filter delay lengths (in samples at 44.1 kHz) — left channel.
// Right channel offsets by a stereo spread constant.
const COMB_LENGTHS: [usize; NUM_COMBS] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
const ALLPASS_LENGTHS: [usize; NUM_ALLPASS] = [556, 441, 341, 225];
const STEREO_SPREAD: usize = 23;
const MAX_PREDELAY_SAMPLES: usize = 44100; // ~1s at 44.1k

struct CombFilter {
    buffer: Vec<f32>,
    idx: usize,
    feedback: f32,
    damp: OnePoleLP,
}

impl CombFilter {
    fn new(len: usize) -> Self {
        Self {
            buffer: vec![0.0; len],
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
}

impl AllpassFilter {
    fn new(len: usize) -> Self {
        Self {
            buffer: vec![0.0; len],
            idx: 0,
        }
    }

    fn resize(&mut self, len: usize) {
        self.buffer.resize(len.max(1), 0.0);
        self.idx %= self.buffer.len();
    }

    fn process(&mut self, input: f32) -> f32 {
        let delayed = self.buffer[self.idx];
        let output = delayed - input;
        self.buffer[self.idx] = input + delayed * 0.5;
        self.idx = (self.idx + 1) % self.buffer.len();
        output
    }

    fn reset(&mut self) {
        self.buffer.fill(0.0);
    }
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
}

impl Reverb {
    pub fn new(sr: f32) -> Self {
        let ratio = sr / 44100.0;
        let combs_l: Vec<_> = COMB_LENGTHS
            .iter()
            .map(|&len| CombFilter::new((len as f32 * ratio) as usize))
            .collect();
        let combs_r: Vec<_> = COMB_LENGTHS
            .iter()
            .map(|&len| CombFilter::new(((len + STEREO_SPREAD) as f32 * ratio) as usize))
            .collect();
        let allpass_l: Vec<_> = ALLPASS_LENGTHS
            .iter()
            .map(|&len| AllpassFilter::new((len as f32 * ratio) as usize))
            .collect();
        let allpass_r: Vec<_> = ALLPASS_LENGTHS
            .iter()
            .map(|&len| AllpassFilter::new(((len + STEREO_SPREAD) as f32 * ratio) as usize))
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
        }
    }

    pub fn set_sample_rate(&mut self, sr: f32) {
        self.sr = sr;
        let ratio = sr / 44100.0;
        for (i, comb) in self.combs_l.iter_mut().enumerate() {
            comb.resize(((COMB_LENGTHS[i] as f32 * ratio) * self.size_factor) as usize);
        }
        for (i, comb) in self.combs_r.iter_mut().enumerate() {
            comb.resize((((COMB_LENGTHS[i] + STEREO_SPREAD) as f32 * ratio) * self.size_factor) as usize);
        }
        for (i, ap) in self.allpass_l.iter_mut().enumerate() {
            ap.resize((ALLPASS_LENGTHS[i] as f32 * ratio) as usize);
        }
        for (i, ap) in self.allpass_r.iter_mut().enumerate() {
            ap.resize(((ALLPASS_LENGTHS[i] + STEREO_SPREAD) as f32 * ratio) as usize);
        }
        self.predelay_buf.resize((sr as usize).max(1), 0.0);
    }

    /// Update reverb parameters.
    /// - `size`: 0–100 (room size)
    /// - `decay`: 0.1–20.0 seconds (maps to feedback)
    /// - `damp`: 0–100 (damping percentage)
    /// - `predelay_ms`: 0–1000 ms
    pub fn set_params(&mut self, size: f32, decay: f32, damp: f32, predelay_ms: f32) {
        let ratio = self.sr / 44100.0;
        self.size_factor = 0.5 + (size / 100.0) * 1.5; // 0.5x – 2.0x

        // Resize comb filters based on size
        for (i, comb) in self.combs_l.iter_mut().enumerate() {
            let new_len = ((COMB_LENGTHS[i] as f32 * ratio) * self.size_factor) as usize;
            comb.resize(new_len.max(1));
        }
        for (i, comb) in self.combs_r.iter_mut().enumerate() {
            let new_len = (((COMB_LENGTHS[i] + STEREO_SPREAD) as f32 * ratio) * self.size_factor) as usize;
            comb.resize(new_len.max(1));
        }

        // Feedback from decay time
        let avg_comb_len = COMB_LENGTHS.iter().sum::<usize>() as f32 / NUM_COMBS as f32;
        let avg_delay_sec = (avg_comb_len * self.size_factor) / self.sr;
        let target_rt60 = decay;
        let feedback = if avg_delay_sec > 0.0 {
            (-3.0 * avg_delay_sec / target_rt60).exp().clamp(0.0, 0.98)
        } else {
            0.5
        };

        // Damping frequency (damp 0=bright, 100=dark)
        let damp_freq = 20000.0 * (1.0 - damp / 100.0 * 0.95).max(0.05);

        for comb in self.combs_l.iter_mut().chain(self.combs_r.iter_mut()) {
            comb.feedback = feedback;
            comb.damp.set_freq(damp_freq, self.sr);
        }

        // Pre-delay
        self.predelay_len = ((predelay_ms / 1000.0 * self.sr) as usize)
            .min(self.predelay_buf.len() - 1);
    }

    /// Process a mono input into stereo reverb output (wet only).
    /// Returns (left_wet, right_wet).
    pub fn process(&mut self, input: f32, width: f32) -> (f32, f32) {
        // Pre-delay
        let pd_read = (self.predelay_idx + self.predelay_buf.len() - self.predelay_len)
            % self.predelay_buf.len();
        let delayed_input = self.predelay_buf[pd_read];
        self.predelay_buf[self.predelay_idx] = input;
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
        out_l /= NUM_COMBS as f32;
        out_r /= NUM_COMBS as f32;

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
    }
}

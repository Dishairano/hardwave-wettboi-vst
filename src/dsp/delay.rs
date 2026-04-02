//! Stereo delay with tempo sync, ping-pong mode, feedback filtering,
//! and time modulation (chorus/flutter).

use super::filters::OnePoleSVF;

const MAX_DELAY_SAMPLES: usize = 88200 * 4; // ~4s at 88.2k

pub struct StereoDelay {
    buf_l: Vec<f32>,
    buf_r: Vec<f32>,
    write_idx: usize,
    delay_samples_l: f32,
    delay_samples_r: f32,
    feedback: f32,
    ping_pong: bool,
    filter_l: OnePoleSVF,
    filter_r: OnePoleSVF,
    hp_freq: f32,
    lp_freq: f32,
    sr: f32,
    // Modulation
    mod_phase: f64,
    mod_rate: f32,    // Hz
    mod_depth: f32,   // 0–100 (maps to samples of modulation)
    // Saturation on feedback path
    saturation: f32,  // 0–100
}

impl StereoDelay {
    pub fn new(sr: f32) -> Self {
        Self {
            buf_l: vec![0.0; MAX_DELAY_SAMPLES],
            buf_r: vec![0.0; MAX_DELAY_SAMPLES],
            write_idx: 0,
            delay_samples_l: (400.0 / 1000.0 * sr),
            delay_samples_r: (600.0 / 1000.0 * sr),
            feedback: 0.35,
            ping_pong: true,
            filter_l: OnePoleSVF::new(sr),
            filter_r: OnePoleSVF::new(sr),
            hp_freq: 120.0,
            lp_freq: 8000.0,
            sr,
            mod_phase: 0.0,
            mod_rate: 0.5,
            mod_depth: 0.0,
            saturation: 0.0,
        }
    }

    pub fn set_sample_rate(&mut self, sr: f32) {
        self.sr = sr;
        self.filter_l.set_sample_rate(sr);
        self.filter_r.set_sample_rate(sr);
        self.buf_l.resize(MAX_DELAY_SAMPLES, 0.0);
        self.buf_r.resize(MAX_DELAY_SAMPLES, 0.0);
    }

    /// Set delay times in milliseconds.
    pub fn set_time_ms(&mut self, time_l_ms: f32, time_r_ms: f32) {
        self.delay_samples_l = (time_l_ms / 1000.0 * self.sr).clamp(1.0, (MAX_DELAY_SAMPLES - 1) as f32);
        self.delay_samples_r = (time_r_ms / 1000.0 * self.sr).clamp(1.0, (MAX_DELAY_SAMPLES - 1) as f32);
    }

    /// Set delay times from BPM and note division (in beats).
    pub fn set_time_sync(&mut self, bpm: f32, beats_l: f32, beats_r: f32) {
        if bpm <= 0.0 {
            return;
        }
        let beat_sec = 60.0 / bpm;
        let ms_l = beats_l * beat_sec * 1000.0;
        let ms_r = beats_r * beat_sec * 1000.0;
        self.set_time_ms(ms_l, ms_r);
    }

    pub fn set_feedback(&mut self, fb_pct: f32) {
        self.feedback = (fb_pct / 100.0).clamp(0.0, 0.95);
    }

    pub fn set_filter(&mut self, hp: f32, lp: f32) {
        self.hp_freq = hp;
        self.lp_freq = lp;
    }

    pub fn set_ping_pong(&mut self, enabled: bool) {
        self.ping_pong = enabled;
    }

    /// Set delay time modulation parameters.
    /// - `rate`: modulation speed in Hz (0.01–10)
    /// - `depth`: modulation depth 0–100 (percentage of max mod range)
    pub fn set_modulation(&mut self, rate: f32, depth: f32) {
        self.mod_rate = rate.clamp(0.01, 10.0);
        self.mod_depth = depth.clamp(0.0, 100.0);
    }

    /// Set feedback saturation amount (0–100).
    pub fn set_saturation(&mut self, amount: f32) {
        self.saturation = amount.clamp(0.0, 100.0);
    }

    /// Process stereo input. Returns (wet_l, wet_r).
    pub fn process(&mut self, in_l: f32, in_r: f32) -> (f32, f32) {
        // Advance modulation LFO
        let mod_inc = self.mod_rate as f64 / self.sr as f64;
        self.mod_phase += mod_inc;
        if self.mod_phase >= 1.0 {
            self.mod_phase -= 1.0;
        }

        // Modulation offset in samples (max ±40 samples at depth=100)
        let mod_val = (self.mod_phase as f32 * std::f32::consts::PI * 2.0).sin();
        let mod_samples = mod_val * (self.mod_depth / 100.0) * 40.0;

        // Apply modulation to delay times (L gets positive, R gets negative for stereo width)
        let mod_delay_l = (self.delay_samples_l + mod_samples).clamp(1.0, (MAX_DELAY_SAMPLES - 1) as f32);
        let mod_delay_r = (self.delay_samples_r - mod_samples * 0.7).clamp(1.0, (MAX_DELAY_SAMPLES - 1) as f32);

        // Read from delay lines (linear interpolation)
        let read_l = self.read_interpolated(&self.buf_l, mod_delay_l);
        let read_r = self.read_interpolated(&self.buf_r, mod_delay_r);

        // Filter the feedback
        let mut filt_l = self.filter_l.process(read_l, self.hp_freq, self.lp_freq);
        let mut filt_r = self.filter_r.process(read_r, self.hp_freq, self.lp_freq);

        // Saturation on feedback path (soft tanh)
        if self.saturation > 0.0 {
            let drive = 1.0 + self.saturation / 100.0 * 4.0; // 1x–5x drive
            filt_l = (filt_l * drive).tanh() / drive.tanh();
            filt_r = (filt_r * drive).tanh() / drive.tanh();
        }

        // Write to delay line
        if self.ping_pong {
            // Ping-pong: left input + right feedback → left buffer, and vice versa
            self.buf_l[self.write_idx] = in_l + filt_r * self.feedback;
            self.buf_r[self.write_idx] = in_r + filt_l * self.feedback;
        } else {
            self.buf_l[self.write_idx] = in_l + filt_l * self.feedback;
            self.buf_r[self.write_idx] = in_r + filt_r * self.feedback;
        }

        self.write_idx = (self.write_idx + 1) % self.buf_l.len();

        (read_l, read_r)
    }

    fn read_interpolated(&self, buf: &[f32], delay_samples: f32) -> f32 {
        let buf_len = buf.len();
        let int_delay = delay_samples as usize;
        let frac = delay_samples - int_delay as f32;

        let idx0 = (self.write_idx + buf_len - int_delay) % buf_len;
        let idx1 = (self.write_idx + buf_len - int_delay - 1) % buf_len;

        buf[idx0] * (1.0 - frac) + buf[idx1] * frac
    }

    pub fn reset(&mut self) {
        self.buf_l.fill(0.0);
        self.buf_r.fill(0.0);
        self.write_idx = 0;
        self.filter_l.reset();
        self.filter_r.reset();
        self.mod_phase = 0.0;
    }
}

//! Stereo delay with tempo sync, ping-pong mode, and feedback filtering.

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

    /// Process stereo input. Returns (wet_l, wet_r).
    pub fn process(&mut self, in_l: f32, in_r: f32) -> (f32, f32) {
        let buf_len = self.buf_l.len();

        // Read from delay lines (linear interpolation)
        let read_l = self.read_interpolated(&self.buf_l, self.delay_samples_l);
        let read_r = self.read_interpolated(&self.buf_r, self.delay_samples_r);

        // Filter the feedback
        let filt_l = self.filter_l.process(read_l, self.hp_freq, self.lp_freq);
        let filt_r = self.filter_r.process(read_r, self.hp_freq, self.lp_freq);

        // Write to delay line
        if self.ping_pong {
            // Ping-pong: left input + right feedback → left buffer, and vice versa
            self.buf_l[self.write_idx] = in_l + filt_r * self.feedback;
            self.buf_r[self.write_idx] = in_r + filt_l * self.feedback;
        } else {
            self.buf_l[self.write_idx] = in_l + filt_l * self.feedback;
            self.buf_r[self.write_idx] = in_r + filt_r * self.feedback;
        }

        self.write_idx = (self.write_idx + 1) % buf_len;

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
    }
}

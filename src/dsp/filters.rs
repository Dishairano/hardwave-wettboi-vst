//! Simple filter primitives used by reverb, delay, and sidechain.

use std::f32::consts::PI;

/// One-pole lowpass for damping / smoothing.
pub struct OnePoleLP {
    coeff: f32,
    state: f32,
}

impl OnePoleLP {
    pub fn new() -> Self {
        Self {
            coeff: 0.5,
            state: 0.0,
        }
    }

    pub fn set_freq(&mut self, freq: f32, sr: f32) {
        let w = (2.0 * PI * freq / sr).min(PI - 0.01);
        self.coeff = w.sin() / (1.0 + w.cos());
    }

    pub fn process(&mut self, input: f32) -> f32 {
        self.state += self.coeff * (input - self.state);
        self.state
    }

    pub fn reset(&mut self) {
        self.state = 0.0;
    }
}

/// Serial highpass → lowpass filter for delay feedback filtering.
pub struct OnePoleSVF {
    prev_input: f32,
    hp_state: f32,
    lp_state: f32,
    sr: f32,
}

impl OnePoleSVF {
    pub fn new(sr: f32) -> Self {
        Self {
            prev_input: 0.0,
            hp_state: 0.0,
            lp_state: 0.0,
            sr,
        }
    }

    pub fn set_sample_rate(&mut self, sr: f32) {
        self.sr = sr;
    }

    /// Process through a highpass at `hp_freq` then lowpass at `lp_freq`.
    pub fn process(&mut self, input: f32, hp_freq: f32, lp_freq: f32) -> f32 {
        // One-pole highpass: y[n] = alpha * (y[n-1] + x[n] - x[n-1])
        let hp_rc = 1.0 / (2.0 * PI * hp_freq.max(1.0));
        let hp_dt = 1.0 / self.sr;
        let hp_alpha = hp_rc / (hp_rc + hp_dt);
        self.hp_state = hp_alpha * (self.hp_state + input - self.prev_input);
        self.prev_input = input;

        // One-pole lowpass on HP output
        let lp_rc = 1.0 / (2.0 * PI * lp_freq.max(1.0));
        let lp_dt = 1.0 / self.sr;
        let lp_alpha = lp_dt / (lp_rc + lp_dt);
        self.lp_state += lp_alpha * (self.hp_state - self.lp_state);

        self.lp_state
    }

    pub fn reset(&mut self) {
        self.prev_input = 0.0;
        self.hp_state = 0.0;
        self.lp_state = 0.0;
    }
}

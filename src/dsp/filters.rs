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

/// State-variable filter (HP/LP) for delay feedback filtering.
pub struct OnePoleSVF {
    lp: f32,
    hp: f32,
    sr: f32,
}

impl OnePoleSVF {
    pub fn new(sr: f32) -> Self {
        Self {
            lp: 0.0,
            hp: 0.0,
            sr,
        }
    }

    pub fn set_sample_rate(&mut self, sr: f32) {
        self.sr = sr;
    }

    /// Process through a highpass at `hp_freq` then lowpass at `lp_freq`.
    pub fn process(&mut self, input: f32, hp_freq: f32, lp_freq: f32) -> f32 {
        // Simple one-pole HP
        let hp_coeff = 1.0 - (2.0 * PI * hp_freq / self.sr).min(PI - 0.01).sin()
            / (1.0 + (2.0 * PI * hp_freq / self.sr).min(PI - 0.01).cos());
        self.hp = hp_coeff * (self.hp + input - self.lp);
        // Simple one-pole LP
        let lp_coeff = (2.0 * PI * lp_freq / self.sr).min(PI - 0.01).sin()
            / (1.0 + (2.0 * PI * lp_freq / self.sr).min(PI - 0.01).cos());
        self.lp += lp_coeff * (self.hp - self.lp);
        self.lp
    }

    pub fn reset(&mut self) {
        self.lp = 0.0;
        self.hp = 0.0;
    }
}

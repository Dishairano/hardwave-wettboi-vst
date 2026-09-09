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
        // Stable one-pole IIR coefficient: alpha = 1 - exp(-2π·f/sr).
        // The previous formula sin(w)/(1+cos(w)) = tan(w/2) blew past 1.0 at
        // typical cutoffs (e.g. 18 kHz @ 44.1 kHz → coeff ≈ 3.34), making
        // `state += coeff*(input-state)` divergent. That fed NaN through the
        // reverb combs and pre-EQ LP, which is why beta testers reported
        // "no audio" after inserting WettBoi: NaN samples landed in the
        // output buffer and the host muted the channel.
        let f = freq.max(1.0);
        let s = sr.max(1.0);
        let alpha = 1.0 - (-2.0 * PI * f / s).exp();
        self.coeff = alpha.clamp(0.0, 1.0);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_pole_lp_basic_behavior() {
        let mut lp = OnePoleLP::new();
        // Set a reasonable cutoff frequency.
        lp.set_freq(1000.0, 48000.0);
        // Feed a constant input and ensure the output converges towards the input value.
        let mut out = 0.0;
        for _ in 0..500 {
            out = lp.process(1.0);
        }
        // After many iterations the low‑pass should be very close to the input.
        assert!((out - 1.0).abs() < 1e-3, "output not close to 1.0: {}", out);
        // Reset should clear the internal state.
        lp.reset();
        // After reset, processing zero should give zero.
        let zero_out = lp.process(0.0);
        assert!((zero_out).abs() < 1e-6, "output after reset not zero: {}", zero_out);
    }

    #[test]
    fn one_pole_lp_no_nan_on_high_freq() {
        let mut lp = OnePoleLP::new();
        // Use a very high cutoff that previously caused NaNs.
        lp.set_freq(20000.0, 44100.0);
        // Process a few samples and ensure the result stays finite.
        for i in 0..10 {
            let out = lp.process(i as f32 * 0.1);
            assert!(out.is_finite(), "output became non‑finite at step {}", i);
        }
    }

    #[test]
    fn one_pole_svf_zero_input_and_reset() {
        let mut svf = OnePoleSVF::new(48000.0);
        // Zero input should produce zero output regardless of frequencies.
        let out = svf.process(0.0, 500.0, 2000.0);
        assert!((out).abs() < 1e-6);
        // Process a non‑zero sample to change internal state.
        let _ = svf.process(1.0, 500.0, 2000.0);
        // Reset should bring the state back to zero.
        svf.reset();
        let out_after_reset = svf.process(0.0, 500.0, 2000.0);
        assert!((out_after_reset).abs() < 1e-6);
    }

    #[test]
    fn one_pole_svf_response() {
        let mut svf = OnePoleSVF::new(48000.0);
        // Use a high‑pass frequency well above the signal frequency and a low‑pass
        // frequency well below it. The combination should heavily attenuate a DC
        // (or low‑frequency) input.
        let hp = 5000.0; // high‑pass
        let lp = 200.0;  // low‑pass
        // Feed a constant DC value.
        let mut out = 0.0;
        for _ in 0..200 {
            out = svf.process(1.0, hp, lp);
        }
        // The output should be close to zero because the high‑pass removes the DC
        // component and the low‑pass then smooths the remaining high‑frequency
        // content.
        assert!(out.abs() < 0.05, "high‑pass/low‑pass chain did not attenuate enough: {}", out);
    }
}

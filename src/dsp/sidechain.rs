//! Sidechain envelope detector and ducker.
//!
//! Detects transients in the sidechain signal and produces a gain reduction
//! envelope with attack/hold/release stages, used to duck the wet signal.

pub struct SidechainDetector {
    envelope: f32,
    hold_counter: f32,
    sr: f32,
    // Parameters
    threshold_lin: f32,
    attack_coeff: f32,
    release_coeff: f32,
    hold_samples: f32,
}

impl SidechainDetector {
    pub fn new(sr: f32) -> Self {
        let mut det = Self {
            envelope: 0.0,
            hold_counter: 0.0,
            sr,
            threshold_lin: 0.0,
            attack_coeff: 0.0,
            release_coeff: 0.0,
            hold_samples: 0.0,
        };
        det.set_params(-18.0, 2.5, 60.0, 280.0);
        det
    }

    pub fn set_sample_rate(&mut self, sr: f32) {
        self.sr = sr;
    }

    /// Set sidechain parameters.
    /// - `threshold_db`: Detection threshold in dB.
    /// - `attack_ms`: Attack time.
    /// - `hold_ms`: Hold time before release starts.
    /// - `release_ms`: Release time.
    pub fn set_params(&mut self, threshold_db: f32, attack_ms: f32, hold_ms: f32, release_ms: f32) {
        self.threshold_lin = db_to_linear(threshold_db);
        self.attack_coeff = time_to_coeff(attack_ms, self.sr);
        self.release_coeff = time_to_coeff(release_ms, self.sr);
        self.hold_samples = hold_ms / 1000.0 * self.sr;
    }

    /// Process one sample of the sidechain signal.
    /// Returns the duck depth (0.0 = no ducking, 1.0 = fully ducked).
    pub fn process(&mut self, sc_sample: f32) -> f32 {
        let level = sc_sample.abs();

        if level > self.threshold_lin {
            // Attack: envelope rises
            self.envelope += self.attack_coeff * (1.0 - self.envelope);
            self.hold_counter = self.hold_samples;
        } else if self.hold_counter > 0.0 {
            // Hold: envelope stays
            self.hold_counter -= 1.0;
        } else {
            // Release: envelope falls
            self.envelope += self.release_coeff * (0.0 - self.envelope);
        }

        self.envelope = self.envelope.clamp(0.0, 1.0);
        self.envelope
    }

    /// Get current duck depth without advancing state.
    pub fn current_depth(&self) -> f32 {
        self.envelope
    }

    pub fn reset(&mut self) {
        self.envelope = 0.0;
        self.hold_counter = 0.0;
    }
}

#[inline(always)]
fn db_to_linear(db: f32) -> f32 {
    10.0_f32.powf(db / 20.0)
}

#[inline(always)]
fn time_to_coeff(time_ms: f32, sr: f32) -> f32 {
    if time_ms <= 0.0 {
        return 1.0;
    }
    let samples = time_ms / 1000.0 * sr;
    1.0 - (-1.0 / samples).exp()
}

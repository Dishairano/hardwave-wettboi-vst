//! ADSR envelope applied as an amplitude shaper to the wet signal.
//!
//! This is a simple follower-style ADSR: it tracks the signal level and
//! applies attack/decay/sustain/release smoothing to create a dynamic
//! amplitude envelope. When signal exceeds a gate threshold, the envelope
//! enters attack → decay → sustain. When signal drops below threshold,
//! release begins.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Idle,
    Attack,
    Decay,
    Sustain,
    Release,
}

#[derive(Debug, Clone)]
pub struct AdsrEnvelope {
    sample_rate: f32,
    stage: Stage,
    level: f32,

    // Times in ms
    attack_ms: f32,
    decay_ms: f32,
    sustain_level: f32,
    release_ms: f32,

    // Per-sample coefficients (computed from ms + sample_rate)
    attack_coeff: f32,
    decay_coeff: f32,
    release_coeff: f32,

    // Gate threshold in linear amplitude
    gate_threshold: f32,
}

impl AdsrEnvelope {
    pub fn new(sample_rate: f32) -> Self {
        let mut env = Self {
            sample_rate,
            stage: Stage::Idle,
            level: 0.0,
            attack_ms: 10.0,
            decay_ms: 100.0,
            sustain_level: 0.7,
            release_ms: 200.0,
            attack_coeff: 0.0,
            decay_coeff: 0.0,
            release_coeff: 0.0,
            gate_threshold: 0.001, // ~ -60 dB
        };
        env.recompute();
        env
    }

    pub fn set_sample_rate(&mut self, sr: f32) {
        self.sample_rate = sr;
        self.recompute();
    }

    pub fn set_attack(&mut self, ms: f32) {
        self.attack_ms = ms.max(0.1);
        self.attack_coeff = Self::time_constant(self.attack_ms, self.sample_rate);
    }

    pub fn set_decay(&mut self, ms: f32) {
        self.decay_ms = ms.max(0.1);
        self.decay_coeff = Self::time_constant(self.decay_ms, self.sample_rate);
    }

    pub fn set_sustain(&mut self, level: f32) {
        self.sustain_level = level.clamp(0.0, 1.0);
    }

    pub fn set_release(&mut self, ms: f32) {
        self.release_ms = ms.max(0.1);
        self.release_coeff = Self::time_constant(self.release_ms, self.sample_rate);
    }

    pub fn reset(&mut self) {
        self.stage = Stage::Idle;
        self.level = 0.0;
    }

    /// Process one sample. Returns the envelope value (0..1) to multiply with the signal.
    #[inline]
    pub fn process(&mut self, input_abs: f32) -> f32 {
        let gate_on = input_abs > self.gate_threshold;

        match self.stage {
            Stage::Idle => {
                if gate_on {
                    self.stage = Stage::Attack;
                }
            }
            Stage::Attack => {
                if !gate_on {
                    self.stage = Stage::Release;
                } else {
                    self.level += self.attack_coeff * (1.0 - self.level);
                    if self.level >= 0.999 {
                        self.level = 1.0;
                        self.stage = Stage::Decay;
                    }
                }
            }
            Stage::Decay => {
                if !gate_on {
                    self.stage = Stage::Release;
                } else {
                    self.level += self.decay_coeff * (self.sustain_level - self.level);
                    if (self.level - self.sustain_level).abs() < 0.001 {
                        self.level = self.sustain_level;
                        self.stage = Stage::Sustain;
                    }
                }
            }
            Stage::Sustain => {
                if !gate_on {
                    self.stage = Stage::Release;
                }
                self.level = self.sustain_level;
            }
            Stage::Release => {
                if gate_on {
                    self.stage = Stage::Attack;
                } else {
                    self.level += self.release_coeff * (0.0 - self.level);
                    if self.level < 0.001 {
                        self.level = 0.0;
                        self.stage = Stage::Idle;
                    }
                }
            }
        }

        self.level
    }

    fn recompute(&mut self) {
        self.attack_coeff = Self::time_constant(self.attack_ms, self.sample_rate);
        self.decay_coeff = Self::time_constant(self.decay_ms, self.sample_rate);
        self.release_coeff = Self::time_constant(self.release_ms, self.sample_rate);
    }

    /// Convert a time in ms to a one-pole smoothing coefficient.
    /// `coeff = 1 - exp(-1 / (time_seconds * sample_rate))`
    fn time_constant(ms: f32, sample_rate: f32) -> f32 {
        let samples = (ms * 0.001 * sample_rate).max(1.0);
        1.0 - (-1.0 / samples).exp()
    }
}

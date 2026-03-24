//! LFO (Low Frequency Oscillator) with multiple waveform shapes.

use std::f32::consts::PI;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Shape {
    Sine,
    Tri,
    Saw,
    Square,
    SampleAndHold,
}

pub struct Lfo {
    phase: f64,
    sr: f32,
    rate: f32,
    phase_offset: f32,
    shape: Shape,
    sh_value: f32,
    sh_last_phase: f64,
}

impl Lfo {
    pub fn new(sr: f32) -> Self {
        Self {
            phase: 0.0,
            sr,
            rate: 2.0,
            phase_offset: 0.0,
            shape: Shape::Sine,
            sh_value: 0.0,
            sh_last_phase: 0.0,
        }
    }

    pub fn set_sample_rate(&mut self, sr: f32) {
        self.sr = sr;
    }

    pub fn set_rate(&mut self, hz: f32) {
        self.rate = hz.max(0.001);
    }

    pub fn set_phase_offset(&mut self, degrees: f32) {
        self.phase_offset = degrees / 360.0;
    }

    pub fn set_shape(&mut self, shape: Shape) {
        self.shape = shape;
    }

    /// Advance the LFO by one sample and return value in range [-1.0, 1.0].
    pub fn process(&mut self) -> f32 {
        let inc = self.rate as f64 / self.sr as f64;
        self.phase += inc;
        if self.phase >= 1.0 {
            self.phase -= 1.0;
        }

        let p = ((self.phase + self.phase_offset as f64) % 1.0) as f32;

        match self.shape {
            Shape::Sine => (p * 2.0 * PI).sin(),
            Shape::Tri => {
                if p < 0.25 {
                    p * 4.0
                } else if p < 0.75 {
                    2.0 - p * 4.0
                } else {
                    p * 4.0 - 4.0
                }
            }
            Shape::Saw => 2.0 * p - 1.0,
            Shape::Square => {
                if p < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
            Shape::SampleAndHold => {
                // New random value each cycle
                if self.phase < self.sh_last_phase {
                    self.sh_value = simple_hash(self.phase) * 2.0 - 1.0;
                }
                self.sh_last_phase = self.phase;
                self.sh_value
            }
        }
    }

    pub fn reset(&mut self) {
        self.phase = 0.0;
        self.sh_value = 0.0;
        self.sh_last_phase = 0.0;
    }
}

/// Simple deterministic pseudo-random from phase (no std rand needed).
fn simple_hash(phase: f64) -> f32 {
    let x = (phase * 12345.6789).sin() * 43758.5453;
    (x - x.floor()) as f32
}

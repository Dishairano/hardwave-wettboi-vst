//! Second-order IIR (biquad) filter for hi-pass and lo-pass.
//!
//! Standard Direct Form II transposed implementation.
//! Coefficients from Audio EQ Cookbook (Robert Bristow-Johnson).

use std::f32::consts::PI;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterType {
    HighPass,
    LowPass,
}

#[derive(Debug, Clone)]
pub struct BiquadFilter {
    filter_type: FilterType,
    sample_rate: f32,
    freq: f32,
    q: f32,

    // Coefficients
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,

    // State (Direct Form II transposed)
    z1: f32,
    z2: f32,
}

impl BiquadFilter {
    pub fn new(filter_type: FilterType, sample_rate: f32) -> Self {
        let mut f = Self {
            filter_type,
            sample_rate,
            freq: if filter_type == FilterType::HighPass { 80.0 } else { 18000.0 },
            q: 0.707, // Butterworth
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
            z1: 0.0,
            z2: 0.0,
        };
        f.compute_coefficients();
        f
    }

    pub fn set_sample_rate(&mut self, sr: f32) {
        self.sample_rate = sr;
        self.compute_coefficients();
    }

    pub fn set_freq(&mut self, freq: f32) {
        let clamped = freq.clamp(20.0, self.sample_rate * 0.49);
        if (clamped - self.freq).abs() > 0.01 {
            self.freq = clamped;
            self.compute_coefficients();
        }
    }

    pub fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }

    #[inline]
    pub fn process(&mut self, input: f32) -> f32 {
        let out = self.b0 * input + self.z1;
        self.z1 = self.b1 * input - self.a1 * out + self.z2;
        self.z2 = self.b2 * input - self.a2 * out;
        out
    }

    fn compute_coefficients(&mut self) {
        let w0 = 2.0 * PI * self.freq / self.sample_rate;
        let cos_w0 = w0.cos();
        let sin_w0 = w0.sin();
        let alpha = sin_w0 / (2.0 * self.q);

        let (b0, b1, b2, a0, a1, a2) = match self.filter_type {
            FilterType::HighPass => {
                let b0 = (1.0 + cos_w0) / 2.0;
                let b1 = -(1.0 + cos_w0);
                let b2 = (1.0 + cos_w0) / 2.0;
                let a0 = 1.0 + alpha;
                let a1 = -2.0 * cos_w0;
                let a2 = 1.0 - alpha;
                (b0, b1, b2, a0, a1, a2)
            }
            FilterType::LowPass => {
                let b0 = (1.0 - cos_w0) / 2.0;
                let b1 = 1.0 - cos_w0;
                let b2 = (1.0 - cos_w0) / 2.0;
                let a0 = 1.0 + alpha;
                let a1 = -2.0 * cos_w0;
                let a2 = 1.0 - alpha;
                (b0, b1, b2, a0, a1, a2)
            }
        };

        // Normalise by a0
        let inv_a0 = 1.0 / a0;
        self.b0 = b0 * inv_a0;
        self.b1 = b1 * inv_a0;
        self.b2 = b2 * inv_a0;
        self.a1 = a1 * inv_a0;
        self.a2 = a2 * inv_a0;
    }
}

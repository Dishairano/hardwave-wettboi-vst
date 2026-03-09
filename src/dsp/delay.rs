//! Simple stereo delay line with feedback.
//!
//! Uses a circular buffer. Max delay is 2 seconds at any sample rate up to 192 kHz.

const MAX_DELAY_SECONDS: f32 = 2.5;

#[derive(Debug, Clone)]
pub struct DelayLine {
    buffer: Vec<f32>,
    write_pos: usize,
    sample_rate: f32,
    delay_samples: f32,
    feedback: f32,
    mix: f32,
}

impl DelayLine {
    pub fn new(sample_rate: f32) -> Self {
        let buf_size = (sample_rate * MAX_DELAY_SECONDS) as usize + 1;
        Self {
            buffer: vec![0.0; buf_size],
            write_pos: 0,
            sample_rate,
            delay_samples: 250.0 * 0.001 * sample_rate,
            feedback: 0.3,
            mix: 0.3,
        }
    }

    pub fn set_sample_rate(&mut self, sr: f32) {
        self.sample_rate = sr;
        let buf_size = (sr * MAX_DELAY_SECONDS) as usize + 1;
        self.buffer = vec![0.0; buf_size];
        self.write_pos = 0;
    }

    pub fn set_delay_ms(&mut self, ms: f32) {
        self.delay_samples = (ms * 0.001 * self.sample_rate).clamp(1.0, self.buffer.len() as f32 - 1.0);
    }

    pub fn set_feedback(&mut self, fb: f32) {
        self.feedback = fb.clamp(0.0, 0.98);
    }

    pub fn set_mix(&mut self, mix: f32) {
        self.mix = mix.clamp(0.0, 1.0);
    }

    pub fn reset(&mut self) {
        self.buffer.fill(0.0);
        self.write_pos = 0;
    }

    /// Process one sample. Returns dry/wet mixed output.
    #[inline]
    pub fn process(&mut self, input: f32) -> f32 {
        let buf_len = self.buffer.len();

        // Linear interpolation for fractional delay
        let delay_int = self.delay_samples as usize;
        let frac = self.delay_samples - delay_int as f32;

        let read_pos_a = (self.write_pos + buf_len - delay_int) % buf_len;
        let read_pos_b = (self.write_pos + buf_len - delay_int - 1) % buf_len;

        let delayed = self.buffer[read_pos_a] * (1.0 - frac) + self.buffer[read_pos_b] * frac;

        // Write input + feedback into buffer
        self.buffer[self.write_pos] = input + delayed * self.feedback;
        self.write_pos = (self.write_pos + 1) % buf_len;

        // Mix dry/wet
        input * (1.0 - self.mix) + delayed * self.mix
    }
}

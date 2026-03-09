//! Simple Schroeder-style reverb (4 comb filters + 2 allpass filters).
//!
//! Modelled after Freeverb with simplified tuning. Good enough for a
//! "wet signal" character effect — not trying to be a convolution reverb.

const NUM_COMBS: usize = 4;
const NUM_ALLPASSES: usize = 2;

// Comb filter delay lengths in samples at 44100 Hz (classic Freeverb values, simplified)
const COMB_LENGTHS: [usize; NUM_COMBS] = [1116, 1188, 1277, 1356];
const ALLPASS_LENGTHS: [usize; NUM_ALLPASSES] = [556, 441];

// Stereo spread (offset for right channel)
const STEREO_SPREAD: usize = 23;

#[derive(Debug, Clone)]
struct CombFilter {
    buffer: Vec<f32>,
    pos: usize,
    filterstore: f32,
}

impl CombFilter {
    fn new(size: usize) -> Self {
        Self {
            buffer: vec![0.0; size],
            pos: 0,
            filterstore: 0.0,
        }
    }

    #[inline]
    fn process(&mut self, input: f32, feedback: f32, damping: f32) -> f32 {
        let output = self.buffer[self.pos];

        // One-pole lowpass for damping
        self.filterstore = output * (1.0 - damping) + self.filterstore * damping;

        self.buffer[self.pos] = input + self.filterstore * feedback;
        self.pos = (self.pos + 1) % self.buffer.len();

        output
    }

    fn reset(&mut self) {
        self.buffer.fill(0.0);
        self.filterstore = 0.0;
        self.pos = 0;
    }
}

#[derive(Debug, Clone)]
struct AllpassFilter {
    buffer: Vec<f32>,
    pos: usize,
}

impl AllpassFilter {
    fn new(size: usize) -> Self {
        Self {
            buffer: vec![0.0; size],
            pos: 0,
        }
    }

    #[inline]
    fn process(&mut self, input: f32) -> f32 {
        let buffered = self.buffer[self.pos];
        let output = -input + buffered;
        self.buffer[self.pos] = input + buffered * 0.5;
        self.pos = (self.pos + 1) % self.buffer.len();
        output
    }

    fn reset(&mut self) {
        self.buffer.fill(0.0);
        self.pos = 0;
    }
}

#[derive(Debug, Clone)]
pub struct Reverb {
    combs_l: Vec<CombFilter>,
    combs_r: Vec<CombFilter>,
    allpasses_l: Vec<AllpassFilter>,
    allpasses_r: Vec<AllpassFilter>,
    feedback: f32,
    damping: f32,
    mix: f32,
    sample_rate: f32,
}

impl Reverb {
    pub fn new(sample_rate: f32) -> Self {
        let scale = (sample_rate / 44100.0).max(1.0) as usize;

        let combs_l: Vec<_> = COMB_LENGTHS.iter().map(|&l| CombFilter::new(l * scale)).collect();
        let combs_r: Vec<_> = COMB_LENGTHS.iter().map(|&l| CombFilter::new((l + STEREO_SPREAD) * scale)).collect();
        let allpasses_l: Vec<_> = ALLPASS_LENGTHS.iter().map(|&l| AllpassFilter::new(l * scale)).collect();
        let allpasses_r: Vec<_> = ALLPASS_LENGTHS.iter().map(|&l| AllpassFilter::new((l + STEREO_SPREAD) * scale)).collect();

        Self {
            combs_l,
            combs_r,
            allpasses_l,
            allpasses_r,
            feedback: 0.7,
            damping: 0.5,
            mix: 0.3,
            sample_rate,
        }
    }

    pub fn set_sample_rate(&mut self, sr: f32) {
        *self = Self::new(sr);
    }

    pub fn set_size(&mut self, size: f32) {
        // Map 0..1 to feedback 0.5..0.98
        self.feedback = 0.5 + size.clamp(0.0, 1.0) * 0.48;
    }

    pub fn set_damping(&mut self, damping: f32) {
        self.damping = damping.clamp(0.0, 1.0);
    }

    pub fn set_mix(&mut self, mix: f32) {
        self.mix = mix.clamp(0.0, 1.0);
    }

    pub fn reset(&mut self) {
        for c in &mut self.combs_l { c.reset(); }
        for c in &mut self.combs_r { c.reset(); }
        for a in &mut self.allpasses_l { a.reset(); }
        for a in &mut self.allpasses_r { a.reset(); }
    }

    /// Process a stereo pair. Returns (left, right).
    #[inline]
    pub fn process(&mut self, input_l: f32, input_r: f32) -> (f32, f32) {
        let input_mono = (input_l + input_r) * 0.5;

        // Sum comb filter outputs
        let mut out_l = 0.0_f32;
        let mut out_r = 0.0_f32;

        for comb in &mut self.combs_l {
            out_l += comb.process(input_mono, self.feedback, self.damping);
        }
        for comb in &mut self.combs_r {
            out_r += comb.process(input_mono, self.feedback, self.damping);
        }

        // Pass through allpass filters
        for ap in &mut self.allpasses_l {
            out_l = ap.process(out_l);
        }
        for ap in &mut self.allpasses_r {
            out_r = ap.process(out_r);
        }

        // Scale down comb outputs
        out_l *= 0.25;
        out_r *= 0.25;

        // Mix dry/wet
        (
            input_l * (1.0 - self.mix) + out_l * self.mix,
            input_r * (1.0 - self.mix) + out_r * self.mix,
        )
    }
}

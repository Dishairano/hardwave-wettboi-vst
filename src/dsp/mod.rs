//! DSP processing modules for WettBoi.

pub mod delay;
pub mod filters;
pub mod lfo;
pub mod mix;
pub mod reverb;
pub mod sidechain;

pub use delay::StereoDelay;
pub use lfo::Lfo;
pub use mix::{mix_dry_wet, parallel_gain};
pub use reverb::Reverb;
pub use sidechain::SidechainDetector;

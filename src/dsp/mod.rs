//! DSP processing modules for WettBoi.

pub mod delay;
pub mod filters;
pub mod lfo;
pub mod reverb;
pub mod sidechain;

pub use delay::StereoDelay;
pub use lfo::Lfo;
pub use reverb::Reverb;
pub use sidechain::SidechainDetector;

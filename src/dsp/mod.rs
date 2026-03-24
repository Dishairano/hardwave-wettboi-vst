//! DSP processing modules for WettBoi.

pub mod reverb;
pub mod delay;
pub mod sidechain;
pub mod lfo;
pub mod filters;

pub use reverb::Reverb;
pub use delay::StereoDelay;
pub use sidechain::SidechainDetector;
pub use lfo::Lfo;

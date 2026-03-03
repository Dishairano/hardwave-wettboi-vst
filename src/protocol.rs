//! Parameter state packet sent from the WettBoi VST plugin to the WebView UI.
//!
//! This mirrors the TypeScript `WettBoiState` interface exactly so the JSON
//! can be consumed directly by `window.__onWettBoiPacket(data)`.

use serde::{Deserialize, Serialize};

/// Full parameter state, serialised as JSON and pushed to the WebView.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WettBoiPacket {
    pub enabled: bool,
    pub wet: f32,

    pub hi_pass_enabled: bool,
    pub hi_pass_freq: f32,

    pub lo_pass_enabled: bool,
    pub lo_pass_freq: f32,

    pub attack: f32,
    pub decay: f32,
    pub sustain: f32,
    pub release: f32,

    pub delay_enabled: bool,
    pub delay_time: f32,
    pub delay_feedback: f32,
    pub delay_mix: f32,

    pub reverb_enabled: bool,
    pub reverb_size: f32,
    pub reverb_damping: f32,
    pub reverb_mix: f32,

    pub fx_order: &'static str,
}


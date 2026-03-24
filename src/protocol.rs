//! Rust → JS packet for the WettBoi webview UI.

use serde::{Deserialize, Serialize};

/// Full state packet pushed to the webview at ~60 fps.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WbPacket {
    pub bpm: f32,

    // ── Reverb ──────────────────────────────────────────────────────────────
    pub rev_enabled: bool,
    pub rev_predelay: f32,
    pub rev_size: f32,
    pub rev_decay: f32,
    pub rev_damp: f32,
    pub rev_width: f32,
    pub rev_wet: f32,
    pub rev_type: String,

    // ── Sidechain ───────────────────────────────────────────────────────────
    pub sc_threshold: f32,
    pub sc_attack: f32,
    pub sc_hold: f32,
    pub sc_release: f32,
    pub sc_source: String,
    /// Current duck depth from DSP (0.0 = no duck, 1.0 = fully ducked).
    pub sc_duck_depth: f32,

    // ── LFO ─────────────────────────────────────────────────────────────────
    pub lfo_enabled: bool,
    pub lfo_rate: f32,
    pub lfo_depth: f32,
    pub lfo_phase: f32,
    pub lfo_shape: String,
    pub lfo_target: String,

    // ── Delay ───────────────────────────────────────────────────────────────
    pub dly_enabled: bool,
    pub dly_sync: bool,
    pub dly_time_l: f32,
    pub dly_time_r: f32,
    pub dly_note_l: String,
    pub dly_note_r: String,
    pub dly_feedback: f32,
    pub dly_hp: f32,
    pub dly_lp: f32,
    pub dly_ping_pong: bool,
    pub dly_wet: f32,

    // ── Global ──────────────────────────────────────────────────────────────
    pub mix: f32,
    pub bypass: bool,
    pub preset: String,
}

/// JS → Rust messages from the webview.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum UiMessage {
    #[serde(rename = "set_param")]
    SetParam { id: String, value: serde_json::Value },
    #[serde(rename = "save_token")]
    SaveToken { token: String },
    #[serde(rename = "clear_token")]
    ClearToken,
}

//! DAW-exposed parameters for the WettBoi wet signal processor.
//!
//! Every field in the TypeScript `WettBoiState` interface has a corresponding
//! nih-plug parameter here so the DAW can automate / save / recall them.

use nih_plug::prelude::*;

/// FX chain order (delay → reverb vs reverb → delay).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum FxOrder {
    #[id = "delay-reverb"]
    #[name = "Delay → Reverb"]
    DelayReverb,
    #[id = "reverb-delay"]
    #[name = "Reverb → Delay"]
    ReverbDelay,
}

#[derive(Params)]
pub struct WettBoiParams {
    // ── Master ──────────────────────────────────────────────────────────────
    #[id = "enabled"]
    pub enabled: BoolParam,

    #[id = "wet"]
    pub wet: FloatParam,

    // ── Hi-pass filter ──────────────────────────────────────────────────────
    #[id = "hi_pass_enabled"]
    pub hi_pass_enabled: BoolParam,

    #[id = "hi_pass_freq"]
    pub hi_pass_freq: FloatParam,

    // ── Lo-pass filter ──────────────────────────────────────────────────────
    #[id = "lo_pass_enabled"]
    pub lo_pass_enabled: BoolParam,

    #[id = "lo_pass_freq"]
    pub lo_pass_freq: FloatParam,

    // ── ADSR envelope ───────────────────────────────────────────────────────
    #[id = "attack"]
    pub attack: FloatParam,

    #[id = "decay"]
    pub decay: FloatParam,

    #[id = "sustain"]
    pub sustain: FloatParam,

    #[id = "release"]
    pub release: FloatParam,

    // ── Delay ───────────────────────────────────────────────────────────────
    #[id = "delay_enabled"]
    pub delay_enabled: BoolParam,

    #[id = "delay_time"]
    pub delay_time: FloatParam,

    #[id = "delay_feedback"]
    pub delay_feedback: FloatParam,

    #[id = "delay_mix"]
    pub delay_mix: FloatParam,

    // ── Reverb ──────────────────────────────────────────────────────────────
    #[id = "reverb_enabled"]
    pub reverb_enabled: BoolParam,

    #[id = "reverb_size"]
    pub reverb_size: FloatParam,

    #[id = "reverb_damping"]
    pub reverb_damping: FloatParam,

    #[id = "reverb_mix"]
    pub reverb_mix: FloatParam,

    // ── FX chain ────────────────────────────────────────────────────────────
    #[id = "fx_order"]
    pub fx_order: EnumParam<FxOrder>,
}

impl Default for WettBoiParams {
    fn default() -> Self {
        Self {
            // Master
            enabled: BoolParam::new("Enabled", true),
            wet: FloatParam::new("Wet", 1.0, FloatRange::Linear { min: 0.0, max: 1.0 })
                .with_unit(" %")
                .with_value_to_string(formatters::v2s_f32_percentage(0))
                .with_string_to_value(formatters::s2v_f32_percentage()),

            // Hi-pass
            hi_pass_enabled: BoolParam::new("Hi-Pass On", false),
            hi_pass_freq: FloatParam::new(
                "Hi-Pass Freq",
                80.0,
                FloatRange::Skewed {
                    min: 20.0,
                    max: 800.0,
                    factor: FloatRange::skew_factor(-1.0),
                },
            )
            .with_unit(" Hz")
            .with_value_to_string(formatters::v2s_f32_hz_then_khz(0))
            .with_string_to_value(formatters::s2v_f32_hz_then_khz()),

            // Lo-pass
            lo_pass_enabled: BoolParam::new("Lo-Pass On", false),
            lo_pass_freq: FloatParam::new(
                "Lo-Pass Freq",
                18000.0,
                FloatRange::Skewed {
                    min: 2000.0,
                    max: 22000.0,
                    factor: FloatRange::skew_factor(-1.0),
                },
            )
            .with_unit(" Hz")
            .with_value_to_string(formatters::v2s_f32_hz_then_khz(0))
            .with_string_to_value(formatters::s2v_f32_hz_then_khz()),

            // ADSR
            attack: FloatParam::new(
                "Attack",
                10.0,
                FloatRange::Skewed {
                    min: 1.0,
                    max: 2000.0,
                    factor: FloatRange::skew_factor(-2.0),
                },
            )
            .with_unit(" ms"),

            decay: FloatParam::new(
                "Decay",
                100.0,
                FloatRange::Skewed {
                    min: 1.0,
                    max: 2000.0,
                    factor: FloatRange::skew_factor(-2.0),
                },
            )
            .with_unit(" ms"),

            sustain: FloatParam::new("Sustain", 0.7, FloatRange::Linear { min: 0.0, max: 1.0 })
                .with_unit(" %")
                .with_value_to_string(formatters::v2s_f32_percentage(0))
                .with_string_to_value(formatters::s2v_f32_percentage()),

            release: FloatParam::new(
                "Release",
                200.0,
                FloatRange::Skewed {
                    min: 10.0,
                    max: 5000.0,
                    factor: FloatRange::skew_factor(-2.0),
                },
            )
            .with_unit(" ms"),

            // Delay
            delay_enabled: BoolParam::new("Delay On", false),
            delay_time: FloatParam::new(
                "Delay Time",
                250.0,
                FloatRange::Skewed {
                    min: 1.0,
                    max: 2000.0,
                    factor: FloatRange::skew_factor(-1.5),
                },
            )
            .with_unit(" ms"),

            delay_feedback: FloatParam::new(
                "Delay Feedback",
                0.3,
                FloatRange::Linear { min: 0.0, max: 0.98 },
            )
            .with_unit(" %")
            .with_value_to_string(formatters::v2s_f32_percentage(0))
            .with_string_to_value(formatters::s2v_f32_percentage()),

            delay_mix: FloatParam::new(
                "Delay Mix",
                0.3,
                FloatRange::Linear { min: 0.0, max: 1.0 },
            )
            .with_unit(" %")
            .with_value_to_string(formatters::v2s_f32_percentage(0))
            .with_string_to_value(formatters::s2v_f32_percentage()),

            // Reverb
            reverb_enabled: BoolParam::new("Reverb On", false),
            reverb_size: FloatParam::new(
                "Reverb Size",
                0.5,
                FloatRange::Linear { min: 0.0, max: 1.0 },
            )
            .with_unit(" %")
            .with_value_to_string(formatters::v2s_f32_percentage(0))
            .with_string_to_value(formatters::s2v_f32_percentage()),

            reverb_damping: FloatParam::new(
                "Reverb Damping",
                0.5,
                FloatRange::Linear { min: 0.0, max: 1.0 },
            )
            .with_unit(" %")
            .with_value_to_string(formatters::v2s_f32_percentage(0))
            .with_string_to_value(formatters::s2v_f32_percentage()),

            reverb_mix: FloatParam::new(
                "Reverb Mix",
                0.3,
                FloatRange::Linear { min: 0.0, max: 1.0 },
            )
            .with_unit(" %")
            .with_value_to_string(formatters::v2s_f32_percentage(0))
            .with_string_to_value(formatters::s2v_f32_percentage()),

            // FX chain
            fx_order: EnumParam::new("FX Order", FxOrder::DelayReverb),
        }
    }
}

//! DAW-exposed parameters for Hardwave WettBoi.

use nih_plug::prelude::*;

/// Reverb algorithm type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum ReverbType {
    #[name = "Room"]
    Room,
    #[name = "Hall"]
    Hall,
    #[name = "Plate"]
    Plate,
    #[name = "Spring"]
    Spring,
}

/// Sidechain source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum ScSource {
    #[name = "Internal"]
    Internal,
    #[name = "Sidechain"]
    Sidechain,
}

/// LFO waveform shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum LfoShape {
    #[name = "Sine"]
    Sine,
    #[name = "Tri"]
    Tri,
    #[name = "Saw"]
    Saw,
    #[name = "Square"]
    Square,
    #[name = "S&H"]
    SampleAndHold,
}

/// LFO modulation target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum LfoTarget {
    #[name = "Rev Wet"]
    RevWet,
    #[name = "Dly Wet"]
    DlyWet,
    #[name = "Dly FB"]
    DlyFeedback,
    #[name = "Filter"]
    Filter,
}

/// Signal routing mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum RoutingMode {
    #[name = "Parallel"]
    Parallel,
    #[name = "Rev→Dly"]
    ReverbToDelay,
    #[name = "Dly→Rev"]
    DelayToReverb,
}

/// Delay note division for tempo sync.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Enum)]
pub enum NoteDiv {
    #[name = "1/16"]
    Sixteenth,
    #[name = "1/8"]
    Eighth,
    #[name = "d1/8"]
    DottedEighth,
    #[name = "1/4"]
    Quarter,
    #[name = "d1/4"]
    DottedQuarter,
    #[name = "1/2"]
    Half,
    #[name = "d1/2"]
    DottedHalf,
    #[name = "1/1"]
    Whole,
}

impl NoteDiv {
    /// Returns the note length as a fraction of a beat (quarter note = 1.0).
    pub fn beats(&self) -> f32 {
        match self {
            NoteDiv::Sixteenth => 0.25,
            NoteDiv::Eighth => 0.5,
            NoteDiv::DottedEighth => 0.75,
            NoteDiv::Quarter => 1.0,
            NoteDiv::DottedQuarter => 1.5,
            NoteDiv::Half => 2.0,
            NoteDiv::DottedHalf => 3.0,
            NoteDiv::Whole => 4.0,
        }
    }
}

#[derive(Params)]
pub struct WettBoiParams {
    // ── Reverb ──────────────────────────────────────────────────────────────
    #[id = "rev_enabled"]
    pub rev_enabled: BoolParam,
    #[id = "rev_type"]
    pub rev_type: EnumParam<ReverbType>,
    #[id = "rev_predelay"]
    pub rev_predelay: FloatParam,
    #[id = "rev_size"]
    pub rev_size: FloatParam,
    #[id = "rev_decay"]
    pub rev_decay: FloatParam,
    #[id = "rev_damp"]
    pub rev_damp: FloatParam,
    #[id = "rev_width"]
    pub rev_width: FloatParam,
    #[id = "rev_wet"]
    pub rev_wet: FloatParam,
    #[id = "rev_freeze"]
    pub rev_freeze: BoolParam,
    #[id = "rev_eq_hp"]
    pub rev_eq_hp: FloatParam,
    #[id = "rev_eq_lp"]
    pub rev_eq_lp: FloatParam,

    // ── Sidechain ───────────────────────────────────────────────────────────
    #[id = "sc_threshold"]
    pub sc_threshold: FloatParam,
    #[id = "sc_attack"]
    pub sc_attack: FloatParam,
    #[id = "sc_hold"]
    pub sc_hold: FloatParam,
    #[id = "sc_release"]
    pub sc_release: FloatParam,
    #[id = "sc_source"]
    pub sc_source: EnumParam<ScSource>,

    // ── LFO ─────────────────────────────────────────────────────────────────
    #[id = "lfo_enabled"]
    pub lfo_enabled: BoolParam,
    #[id = "lfo_rate"]
    pub lfo_rate: FloatParam,
    #[id = "lfo_depth"]
    pub lfo_depth: FloatParam,
    #[id = "lfo_phase"]
    pub lfo_phase: FloatParam,
    #[id = "lfo_shape"]
    pub lfo_shape: EnumParam<LfoShape>,
    #[id = "lfo_target"]
    pub lfo_target: EnumParam<LfoTarget>,

    // ── Delay ───────────────────────────────────────────────────────────────
    #[id = "dly_enabled"]
    pub dly_enabled: BoolParam,
    #[id = "dly_sync"]
    pub dly_sync: BoolParam,
    #[id = "dly_time_l"]
    pub dly_time_l: FloatParam,
    #[id = "dly_time_r"]
    pub dly_time_r: FloatParam,
    #[id = "dly_note_l"]
    pub dly_note_l: EnumParam<NoteDiv>,
    #[id = "dly_note_r"]
    pub dly_note_r: EnumParam<NoteDiv>,
    #[id = "dly_feedback"]
    pub dly_feedback: FloatParam,
    #[id = "dly_hp"]
    pub dly_hp: FloatParam,
    #[id = "dly_lp"]
    pub dly_lp: FloatParam,
    #[id = "dly_ping_pong"]
    pub dly_ping_pong: BoolParam,
    #[id = "dly_wet"]
    pub dly_wet: FloatParam,
    #[id = "dly_mod_rate"]
    pub dly_mod_rate: FloatParam,
    #[id = "dly_mod_depth"]
    pub dly_mod_depth: FloatParam,
    #[id = "dly_saturation"]
    pub dly_saturation: FloatParam,

    // ── Global ──────────────────────────────────────────────────────────────
    #[id = "mix"]
    pub mix: FloatParam,
    #[id = "bypass"]
    pub bypass: BoolParam,
    #[id = "routing"]
    pub routing: EnumParam<RoutingMode>,
}

impl Default for WettBoiParams {
    fn default() -> Self {
        Self {
            // Reverb
            rev_enabled: BoolParam::new("Reverb On", true),
            rev_type: EnumParam::new("Reverb Type", ReverbType::Room),
            rev_predelay: FloatParam::new(
                "Pre-Delay",
                18.0,
                FloatRange::Linear { min: 0.0, max: 200.0 },
            )
            .with_unit(" ms"),
            rev_size: FloatParam::new(
                "Size",
                65.0,
                FloatRange::Linear { min: 0.0, max: 100.0 },
            )
            .with_unit(" %")
            .with_value_to_string(formatters::v2s_f32_rounded(0)),
            rev_decay: FloatParam::new(
                "Decay",
                2.4,
                FloatRange::Skewed {
                    min: 0.1,
                    max: 20.0,
                    factor: FloatRange::skew_factor(-2.0),
                },
            )
            .with_unit(" s"),
            rev_damp: FloatParam::new(
                "Damp",
                40.0,
                FloatRange::Linear { min: 0.0, max: 100.0 },
            )
            .with_unit(" %")
            .with_value_to_string(formatters::v2s_f32_rounded(0)),
            rev_width: FloatParam::new(
                "Width",
                120.0,
                FloatRange::Linear { min: 0.0, max: 200.0 },
            )
            .with_unit(" %")
            .with_value_to_string(formatters::v2s_f32_rounded(0)),
            rev_wet: FloatParam::new(
                "Rev Wet",
                70.0,
                FloatRange::Linear { min: 0.0, max: 100.0 },
            )
            .with_unit(" %")
            .with_value_to_string(formatters::v2s_f32_rounded(0)),
            rev_freeze: BoolParam::new("Freeze", false),
            rev_eq_hp: FloatParam::new(
                "Rev EQ HP",
                20.0,
                FloatRange::Skewed {
                    min: 20.0,
                    max: 2000.0,
                    factor: FloatRange::skew_factor(-1.5),
                },
            )
            .with_unit(" Hz"),
            rev_eq_lp: FloatParam::new(
                "Rev EQ LP",
                18000.0,
                FloatRange::Skewed {
                    min: 1000.0,
                    max: 20000.0,
                    factor: FloatRange::skew_factor(-1.0),
                },
            )
            .with_unit(" Hz"),

            // Sidechain
            sc_threshold: FloatParam::new(
                "SC Threshold",
                -18.0,
                FloatRange::Linear { min: -60.0, max: 0.0 },
            )
            .with_unit(" dB"),
            sc_attack: FloatParam::new(
                "SC Attack",
                2.5,
                FloatRange::Skewed {
                    min: 0.1,
                    max: 50.0,
                    factor: FloatRange::skew_factor(-2.0),
                },
            )
            .with_unit(" ms"),
            sc_hold: FloatParam::new(
                "SC Hold",
                60.0,
                FloatRange::Linear { min: 0.0, max: 500.0 },
            )
            .with_unit(" ms"),
            sc_release: FloatParam::new(
                "SC Release",
                280.0,
                FloatRange::Skewed {
                    min: 10.0,
                    max: 2000.0,
                    factor: FloatRange::skew_factor(-1.5),
                },
            )
            .with_unit(" ms"),
            sc_source: EnumParam::new("SC Source", ScSource::Sidechain),

            // LFO
            lfo_enabled: BoolParam::new("LFO On", true),
            lfo_rate: FloatParam::new(
                "LFO Rate",
                2.0,
                FloatRange::Skewed {
                    min: 0.01,
                    max: 20.0,
                    factor: FloatRange::skew_factor(-2.0),
                },
            )
            .with_unit(" Hz"),
            lfo_depth: FloatParam::new(
                "LFO Depth",
                50.0,
                FloatRange::Linear { min: 0.0, max: 100.0 },
            )
            .with_unit(" %")
            .with_value_to_string(formatters::v2s_f32_rounded(0)),
            lfo_phase: FloatParam::new(
                "LFO Phase",
                0.0,
                FloatRange::Linear { min: 0.0, max: 360.0 },
            )
            .with_unit("°")
            .with_value_to_string(formatters::v2s_f32_rounded(0)),
            lfo_shape: EnumParam::new("LFO Shape", LfoShape::Sine),
            lfo_target: EnumParam::new("LFO Target", LfoTarget::RevWet),

            // Delay
            dly_enabled: BoolParam::new("Delay On", true),
            dly_sync: BoolParam::new("Delay Sync", true),
            dly_time_l: FloatParam::new(
                "Delay L",
                400.0,
                FloatRange::Skewed {
                    min: 1.0,
                    max: 2000.0,
                    factor: FloatRange::skew_factor(-1.5),
                },
            )
            .with_unit(" ms"),
            dly_time_r: FloatParam::new(
                "Delay R",
                600.0,
                FloatRange::Skewed {
                    min: 1.0,
                    max: 2000.0,
                    factor: FloatRange::skew_factor(-1.5),
                },
            )
            .with_unit(" ms"),
            dly_note_l: EnumParam::new("Note L", NoteDiv::Eighth),
            dly_note_r: EnumParam::new("Note R", NoteDiv::DottedEighth),
            dly_feedback: FloatParam::new(
                "Feedback",
                35.0,
                FloatRange::Linear { min: 0.0, max: 95.0 },
            )
            .with_unit(" %")
            .with_value_to_string(formatters::v2s_f32_rounded(0)),
            dly_hp: FloatParam::new(
                "Delay HP",
                120.0,
                FloatRange::Skewed {
                    min: 20.0,
                    max: 2000.0,
                    factor: FloatRange::skew_factor(-1.5),
                },
            )
            .with_unit(" Hz"),
            dly_lp: FloatParam::new(
                "Delay LP",
                8000.0,
                FloatRange::Skewed {
                    min: 1000.0,
                    max: 20000.0,
                    factor: FloatRange::skew_factor(-1.0),
                },
            )
            .with_unit(" Hz"),
            dly_ping_pong: BoolParam::new("Ping Pong", true),
            dly_wet: FloatParam::new(
                "Dly Wet",
                55.0,
                FloatRange::Linear { min: 0.0, max: 100.0 },
            )
            .with_unit(" %")
            .with_value_to_string(formatters::v2s_f32_rounded(0)),
            dly_mod_rate: FloatParam::new(
                "Mod Rate",
                0.5,
                FloatRange::Skewed {
                    min: 0.01,
                    max: 10.0,
                    factor: FloatRange::skew_factor(-2.0),
                },
            )
            .with_unit(" Hz"),
            dly_mod_depth: FloatParam::new(
                "Mod Depth",
                0.0,
                FloatRange::Linear { min: 0.0, max: 100.0 },
            )
            .with_unit(" %")
            .with_value_to_string(formatters::v2s_f32_rounded(0)),
            dly_saturation: FloatParam::new(
                "Saturation",
                0.0,
                FloatRange::Linear { min: 0.0, max: 100.0 },
            )
            .with_unit(" %")
            .with_value_to_string(formatters::v2s_f32_rounded(0)),

            // Global
            mix: FloatParam::new("Mix", 75.0, FloatRange::Linear { min: 0.0, max: 100.0 })
                .with_unit(" %")
                .with_value_to_string(formatters::v2s_f32_rounded(0)),
            bypass: BoolParam::new("Bypass", false),
            routing: EnumParam::new("Routing", RoutingMode::Parallel),
        }
    }
}

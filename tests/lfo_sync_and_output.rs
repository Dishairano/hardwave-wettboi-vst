//! LFO tempo sync and the output gain.
//!
//! A customer asked for the LFO rate in beats instead of Hz, and for an output
//! gain because the plug-in made his signal quieter. Both are new parameters,
//! and both have to leave every existing project and preset sounding exactly as
//! it did: sync off, and 0 dB of gain that changes no sample at all.

use hardwave_wettboi::editor::{lfo_note_index, string_to_param_value, LFO_NOTE_NAMES};
use hardwave_wettboi::params::{beats_per_bar, LfoNote, WettBoiParams};
use hardwave_wettboi::{output_gain, sanitize_state};
use nih_plug::prelude::{Enum, PluginState};
use nih_plug::wrapper::state::ParamValue;
use std::collections::BTreeMap;

const ALL_NOTES: [LfoNote; 8] = [
    LfoNote::Sixteenth,
    LfoNote::Eighth,
    LfoNote::Quarter,
    LfoNote::Half,
    LfoNote::Bar,
    LfoNote::TwoBars,
    LfoNote::FourBars,
    LfoNote::EightBars,
];

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() <= 1.0e-6 * b.abs().max(1.0)
}

/// `ParamValue` has no `PartialEq`, so compare by kind and value.
fn assert_value(state: &PluginState, id: &str, want: ParamValue) {
    let got = state.params.get(id);
    let same = match (got, &want) {
        (Some(ParamValue::F32(a)), ParamValue::F32(b)) => a == b,
        (Some(ParamValue::I32(a)), ParamValue::I32(b)) => a == b,
        (Some(ParamValue::Bool(a)), ParamValue::Bool(b)) => a == b,
        _ => false,
    };
    assert!(same, "{id} is {got:?}, expected {want:?}");
}

#[test]
fn each_note_lasts_the_beats_it_says_in_four_four() {
    let beats: Vec<f32> = ALL_NOTES.iter().map(|n| n.beats(4.0)).collect();
    // Written out rather than computed, so a wrong value or order fails here.
    assert_eq!(beats, vec![0.25, 0.5, 1.0, 2.0, 4.0, 8.0, 16.0, 32.0]);
}

#[test]
fn note_values_turn_into_the_right_rate() {
    // At 120 BPM a beat is half a second, so a quarter note cycles at 2 Hz.
    let expected_hz = [8.0, 4.0, 2.0, 1.0, 0.5, 0.25, 0.125, 0.0625];
    for (note, hz) in ALL_NOTES.iter().zip(expected_hz) {
        let got = note.rate_hz(120.0, 4.0);
        assert!(
            close(got, hz),
            "{note:?} at 120 BPM gave {got} Hz, expected {hz}"
        );
    }
    // Harder styles live around 150: a 1/4 is 2.5 Hz, one bar is 0.625 Hz.
    assert!(close(LfoNote::Quarter.rate_hz(150.0, 4.0), 2.5));
    assert!(close(LfoNote::Bar.rate_hz(150.0, 4.0), 0.625));
    // Doubling the tempo doubles the rate.
    for note in ALL_NOTES {
        assert!(close(
            note.rate_hz(300.0, 4.0),
            2.0 * note.rate_hz(150.0, 4.0)
        ));
    }
}

#[test]
fn bars_follow_the_time_signature() {
    assert_eq!(beats_per_bar(Some(4), Some(4)), Some(4.0));
    assert_eq!(beats_per_bar(Some(3), Some(4)), Some(3.0));
    // 6/8 is six eighth notes, which is three beats, not six.
    assert_eq!(beats_per_bar(Some(6), Some(8)), Some(3.0));
    assert_eq!(beats_per_bar(Some(7), Some(8)), Some(3.5));
    // A numerator alone is counted in beats.
    assert_eq!(beats_per_bar(Some(5), None), Some(5.0));
    // No time signature, or one no song has: keep the bar we had.
    assert_eq!(beats_per_bar(None, Some(4)), None);
    assert_eq!(beats_per_bar(None, None), None);
    assert_eq!(beats_per_bar(Some(0), Some(4)), None);
    assert_eq!(beats_per_bar(Some(-3), Some(4)), None);
    // A broken denominator falls back to quarter notes.
    assert_eq!(beats_per_bar(Some(3), Some(0)), Some(3.0));

    // One bar of 3/4 at 120 BPM is a second and a half.
    let bar = beats_per_bar(Some(3), Some(4)).unwrap();
    assert!(close(LfoNote::Bar.rate_hz(120.0, bar), 1.0 / 1.5));
    // The note values below a bar do not care about the time signature.
    assert_eq!(LfoNote::Quarter.beats(3.0), LfoNote::Quarter.beats(4.0));
}

#[test]
fn zero_db_is_unity() {
    assert_eq!(output_gain(0.0), 1.0);
    // Not close to: equal, bit for bit, so a project that never touches the
    // control renders the same samples it did before.
    for x in [0.0_f32, 1.0, -1.0, 0.123_456_79, -0.987_654_3, 1.0e-20, 7.5] {
        assert_eq!((x * output_gain(0.0)).to_bits(), x.to_bits(), "{x} changed");
    }
}

#[test]
fn the_gain_is_in_decibels() {
    let db = |g: f32| 20.0 * g.log10();
    assert!((db(output_gain(-6.0)) + 6.0).abs() < 1.0e-3);
    assert!((db(output_gain(12.0)) - 12.0).abs() < 1.0e-3);
    assert!((db(output_gain(-24.0)) + 24.0).abs() < 1.0e-3);
}

#[test]
fn the_defaults_change_nothing() {
    let p = WettBoiParams::default();
    assert!(!p.lfo_sync.value(), "tempo sync is on by default");
    assert_eq!(p.lfo_note.value(), LfoNote::Quarter);
    assert_eq!(p.out_gain.value(), 0.0);
    // The smoother at rest gives the value itself, so the gain is exactly 1.
    for _ in 0..64 {
        assert_eq!(output_gain(p.out_gain.smoothed.next()), 1.0);
    }
}

#[test]
fn the_ui_and_the_host_use_the_same_note_names() {
    // The UI sends these names, and the host shows the enum's own names. If the
    // two lists drift apart a click selects one note and the host shows another.
    assert_eq!(LfoNote::variants(), &LFO_NOTE_NAMES[..]);
    for (i, name) in LFO_NOTE_NAMES.iter().enumerate() {
        assert_eq!(lfo_note_index(name), Some(i), "{name} is not variant {i}");
        assert_eq!(LfoNote::from_index(i), ALL_NOTES[i]);
        assert_eq!(string_to_param_value("lfo_note", name), Some(i as f32));
    }
    assert_eq!(lfo_note_index("3 bars"), None);
    assert_eq!(lfo_note_index("d1/8"), None);
    assert_eq!(string_to_param_value("lfo_note", "1/1"), None);
}

/// A preset saved before these parameters existed does not mention them. It
/// has to load with them at their defaults, whatever they were set to before,
/// or its LFO would run at a note value instead of the rate it was saved with.
#[test]
fn an_old_state_gets_the_new_parameters_at_their_defaults() {
    let mut params = BTreeMap::new();
    params.insert("lfo_rate".to_string(), ParamValue::F32(0.5));
    params.insert("mix".to_string(), ParamValue::F32(60.0));
    let mut state = PluginState {
        version: "0.4.10-rc1".to_string(),
        params,
        fields: BTreeMap::new(),
    };

    sanitize_state(&mut state);

    assert_value(&state, "lfo_sync", ParamValue::Bool(false));
    // Quarter is the third variant.
    assert_value(&state, "lfo_note", ParamValue::I32(2));
    assert_value(&state, "out_gain", ParamValue::F32(0.0));
    // What the state did say is left as it was.
    assert_value(&state, "lfo_rate", ParamValue::F32(0.5));
    assert_value(&state, "mix", ParamValue::F32(60.0));
    assert_eq!(state.params.len(), 5);
}

#[test]
fn a_new_state_keeps_its_own_values() {
    let mut params = BTreeMap::new();
    params.insert("lfo_sync".to_string(), ParamValue::Bool(true));
    params.insert("lfo_note".to_string(), ParamValue::I32(5));
    params.insert("out_gain".to_string(), ParamValue::F32(4.5));
    let mut state = PluginState {
        version: "0.4.11".to_string(),
        params,
        fields: BTreeMap::new(),
    };

    sanitize_state(&mut state);

    assert_value(&state, "lfo_sync", ParamValue::Bool(true));
    assert_value(&state, "lfo_note", ParamValue::I32(5));
    assert_value(&state, "out_gain", ParamValue::F32(4.5));

    // And a gain outside the range is pulled into it, like every other value.
    let mut params = BTreeMap::new();
    params.insert("out_gain".to_string(), ParamValue::F32(80.0));
    let mut state = PluginState {
        version: "0.4.11".to_string(),
        params,
        fields: BTreeMap::new(),
    };
    sanitize_state(&mut state);
    assert_value(&state, "out_gain", ParamValue::F32(12.0));
}

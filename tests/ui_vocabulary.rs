//! Every word the UI can send must reach a parameter.
//!
//! The lists below are copied from the webview (`WettBoiUI.tsx`) on purpose: if
//! somebody adds an option there and not here, this test still passes, but if
//! somebody changes the plugin's vocabulary the test fails loudly. It exists
//! because the delay's note divisions were missing from the plugin's parser in
//! the shipped build, so the buttons and the presets that used them did nothing
//! and nothing anywhere reported a problem.

use hardwave_wettboi::editor::{note_div_index, string_to_param_value, NOTE_DIV_NAMES};

/// (param id, the values the UI sends for it)
const UI_VOCABULARY: &[(&str, &[&str])] = &[
    ("rev_type", &["room", "hall", "plate", "spring"]),
    ("sc_source", &["internal", "sidechain"]),
    ("lfo_shape", &["sine", "tri", "saw", "square", "s&h"]),
    ("lfo_target", &["rev_wet", "dly_wet", "dly_fb", "filter"]),
    ("routing", &["parallel", "rev_to_dly", "dly_to_rev"]),
    (
        "dly_note_l",
        &["1/16", "1/8", "d1/8", "1/4", "d1/4", "1/2", "d1/2", "1/1"],
    ),
    (
        "dly_note_r",
        &["1/16", "1/8", "d1/8", "1/4", "d1/4", "1/2", "d1/2", "1/1"],
    ),
];

#[test]
fn every_value_the_ui_can_send_resolves() {
    let mut dead = Vec::new();
    for (id, values) in UI_VOCABULARY {
        for v in *values {
            if string_to_param_value(id, v).is_none() {
                dead.push(format!("{id} = {v:?}"));
            }
        }
    }
    for (id, values) in UI_VOCABULARY {
        println!("  {id}: {} values", values.len());
    }
    assert!(
        dead.is_empty(),
        "these do nothing when the UI sends them: {dead:?}"
    );
}

#[test]
fn note_divisions_map_to_distinct_variants() {
    let mut seen = Vec::new();
    for (i, name) in NOTE_DIV_NAMES.iter().enumerate() {
        let idx = note_div_index(name).unwrap_or_else(|| panic!("{name} is not a note division"));
        assert_eq!(idx, i, "{name} resolved to variant {idx}, expected {i}");
        seen.push(idx);
    }
    seen.sort_unstable();
    seen.dedup();
    assert_eq!(
        seen.len(),
        NOTE_DIV_NAMES.len(),
        "two note names share a variant"
    );
    println!(
        "  all {} note divisions map to their own variant",
        NOTE_DIV_NAMES.len()
    );
}

#[test]
fn an_unknown_value_is_refused_rather_than_guessed() {
    assert!(string_to_param_value("rev_type", "cathedral").is_none());
    assert!(string_to_param_value("dly_note_l", "1/3").is_none());
    assert!(string_to_param_value("no_such_param", "anything").is_none());
    println!("  unknown values return None instead of silently landing on variant 0");
}

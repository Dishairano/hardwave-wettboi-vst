//! The controls a person clicks have to arrive at the DSP.
//!
//! Every delay note button was dead: the editor had no case for dly_note_l and
//! dly_note_r, so each click fell through to None and the parameter never moved.
//! Tempo sync is the default mode, which makes those buttons the only visible
//! time control, so the delay time could not be changed at all.

use hardwave_wettboi::editor::{note_div_index, NOTE_DIV_NAMES};
use hardwave_wettboi::mix_dry_wet;
use hardwave_wettboi::params::NoteDiv;

#[test]
fn every_note_the_ui_offers_maps_to_a_value() {
    for (i, name) in NOTE_DIV_NAMES.iter().enumerate() {
        let got = note_div_index(name);
        assert_eq!(got, Some(i), "note {name:?} did not map to index {i}");
    }
}

#[test]
fn an_unknown_note_is_refused_rather_than_guessed() {
    assert_eq!(note_div_index("1/3"), None);
    assert_eq!(note_div_index(""), None);
    assert_eq!(note_div_index("d1/16"), None);
}

/// The names have to line up with the enum, or a click selects the wrong length.
#[test]
fn the_names_are_in_enum_order() {
    let beats: Vec<f32> = NOTE_DIV_NAMES
        .iter()
        .map(|n| {
            let i = note_div_index(n).unwrap();
            let div = match i {
                0 => NoteDiv::Sixteenth,
                1 => NoteDiv::Eighth,
                2 => NoteDiv::DottedEighth,
                3 => NoteDiv::Quarter,
                4 => NoteDiv::DottedQuarter,
                5 => NoteDiv::Half,
                6 => NoteDiv::DottedHalf,
                7 => NoteDiv::Whole,
                _ => unreachable!(),
            };
            div.beats()
        })
        .collect();
    // Written out rather than computed, so a wrong reordering fails here.
    assert_eq!(beats, vec![0.25, 0.5, 0.75, 1.0, 1.5, 2.0, 3.0, 4.0]);
    for pair in beats.windows(2) {
        assert!(pair[1] > pair[0], "the list is not ascending: {pair:?}");
    }
}

#[test]
fn mix_is_a_crossfade_not_a_sum() {
    // Fully dry and fully wet are exactly that.
    assert_eq!(mix_dry_wet(1.0, 0.25, 0.0), 1.0);
    assert_eq!(mix_dry_wet(1.0, 0.25, 1.0), 0.25);
    // Halfway is halfway.
    assert!((mix_dry_wet(1.0, 0.0, 0.5) - 0.5).abs() < 1e-6);
}

#[test]
fn turning_mix_up_cannot_make_it_louder_than_the_loudest_side() {
    // The old formula was dry + wet * mix, so this returned 1.75 at mix 0.75
    // with a wet of 1.0: louder than either input, and louder still in Parallel
    // routing where reverb and delay are summed into the wet.
    for m in [0.0, 0.25, 0.5, 0.75, 1.0] {
        let out = mix_dry_wet(1.0, 1.0, m);
        assert!(out <= 1.0 + 1e-6, "mix {m} gave {out}, above both inputs");
    }
    let wet_heavy = mix_dry_wet(1.0, 2.0, 0.75);
    assert!(
        wet_heavy <= 2.0,
        "output {wet_heavy} exceeded the wet it came from"
    );
}

#[test]
fn mix_outside_the_range_is_clamped_not_extrapolated() {
    assert_eq!(mix_dry_wet(1.0, 0.0, -1.0), 1.0);
    assert_eq!(mix_dry_wet(1.0, 0.0, 2.0), 0.0);
}

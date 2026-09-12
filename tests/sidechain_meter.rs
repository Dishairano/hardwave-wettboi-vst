//! What the sidechain tells the editor.
//!
//! The editor used to get only a duck depth, which is why the display could
//! show that it was ducking but never why: no key level, no threshold, nothing
//! to compare. These are the two numbers a person needs to read it.

use hardwave_wettboi::dsp::sidechain::SidechainDetector;

const SR: f32 = 48_000.0;

fn db(lin: f32) -> f32 {
    if lin <= 1e-9 {
        -180.0
    } else {
        20.0 * lin.log10()
    }
}

#[test]
fn the_threshold_is_reported_in_the_same_units_as_the_level() {
    let mut d = SidechainDetector::new(SR);
    d.set_params(-18.0, 2.0, 50.0, 200.0);
    let t = d.threshold_linear();
    assert!(
        (db(t) + 18.0).abs() < 0.01,
        "threshold read back as {} dB",
        db(t)
    );
}

#[test]
fn the_key_level_follows_the_signal() {
    let mut d = SidechainDetector::new(SR);
    d.set_params(-18.0, 2.0, 50.0, 200.0);
    assert!(d.key_level() < 1e-6, "starts at silence");

    // A kick-shaped burst: loud, then nothing.
    for _ in 0..480 {
        d.process(0.8);
    }
    let peak = d.key_level();
    assert!(
        (peak - 0.8).abs() < 0.01,
        "level read {peak}, expected about 0.8"
    );

    // It falls back, but not instantly: a meter you cannot read is no meter.
    for _ in 0..(SR as usize / 100) {
        d.process(0.0);
    }
    let after_10ms = d.key_level();
    assert!(after_10ms < peak, "level did not fall at all");
    assert!(
        after_10ms > peak * 0.5,
        "level fell too fast to read: {after_10ms}"
    );
}

#[test]
fn it_ducks_only_once_the_key_is_over_the_threshold() {
    let mut d = SidechainDetector::new(SR);
    d.set_params(-18.0, 1.0, 10.0, 50.0);

    // -30 dB is well under the threshold: nothing should happen.
    let quiet = 10.0_f32.powf(-30.0 / 20.0);
    for _ in 0..4800 {
        d.process(quiet);
    }
    assert!(
        d.current_depth() < 0.01,
        "ducked on a signal below the threshold"
    );

    // -6 dB is well over it.
    let loud = 10.0_f32.powf(-6.0 / 20.0);
    for _ in 0..4800 {
        d.process(loud);
    }
    assert!(
        d.current_depth() > 0.5,
        "did not duck on a signal over the threshold"
    );
}

#[test]
fn a_reset_clears_the_meter_too() {
    let mut d = SidechainDetector::new(SR);
    for _ in 0..480 {
        d.process(0.9);
    }
    d.reset();
    assert!(
        d.key_level() < 1e-6,
        "the meter kept a level from before the reset"
    );
    assert!(d.current_depth() < 1e-6);
}

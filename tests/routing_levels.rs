//! Are the three routing modes level-matched?
//!
//! The founder's report was that Parallel is louder than the other modes. These
//! measure it instead of arguing about it: fake reverb and delay with known
//! gains, so every number below is arithmetic, not taste.

use hardwave_wettboi::params::RoutingMode;
use hardwave_wettboi::route_wet;

/// A reverb that returns its input on both channels, so a gain of 1.0 in means
/// 1.0 out and any level change is the routing's doing, not the reverb's.
fn unity_reverb(mono: f32) -> (f32, f32) {
    (mono, mono)
}

/// A delay whose "tap" is simply its input, for the same reason.
fn unity_delay(l: f32, r: f32) -> (f32, f32) {
    (l, r)
}

fn wet(routing: RoutingMode, rev_on: bool, dly_on: bool, rev_wet: f32, dly_wet: f32) -> (f32, f32) {
    route_wet(
        routing,
        (1.0, 1.0),
        rev_on,
        dly_on,
        rev_wet,
        dly_wet,
        unity_reverb,
        unity_delay,
    )
}

#[test]
fn reverb_alone_is_the_same_level_in_every_mode() {
    let p = wet(RoutingMode::Parallel, true, false, 1.0, 0.0).0;
    let rd = wet(RoutingMode::ReverbToDelay, true, false, 1.0, 0.0).0;
    let dr = wet(RoutingMode::DelayToReverb, true, false, 1.0, 0.0).0;
    println!("reverb only: parallel {p:.3}, rev->dly {rd:.3}, dly->rev {dr:.3}");
    assert!(
        (p - rd).abs() < 1e-6,
        "rev->dly lost the reverb: {p} vs {rd}"
    );
    assert!(
        (p - dr).abs() < 1e-6,
        "dly->rev lost the reverb: {p} vs {dr}"
    );
}

#[test]
fn the_first_effect_in_a_serial_chain_is_still_audible() {
    // Reverb at full, delay present but contributing nothing. The reverb must
    // come through: before the fix this returned 0 because only the delay's
    // taps reached the output.
    let (l, _) = wet(RoutingMode::ReverbToDelay, true, true, 1.0, 0.0);
    println!("rev->dly with dly_wet at 0: {l:.3}");
    assert!(l > 0.9, "the reverb vanished into the delay stage: got {l}");

    let (l2, _) = wet(RoutingMode::DelayToReverb, true, true, 0.0, 1.0);
    println!("dly->rev with rev_wet at 0: {l2:.3}");
    assert!(
        l2 > 0.9,
        "the delay vanished into the reverb stage: got {l2}"
    );
}

#[test]
fn a_wet_control_is_not_applied_twice() {
    // At 50% each, a serial chain used to multiply both controls together and
    // deliver 25% of the first effect. Each control should scale its own stage.
    let (l, _) = wet(RoutingMode::ReverbToDelay, true, true, 0.5, 0.0);
    println!("rev at 50%, dly at 0%: {l:.3}, expected 0.500");
    assert!(
        (l - 0.5).abs() < 1e-6,
        "rev_wet was applied more than once: {l}"
    );
}

#[test]
fn parallel_is_not_louder_than_serial_for_the_same_settings() {
    let p = wet(RoutingMode::Parallel, true, true, 0.5, 0.5).0;
    let rd = wet(RoutingMode::ReverbToDelay, true, true, 0.5, 0.5).0;
    let dr = wet(RoutingMode::DelayToReverb, true, true, 0.5, 0.5).0;
    println!("both at 50%: parallel {p:.3}, rev->dly {rd:.3}, dly->rev {dr:.3}");
    // Parallel sums two independent sources, so it is allowed to be fuller, but
    // not by the factor of four the old code produced.
    assert!(p <= rd * 1.5 + 1e-6, "parallel {p} dwarfs rev->dly {rd}");
    assert!(p <= dr * 1.5 + 1e-6, "parallel {p} dwarfs dly->rev {dr}");
}

#[test]
fn a_disabled_effect_contributes_nothing_in_parallel() {
    let (l, r) = wet(RoutingMode::Parallel, false, false, 1.0, 1.0);
    println!("both off in parallel: {l:.3}, {r:.3}");
    assert_eq!((l, r), (0.0, 0.0), "a disabled effect still made sound");
}

#[test]
fn the_mix_control_trades_dry_for_wet_instead_of_adding() {
    use hardwave_wettboi::mix_dry_wet;
    // Fully dry, fully wet, and halfway. The halfway point must sit between the
    // two, never above both, which is what summing would do.
    let dry = 1.0;
    let wet_sig = 1.0;
    let at_0 = mix_dry_wet(dry, wet_sig, 0.0);
    let at_half = mix_dry_wet(dry, wet_sig, 0.5);
    let at_1 = mix_dry_wet(dry, wet_sig, 1.0);
    println!("mix 0 -> {at_0:.3}, mix 0.5 -> {at_half:.3}, mix 1 -> {at_1:.3}");
    assert!(
        (at_0 - 1.0).abs() < 1e-6,
        "mix at 0 should be the dry signal"
    );
    assert!(
        at_half <= 1.0 + 1e-6,
        "mix at halfway got louder than either side: {at_half}"
    );
    assert!(
        (at_1 - 1.0).abs() < 1e-6,
        "mix at 1 should be the wet signal"
    );
}

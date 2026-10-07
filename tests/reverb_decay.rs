//! Does the Decay knob mean what it says?
//!
//! Decay is shown in seconds, and a reverb time in seconds is an RT60: the time
//! the tail takes to fall 60 dB. The comb feedback used exp(-3 d / T), which
//! falls only 26 dB in T, so the tail ran about 2.3x too long. It also divided
//! comb lengths counted at 44.1 kHz by the real sample rate, which made it
//! longer again at 96 kHz, and a 0.98 ceiling on the feedback left the top of
//! the knob doing nothing. Measured on v0.4.8: 4.8 s for a 2.4 s setting at
//! 44.1 kHz and 10.4 s at 96 kHz.
//!
//! The RT60 here is measured the standard way: record the impulse response,
//! integrate its energy backwards (Schroeder), fit a straight line to the
//! decay curve between -5 and -35 dB and extrapolate it to 60 dB.

use hardwave_wettboi::dsp::reverb::{Reverb, ReverbType};

/// Impulse response energy (left squared plus right squared), with damping and
/// the input EQ out of the way so only the decay itself is measured.
fn impulse_energy(sr: f32, ty: ReverbType, decay: f32) -> Vec<f64> {
    // Set up the way the plug-in's initialize() does.
    let mut rv = Reverb::new(44_100.0);
    rv.set_sample_rate(sr);
    rv.reset();
    rv.set_type(ty);
    rv.set_freeze(false);
    // Default size, no damping, no pre-delay, EQ wide open.
    rv.set_params(65.0, decay, 0.0, 0.0);
    rv.set_eq(20.0, 20_000.0);

    let n = ((decay * 1.5 + 0.5) * sr) as usize;
    let mut e = Vec::with_capacity(n);
    for i in 0..n {
        let x = if i == 0 { 1.0 } else { 0.0 };
        let (l, r) = rv.process(x, 100.0);
        e.push((l as f64).powi(2) + (r as f64).powi(2));
    }
    e
}

/// RT60 from a least-squares line through the Schroeder decay curve between
/// -5 and -35 dB. NaN when the curve never gets that far.
fn rt60(e: &[f64], sr: f32) -> f64 {
    let mut edc = vec![0.0_f64; e.len()];
    let mut acc = 0.0;
    for i in (0..e.len()).rev() {
        acc += e[i];
        edc[i] = acc;
    }
    let total = edc[0];
    let (mut n, mut sx, mut sy, mut sxx, mut sxy) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for (i, &v) in edc.iter().enumerate() {
        let level = 10.0 * (v / total).log10();
        if level > -5.0 {
            continue;
        }
        if level < -35.0 {
            break;
        }
        let t = i as f64 / sr as f64;
        n += 1.0;
        sx += t;
        sy += level;
        sxx += t * t;
        sxy += t * level;
    }
    if n < 2.0 {
        return f64::NAN;
    }
    let slope = (n * sxy - sx * sy) / (n * sxx - sx * sx); // dB per second
    -60.0 / slope
}

fn check(ty: ReverbType) {
    let mut failures = Vec::new();
    for sr in [44_100.0_f32, 96_000.0] {
        for decay in [1.0_f32, 2.4, 6.0] {
            let measured = rt60(&impulse_energy(sr, ty, decay), sr);
            let off = (measured - decay as f64) / decay as f64;
            println!(
                "{ty:?} at {sr} Hz: Decay {decay} s measures {measured:.2} s ({:+.0}%)",
                off * 100.0
            );
            if off.is_nan() || off.abs() > 0.15 {
                failures.push(format!("{decay} s at {sr} Hz measured {measured:.2} s"));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{ty:?}: RT60 more than 15% off the Decay setting: {}",
        failures.join("; ")
    );
}

#[test]
fn room_decay_is_the_rt60() {
    check(ReverbType::Room);
}

#[test]
fn hall_decay_is_the_rt60() {
    check(ReverbType::Hall);
}

#[test]
fn plate_decay_is_the_rt60() {
    check(ReverbType::Plate);
}

#[test]
fn spring_decay_is_the_rt60() {
    check(ReverbType::Spring);
}

/// The comb feedback now goes higher than before, so the loops get the same
/// abuse test as the delay: absurd input at the longest, brightest settings
/// must come out finite and still die away.
#[test]
fn extreme_input_stays_finite_and_decays() {
    let sr = 44_100.0;
    for ty in [
        ReverbType::Room,
        ReverbType::Hall,
        ReverbType::Plate,
        ReverbType::Spring,
    ] {
        let mut rv = Reverb::new(sr);
        rv.set_type(ty);
        rv.set_params(100.0, 20.0, 0.0, 0.0);
        rv.set_eq(20.0, 20_000.0);
        let mut s = 7_u64;
        for _ in 0..sr as usize {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            let x = (((s >> 40) as f32 / (1u64 << 24) as f32) * 2.0 - 1.0) * 1.0e6;
            let (l, r) = rv.process(x, 200.0);
            assert!(l.is_finite() && r.is_finite(), "{ty:?}: non-finite output");
        }
        let mut first = 0.0_f32;
        let mut last = 0.0_f32;
        let n = 30 * sr as usize;
        for i in 0..n {
            let (l, r) = rv.process(0.0, 200.0);
            assert!(l.is_finite() && r.is_finite(), "{ty:?}: non-finite tail");
            let peak = l.abs().max(r.abs());
            if i < sr as usize {
                first = first.max(peak);
            }
            if i >= n - sr as usize {
                last = last.max(peak);
            }
        }
        let fall = 20.0 * (first / last.max(1e-30)).log10();
        println!("{ty:?}: tail fell {fall:.1} dB in 29 s at Decay 20 s");
        // 29 s of a 20 s RT60 is 87 dB; allow for the damping and the start.
        assert!(fall > 60.0, "{ty:?}: the tail fell only {fall:.1} dB");
    }
}

/// The ceiling on the comb feedback made everything above about 6.8 s sound
/// the same. Each step up the top of the range has to make the tail longer.
#[test]
fn long_settings_still_lengthen_the_tail() {
    let sr = 44_100.0;
    let mut last = 0.0;
    for decay in [6.0_f32, 10.0, 15.0] {
        let measured = rt60(&impulse_energy(sr, ReverbType::Room, decay), sr);
        println!("Room, Decay {decay} s: {measured:.2} s");
        assert!(
            measured > last * 1.2,
            "Decay {decay} s gave {measured:.2} s, no longer than the step below ({last:.2} s)"
        );
        last = measured;
    }
}

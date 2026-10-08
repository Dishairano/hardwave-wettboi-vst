//! Do the four reverb types sit at the same level?
//!
//! The diffusion allpasses used the form Freeverb made popular, which is only
//! close to an allpass at a coefficient of 0.5. Above that it boosts: Hall runs
//! its allpasses at 0.6 and Plate at 0.7, four in series. On pink noise the
//! wet sat 13.8 dB (Room) to 17.3 dB (Plate) above the dry, so the wet was far
//! too hot and switching type jumped the level by up to 4.4 dB.
//!
//! Changing type should change the character of the room, not the volume.

use hardwave_wettboi::dsp::reverb::{Reverb, ReverbType};

const TYPES: [ReverbType; 4] = [
    ReverbType::Room,
    ReverbType::Hall,
    ReverbType::Plate,
    ReverbType::Spring,
];

/// Repeatable pink noise: white noise (xorshift) through Paul Kellet's
/// three-pole pinking filter.
fn pink(n: usize, seed: u64) -> Vec<f32> {
    let mut s = seed;
    let (mut b0, mut b1, mut b2) = (0.0_f32, 0.0_f32, 0.0_f32);
    (0..n)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            let w = ((s >> 40) as f32 / (1u64 << 24) as f32) * 2.0 - 1.0;
            b0 = 0.99765 * b0 + w * 0.099_046;
            b1 = 0.963 * b1 + w * 0.296_516_4;
            b2 = 0.57 * b2 + w * 1.052_691_3;
            (b0 + b1 + b2 + w * 0.1848) * 0.12
        })
        .collect()
}

fn rms(xs: &[f32]) -> f64 {
    (xs.iter().map(|&v| (v as f64).powi(2)).sum::<f64>() / xs.len().max(1) as f64).sqrt()
}

#[derive(Clone, Copy, Debug)]
struct Settings {
    sr: f32,
    size: f32,
    decay: f32,
    damp: f32,
}

/// The plug-in's defaults: size 65, Decay 2.4 s, Damp 40.
const DEFAULTS: Settings = Settings {
    sr: 44_100.0,
    size: 65.0,
    decay: 2.4,
    damp: 40.0,
};

/// Wet level relative to the dry, in dB, on 6 s of pink noise. Measured over
/// the second half, once the tail has built up.
fn wet_level_db(ty: ReverbType, s: Settings) -> f64 {
    let mut rv = Reverb::new(44_100.0);
    rv.set_sample_rate(s.sr);
    rv.reset();
    rv.set_type(ty);
    rv.set_freeze(false);
    // Pre-delay, width and EQ at the plug-in's defaults.
    rv.set_params(s.size, s.decay, s.damp, 18.0);
    rv.set_eq(20.0, 18_000.0);

    let n = (6.0 * s.sr) as usize;
    let input = pink(n, 21);
    let mut l = Vec::with_capacity(n);
    let mut r = Vec::with_capacity(n);
    for &x in &input {
        let (a, b) = rv.process(x, 120.0);
        l.push(a);
        r.push(b);
    }
    let half = n / 2;
    let wet = ((rms(&l[half..]).powi(2) + rms(&r[half..]).powi(2)) / 2.0).sqrt();
    20.0 * (wet / rms(&input[half..])).log10()
}

fn spread(s: Settings) -> (Vec<f64>, f64) {
    let levels: Vec<f64> = TYPES.iter().map(|&ty| wet_level_db(ty, s)).collect();
    let hi = levels.iter().cloned().fold(f64::MIN, f64::max);
    let lo = levels.iter().cloned().fold(f64::MAX, f64::min);
    let line: Vec<String> = TYPES
        .iter()
        .zip(&levels)
        .map(|(ty, db)| format!("{ty:?} {db:+.1} dB"))
        .collect();
    println!("{s:?}: {} (spread {:.2} dB)", line.join(", "), hi - lo);
    (levels, hi - lo)
}

#[test]
fn the_types_sit_within_1_db_at_the_defaults() {
    let (_, sp) = spread(DEFAULTS);
    assert!(
        sp <= 1.0,
        "switching reverb type moves the level by {sp:.2} dB"
    );
}

/// The match must not be tuned to one setting only.
#[test]
fn the_types_sit_within_1_db_across_settings() {
    let cases = [
        Settings {
            size: 30.0,
            decay: 1.0,
            ..DEFAULTS
        },
        Settings {
            size: 100.0,
            decay: 6.0,
            ..DEFAULTS
        },
        Settings {
            damp: 0.0,
            ..DEFAULTS
        },
        Settings {
            sr: 96_000.0,
            ..DEFAULTS
        },
    ];
    let mut failures = Vec::new();
    for s in cases {
        let (_, sp) = spread(s);
        if sp > 1.0 {
            failures.push(format!("{s:?}: {sp:.2} dB"));
        }
    }
    assert!(
        failures.is_empty(),
        "reverb types more than 1 dB apart: {}",
        failures.join("; ")
    );
}

/// At the default settings and Rev Wet 100% the wet used to sit 14 to 17 dB
/// over the dry. It should be in the same neighbourhood as the dry, so Rev Wet
/// and Mix behave like level controls rather than ways of taming a boost.
#[test]
fn the_wet_is_not_hotter_than_the_dry_at_the_defaults() {
    let (levels, _) = spread(DEFAULTS);
    for (ty, db) in TYPES.iter().zip(levels) {
        assert!(db <= 3.0, "{ty:?} wet sits {db:+.1} dB over the dry");
    }
}

//! Where does the echo actually land?
//!
//! Feed one impulse into each channel and find the first echo. With L set to
//! 100 ms and R to 200 ms at 48 kHz, the taps belong at 4800 and 9600 samples.

use hardwave_wettboi::dsp::delay::StereoDelay;

const SR: f32 = 48_000.0;

/// Index of the first sample after the impulse whose level is worth calling an echo.
fn first_echo(xs: &[f32]) -> Option<usize> {
    xs.iter()
        .enumerate()
        .skip(2)
        .find(|(_, v)| v.abs() > 0.05)
        .map(|(i, _)| i)
}

fn run(time_l_ms: f32, time_r_ms: f32, ping_pong: bool, n: usize) -> (Vec<f32>, Vec<f32>) {
    let mut d = StereoDelay::new(SR);
    d.set_time_ms(time_l_ms, time_r_ms);
    d.set_feedback(0.0);
    d.set_filter(20.0, 20_000.0);
    d.set_ping_pong(ping_pong);
    d.set_modulation(0.5, 0.0);
    d.set_saturation(0.0);

    let mut out_l = Vec::with_capacity(n);
    let mut out_r = Vec::with_capacity(n);
    for i in 0..n {
        let x = if i == 0 { 1.0 } else { 0.0 };
        let (l, r) = d.process(x, x);
        out_l.push(l);
        out_r.push(r);
    }
    (out_l, out_r)
}

#[test]
fn each_channel_echoes_at_its_own_time() {
    let (l, r) = run(100.0, 200.0, false, 15_000);
    let el = first_echo(&l).expect("no echo on the left at all");
    let er = first_echo(&r).expect("no echo on the right at all");
    println!(
        "left echo at {el} samples ({:.1} ms), expected 4800 (100.0 ms)",
        el as f32 / SR * 1000.0
    );
    println!(
        "right echo at {er} samples ({:.1} ms), expected 9600 (200.0 ms)",
        er as f32 / SR * 1000.0
    );
    assert!(
        (el as i64 - 4800).abs() <= 2,
        "left echo landed at {el}, not 4800"
    );
    assert!(
        (er as i64 - 9600).abs() <= 2,
        "right echo landed at {er}, not 9600"
    );
}

#[test]
fn the_times_are_not_swapped() {
    let (l, r) = run(50.0, 400.0, false, 25_000);
    let el = first_echo(&l).expect("no echo on the left");
    let er = first_echo(&r).expect("no echo on the right");
    println!("short side {el} samples, long side {er} samples");
    assert!(
        el < er,
        "L was set shorter than R but echoed later ({el} vs {er}): channels are swapped"
    );
}

#[test]
fn changing_the_time_moves_the_echo() {
    let (a, _) = run(100.0, 100.0, false, 15_000);
    let (b, _) = run(200.0, 200.0, false, 20_000);
    let ea = first_echo(&a).expect("no echo at 100 ms");
    let eb = first_echo(&b).expect("no echo at 200 ms");
    println!("100 ms -> {ea}, 200 ms -> {eb}");
    assert!(
        eb > ea + 4000,
        "doubling the time barely moved the echo: {ea} then {eb}"
    );
}

#[test]
fn tempo_sync_sets_a_real_time() {
    let mut d = StereoDelay::new(SR);
    d.set_feedback(0.0);
    d.set_filter(20.0, 20_000.0);
    d.set_ping_pong(false);
    d.set_modulation(0.5, 0.0);
    d.set_saturation(0.0);
    // 120 BPM, an eighth note = 0.5 beats = 250 ms = 12000 samples at 48k.
    d.set_time_sync(120.0, 0.5, 0.5);
    let mut out = Vec::with_capacity(20_000);
    for i in 0..20_000 {
        let x = if i == 0 { 1.0 } else { 0.0 };
        out.push(d.process(x, x).0);
    }
    let e = first_echo(&out).expect("tempo sync produced no echo");
    println!("eighth at 120 BPM -> {e} samples, expected 12000");
    assert!(
        (e as i64 - 12_000).abs() <= 2,
        "sync echo landed at {e}, not 12000"
    );
}

/// Ping-pong means the repeats alternate sides. Feed the left channel only and
/// look at which side each successive echo comes out of.
#[test]
fn ping_pong_alternates_sides() {
    let mut d = StereoDelay::new(SR);
    d.set_time_ms(100.0, 100.0);
    d.set_feedback(60.0);
    d.set_filter(20.0, 20_000.0);
    d.set_ping_pong(true);
    d.set_modulation(0.5, 0.0);
    d.set_saturation(0.0);

    let n = 48_000; // one second, so four echoes at 100 ms
    let mut l = Vec::with_capacity(n);
    let mut r = Vec::with_capacity(n);
    for i in 0..n {
        let x = if i == 0 { 1.0 } else { 0.0 };
        let (a, b) = d.process(x, 0.0); // left only
        l.push(a);
        r.push(b);
    }

    // Which side is louder around each echo?
    let mut sides = Vec::new();
    for k in 1..=4 {
        let at = k * 4800;
        let win = 64;
        let pl: f32 = l[at - win..at + win]
            .iter()
            .fold(0.0, |m, v| m.max(v.abs()));
        let pr: f32 = r[at - win..at + win]
            .iter()
            .fold(0.0, |m, v| m.max(v.abs()));
        sides.push(if pl > pr {
            'L'
        } else if pr > pl {
            'R'
        } else {
            '-'
        });
        println!("echo {k} at {at}: L peak {pl:.4}, R peak {pr:.4}");
    }
    let pattern: String = sides.iter().collect();
    println!("pattern: {pattern}");
    assert_eq!(
        pattern, "LRLR",
        "ping-pong should alternate sides, got {pattern}"
    );
}

/// Ping-pong with different times set per side, which is what the plugin ships
/// with by default: tempo mode, L an eighth and R a dotted eighth. The repeats
/// used to land at 250, 625, 875, 1250 ms, gaps of 375, 250, 375: a limp, not a
/// groove. Ping-pong bounces ONE time between the speakers, so the spacing has
/// to be even whatever the right-hand control says.
#[test]
fn ping_pong_stays_even_when_the_two_times_differ() {
    let mut d = StereoDelay::new(SR);
    // 120 BPM: eighth = 250 ms, dotted eighth = 375 ms. The shipped defaults.
    d.set_time_ms(250.0, 375.0);
    d.set_feedback(60.0);
    d.set_filter(20.0, 20_000.0);
    d.set_ping_pong(true);
    d.set_modulation(0.5, 0.0);
    d.set_saturation(0.0);

    let n = 96_000;
    let mut l = Vec::with_capacity(n);
    let mut r = Vec::with_capacity(n);
    for i in 0..n {
        let x = if i == 0 { 1.0 } else { 0.0 };
        let (a, b) = d.process(x, 0.0);
        l.push(a);
        r.push(b);
    }

    // Collect every echo above a useful level, with its time and side.
    let mut hits: Vec<(usize, char, f32)> = Vec::new();
    let mut i = 2;
    while i < n {
        let (a, b) = (l[i].abs(), r[i].abs());
        if a > 0.02 || b > 0.02 {
            hits.push((i, if a > b { 'L' } else { 'R' }, a.max(b)));
            i += 200; // skip past this echo
        } else {
            i += 1;
        }
    }
    for (at, side, amp) in &hits {
        println!(
            "  echo at {:>6} ({:>7.1} ms)  {}  {:.3}",
            at,
            *at as f32 / SR * 1000.0,
            side,
            amp
        );
    }

    // Gaps between successive echoes. A ping-pong is expected to be even.
    let gaps: Vec<i64> = hits
        .windows(2)
        .map(|w| w[1].0 as i64 - w[0].0 as i64)
        .collect();
    println!("  gaps in samples: {gaps:?}");
    let even = gaps.windows(2).all(|g| (g[0] - g[1]).abs() <= 48); // 1 ms tolerance
    assert!(even, "ping-pong repeats are unevenly spaced: {gaps:?}");
}

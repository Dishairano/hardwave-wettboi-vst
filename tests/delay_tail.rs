//! Does the delay die out once the input stops?
//!
//! The saturator on the feedback path was tanh(x * drive) / tanh(drive). That
//! keeps the loudest repeat at full scale, but on a quiet one its gain is
//! drive / tanh(drive): 5x at 100% saturation. With the feedback at 35% the
//! loop gain was then 1.75, so the repeats grew until the tanh held them. The
//! wet sat at -10.5 dBFS (35% feedback) and -0.6 dBFS (95%) after 20 s of
//! silence and never went away.
//!
//! A saturator inside a feedback loop must never add gain. Quiet repeats then
//! fade at the rate the Feedback control sets, and loud ones a little faster.

use hardwave_wettboi::dsp::delay::StereoDelay;

const SR: f32 = 44_100.0;

/// Repeatable white noise in -1..1 (xorshift), so a failure can be re-run.
struct Noise(u64);

impl Noise {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        ((self.0 >> 40) as f32 / (1u64 << 24) as f32) * 2.0 - 1.0
    }
}

fn dbfs(x: f32) -> f32 {
    20.0 * x.max(1e-30).log10()
}

/// One second of stereo noise peaking at -6 dBFS, then `silence_s` seconds of
/// nothing. Returns the peak of the delay's output over the last half second,
/// and over the second right after the noise stops, both in dBFS.
fn tail(time_ms: f32, feedback: f32, saturation: f32, silence_s: f32) -> (f32, f32) {
    let mut d = StereoDelay::new(SR);
    d.set_sample_rate(SR);
    d.reset();
    d.set_time_ms(time_ms, time_ms);
    d.set_feedback(feedback);
    // The plug-in's default feedback filter and ping-pong setting.
    d.set_filter(120.0, 8000.0);
    d.set_ping_pong(true);
    d.set_modulation(0.5, 0.0);
    d.set_saturation(saturation);

    let noise_len = SR as usize;
    let n = noise_len + (silence_s * SR) as usize;
    let last = n - (SR * 0.5) as usize;
    let mut rng = Noise(31);
    let mut early = 0.0_f32;
    let mut late = 0.0_f32;
    for i in 0..n {
        let (l, r) = if i < noise_len {
            (rng.next() * 0.5, rng.next() * 0.5)
        } else {
            (0.0, 0.0)
        };
        let (a, b) = d.process(l, r);
        assert!(a.is_finite() && b.is_finite(), "non-finite output at {i}");
        let peak = a.abs().max(b.abs());
        if (noise_len..2 * noise_len).contains(&i) {
            early = early.max(peak);
        }
        if i >= last {
            late = late.max(peak);
        }
    }
    (dbfs(early), dbfs(late))
}

/// The job's case: 95% feedback, 100% saturation, 1 s of noise and 20 s of
/// silence. A 95% loop loses about 0.45 dB a repeat plus whatever the feedback
/// filter takes, so it needs roughly 200 repeats to fall 90 dB. At 100 ms that
/// fits in 20 s; at a longer time the tail is longer, which is the Feedback
/// control doing its job. Before the fix this sat just under full scale.
#[test]
fn full_feedback_and_saturation_dies_out() {
    let (early, late) = tail(100.0, 95.0, 100.0, 20.0);
    println!("fb 95% sat 100% @ 100 ms: first second after the noise {early:+.1} dBFS, last 0.5 s of 20 s {late:+.1} dBFS");
    assert!(
        late < -90.0,
        "the delay is still at {late:.1} dBFS after 20 s of silence"
    );
}

/// The founder's other measurement, at the plug-in's default time (an eighth
/// at 150 BPM) and default feedback: it held at -10.5 dBFS for good.
#[test]
fn default_feedback_with_saturation_dies_out() {
    let (early, late) = tail(200.0, 35.0, 100.0, 20.0);
    println!("fb 35% sat 100% @ 200 ms: first second after the noise {early:+.1} dBFS, last 0.5 s of 20 s {late:+.1} dBFS");
    assert!(
        late < -90.0,
        "the delay is still at {late:.1} dBFS after 20 s of silence"
    );
}

/// Saturation may round off a repeat but must never make the tail longer than
/// the same feedback without it.
#[test]
fn saturation_never_lengthens_the_tail() {
    for fb in [35.0, 60.0, 95.0] {
        let (_, clean) = tail(100.0, fb, 0.0, 5.0);
        for sat in [25.0, 50.0, 100.0] {
            let (_, driven) = tail(100.0, fb, sat, 5.0);
            println!("fb {fb}%: after 5 s, sat 0% {clean:+.1} dBFS, sat {sat}% {driven:+.1} dBFS");
            assert!(
                driven <= clean + 0.5,
                "saturation {sat}% left the fb {fb}% tail at {driven:.1} dBFS, above the clean {clean:.1} dBFS"
            );
        }
    }
}

/// A feedback loop has to survive absurd input. Full-scale-times-a-million
/// noise must come out finite and must still die away afterwards.
#[test]
fn extreme_input_stays_finite_and_decays() {
    for sat in [0.0, 100.0] {
        let mut d = StereoDelay::new(SR);
        d.set_time_ms(50.0, 50.0);
        d.set_feedback(95.0);
        d.set_filter(20.0, 20_000.0);
        d.set_ping_pong(false);
        d.set_modulation(10.0, 100.0);
        d.set_saturation(sat);
        let mut rng = Noise(7);
        for _ in 0..SR as usize {
            let (a, b) = d.process(rng.next() * 1.0e6, rng.next() * 1.0e6);
            assert!(
                a.is_finite() && b.is_finite(),
                "sat {sat}%: non-finite output"
            );
        }
        let mut late = 0.0_f32;
        for i in 0..(30.0 * SR) as usize {
            let (a, b) = d.process(0.0, 0.0);
            assert!(
                a.is_finite() && b.is_finite(),
                "sat {sat}%: non-finite tail"
            );
            if i >= (29.0 * SR) as usize {
                late = late.max(a.abs()).max(b.abs());
            }
        }
        println!("sat {sat}%: after 1e6 noise and 30 s of silence, peak {late:e}");
        // 600 repeats at 95% is over 260 dB down, so -60 dBFS is generous.
        assert!(
            late < 1.0e-3,
            "sat {sat}%: the loop did not decay, peak {late}"
        );
    }
}

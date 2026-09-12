//! How dry and wet are combined, and how parallel engines are summed.
//!
//! Both of these were arithmetic done inline in `process`, and both were wrong
//! in a way that only showed up as "it gets louder":
//!
//! * Mix was `dry * (1 - mix) + (dry + wet) * mix`, which expands to
//!   `dry + wet * mix`. The dry signal stayed at full level and the wet was
//!   added on top, so raising Mix always raised the output. A Mix control is a
//!   crossfade; it should not change the total level of a dry signal.
//! * Parallel summed the reverb and the delay outright, so switching from a
//!   serial mode to Parallel jumped in level with nothing else changed.

/// Crossfade between dry and wet. `mix` is 0.0 (all dry) to 1.0 (all wet).
#[inline]
pub fn mix_dry_wet(dry: f32, wet: f32, mix: f32) -> f32 {
    let m = mix.clamp(0.0, 1.0);
    dry * (1.0 - m) + wet * m
}

/// Level compensation for summing `active` parallel engines.
///
/// Reverb tails and delay repeats are largely uncorrelated, so their powers add
/// rather than their amplitudes: two sources at the same level sum to about
/// +3 dB, not +6. Dividing by the square root of the count keeps the loudness
/// roughly where one engine alone put it, which is what makes switching routing
/// modes feel like a routing change instead of a volume change.
#[inline]
pub fn parallel_gain(active: usize) -> f32 {
    match active {
        0 | 1 => 1.0,
        n => 1.0 / (n as f32).sqrt(),
    }
}

#[cfg(test)]
mod tests {
    use super::{mix_dry_wet, parallel_gain};

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1.0e-5
    }

    #[test]
    fn mix_at_zero_is_the_dry_signal_untouched() {
        assert!(close(mix_dry_wet(0.8, 0.5, 0.0), 0.8));
    }

    #[test]
    fn mix_at_one_is_the_wet_signal_only() {
        assert!(close(mix_dry_wet(0.8, 0.5, 1.0), 0.5));
    }

    #[test]
    fn mix_never_exceeds_the_louder_of_the_two() {
        // The old formula returned dry + wet * mix, so this was 1.3 at mix=1.
        for m in [0.0, 0.25, 0.5, 0.75, 1.0] {
            let out = mix_dry_wet(0.8, 0.5, m);
            assert!(
                out <= 0.8 + 1.0e-6,
                "mix {m} produced {out}, louder than the dry input"
            );
        }
    }

    #[test]
    fn turning_mix_up_on_silence_stays_silent() {
        assert!(close(mix_dry_wet(0.0, 0.0, 1.0), 0.0));
    }

    #[test]
    fn mix_is_clamped_so_automation_cannot_overshoot() {
        assert!(close(mix_dry_wet(1.0, 0.0, -0.5), 1.0));
        assert!(close(mix_dry_wet(1.0, 0.0, 1.5), 0.0));
    }

    #[test]
    fn one_engine_is_not_attenuated() {
        assert!(close(parallel_gain(1), 1.0));
        assert!(close(parallel_gain(0), 1.0));
    }

    #[test]
    fn two_engines_come_back_about_three_db() {
        let g = parallel_gain(2);
        let db = 20.0 * g.log10();
        assert!((db + 3.0103).abs() < 0.01, "expected about -3 dB, got {db}");
    }
}

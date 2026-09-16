# Changelog

## 0.4.1

Two faults in how the plug-in hands its settings back to your DAW, both found by
the automated testers rather than reported by anyone.

- **Your settings came back, but the DAW did not know.** Reopening a project
  restored every control inside WettBoi, and the plug-in never told the host to
  re-read them. A host that trusts its own copy showed and automated the old
  values, so a project could sound different from what the controls said.
- **A damaged project file could take the whole DAW down.** Loading a corrupt or
  foreign state made the plug-in ask for an impossible amount of memory, and the
  failed request killed the host process, not just the plug-in. It now refuses
  the state and carries on.
- Crash reports from our own test runs no longer reach the crash dashboard, so a
  real crash is not buried under our noise.

## 0.4.0

Four things the founder reported, and the causes behind them.

- **The delay's time controls did nothing in the default mode.** Tempo sync is on
  by default, which makes the note buttons the only visible time control, and the
  plugin had no parser entry for them: every click and every preset that set one
  was silently discarded. The note divisions now reach the parameter, and a test
  walks the whole UI vocabulary so a missing entry fails the build instead of
  going quiet.
- **Ping-pong limped instead of bouncing.** It crossed the feedback between two
  delay lines running at different times, so with the shipped defaults (an eighth
  on the left, a dotted eighth on the right) the repeats landed 375 ms and 250 ms
  apart in turn and never settled into the beat. A ping-pong is one time bouncing
  between the speakers, so it now uses one. Switch ping-pong off and the two times
  are independent again, which is a dual delay and a different instrument.
- **Parallel routing was louder than the other modes**, for two reasons. The mix
  control was adding the wet on top of a dry that stayed at full level, so raising
  Mix always raised the output. And both serial modes lost their first effect
  entirely: `Reverb -> Delay` returned only the echoes of the reverb, never the
  reverb, and attenuated them by both wet controls at once, so 50% and 50% arrived
  at 25%. Mix is now a crossfade, and every mode passes the same rule: the wet is
  the sum of the enabled effects, each at its own level. Parallel still sums two
  effects, which is what parallel means, and the UI now says so.
- **The sidechain showed how much it was ducking but never why.** Added a meter
  that draws the key signal against the threshold on one decibel scale, with the
  distance over the threshold and the gain reduction in decibels. The threshold
  knob also reaches -60 dB now, matching the parameter; it stopped at -40, so a
  third of the range could not be reached.
- **Seven presets added**, filling real gaps: there was no delay-only starting
  point at all, and nothing for uptempo tempos, vocals, snares, or a build.

Behaviour note: because Mix is now a crossfade, an existing project will load
quieter than before at the same setting. That is the bug being fixed, not a new
one, but it is audible and worth knowing before you open an old session.

Under the hood: the DSP is now reachable from tests (the crate was `cdylib` only,
so the audio code had never had a test run against it). 36 tests cover the delay
timing, the routing levels, the sidechain metering and the UI vocabulary.

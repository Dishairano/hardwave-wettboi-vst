# Changelog

## 0.4.4

Saved settings come back the way you left them, a damaged project cannot take the
plug-in down with it, and a window that stays empty now leaves a log we can read.

- **Your DAW and WettBoi disagreed about every control after reopening a project.**
  The values were restored correctly inside the plug-in, but nothing told the host
  to read them again, so the DAW kept showing and automating what it believed a
  fresh instance held. Reopening a project now refreshes the host's own view.
- **A typed value could land one step away from itself.** A control with no
  rounding printed its raw number, all nine digits, so reading that text back gave
  a slightly different value and printing it again gave a different string. Values
  round to what the control can actually hold.
- **A damaged or foreign saved state is refused instead of crashing.** Loading
  random bytes used to abort the whole host process.
- **A window that stays empty now leaves a log we can read.** Everything the
  editor knew is written to a file: whether it found your licence token, which
  address it loaded, whether the WebView was created, whether the interface
  answered. All of it used to go to a console no DAW shows. It is now also
  appended to `%APPDATA%\hardwave\wettboi-editor.log` (on macOS
  `~/Library/Application Support/hardwave/wettboi-editor.log`). Send that file
  with a report and we can usually see the cause in it. It holds no audio, no
  project data and no licence token.
- **A slow network no longer replaces a working interface with an apology.**
  Before the window opens, the plug-in asks whether the interface can be
  reached, and anything short of an answer counted as offline: a timeout, a
  proxy, a firewall that allows the browser but not the DAW. Only a refused
  connection or a name that does not resolve counts now. Everything else loads
  the interface and lets the WebView try, because it often gets through where
  we do not.

## 0.4.3

A crash when a DAW loads the plug-in, unloads it and loads it again, reported
from the field with a crash dump by a producer running MPC desktop.

- **WettBoi could take MPC down when its window was opened a second time.** The
  editor's webview registers a Win32 window class from inside the plug-in, and
  such a class outlives the plug-in that registered it. When MPC unloaded WettBoi
  and loaded it again, the new window still pointed at code in the old, freed
  copy, and the first message it received crashed the host. WettBoi now keeps
  itself loaded for as long as the host runs, so that cannot happen. Windows
  only; nothing changes on macOS or Linux. The same guard went into every
  Hardwave plug-in.
- **Our own test panics no longer show up as crash reports.** They were being
  sent as if a released build had crashed on somebody's machine.

## 0.4.2

Typing a number into a control now works everywhere, found by the automated
testers rather than reported by anyone.

- **Typed values were ignored on most controls.** Only a control whose text
  happened to match its own unit accepted a typed number, so anything that
  prints its own format (1.2 kHz, -12.0 dB, 75 %) simply refused what you
  typed and snapped back. Every control takes a typed number now, with or
  without the unit.
- **A displayed value did not always survive being typed back in.** A control
  with no formatter printed its raw value (1.2286583 ms), which does not
  round trip. Values now print to six significant digits.

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

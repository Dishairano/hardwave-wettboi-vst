//! Hardwave WettBoi — sidechain reverb & delay VST3/CLAP plugin.
//!
//! Signal chain (parallel mode):
//!   Input → Reverb (wet) + Delay (wet) → Sidechain Duck → LFO Modulation → Mix → Output
//! Serial modes:
//!   Rev→Dly: Input → Reverb → Delay → Sidechain → Mix → Output
//!   Dly→Rev: Input → Delay → Reverb → Sidechain → Mix → Output

#![allow(clippy::type_complexity, clippy::too_many_arguments)]

use crossbeam_channel::{Receiver, Sender};
use nih_plug::prelude::*;
use parking_lot::Mutex;
use std::sync::Arc;

mod auth;
mod clap_export;
#[macro_use]
pub mod diag;
pub mod dsp;
pub mod editor;
pub mod params;
mod protocol;

// The CLAP entry point. `clap_export` stands in for `nih_export_clap!` so the
// `clap.state` extension can check a saved state's length before the framework
// allocates for it, and so a finished load tells the host to rescan the
// parameter values. See that module for why both have to happen out there.
pub use clap_export::clap_entry;

use dsp::lfo::Shape as LfoShape;
use dsp::reverb::ReverbType as DspReverbType;
use dsp::{Lfo, Reverb, SidechainDetector, StereoDelay};
use nih_plug::wrapper::state::ParamValue;
use params::{LfoTarget, RoutingMode, WettBoiParams};
use protocol::WbPacket;

// ─── Crash handler ───────────────────────────────────────────────────────────
//
// v0.3.9 fallback build: identical runtime behaviour to v0.3.7, plus this
// panic hook. A panic crossing the FFI boundary into a VST host is undefined
// behaviour and almost always crashes the host. Installing this hook means
// any panic we miss in the editor / IPC paths gets logged to
// %APPDATA%\hardwave\wettboi-crash.log instead of being lost. The previous
// v0.3.8 had this too, but also changed Drop / WebView2 dir layout / FFI
// error returns — for customers where v0.3.8 regressed we fall back here.

fn hardwave_data_dir() -> std::path::PathBuf {
    dirs::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("hardwave")
}

fn crash_log_path() -> std::path::PathBuf {
    hardwave_data_dir().join("wettboi-crash.log")
}

fn crash_pending_path() -> std::path::PathBuf {
    hardwave_data_dir().join("wettboi-crash-pending")
}

mod crash_reporter;

fn install_crash_handler() {
    use std::sync::Once;
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            use std::io::Write;
            let path = crash_log_path();
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
            {
                let ts = unix_timestamp();
                let payload = if let Some(s) = info.payload().downcast_ref::<&str>() {
                    (*s).to_string()
                } else if let Some(s) = info.payload().downcast_ref::<String>() {
                    s.clone()
                } else {
                    "unknown panic".to_string()
                };
                let location = info
                    .location()
                    .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
                    .unwrap_or_else(|| "unknown location".to_string());
                let bt = std::backtrace::Backtrace::force_capture();

                let _ = writeln!(f, "========================================");
                let _ = writeln!(f, "HARDWAVE WETTBOI CRASH REPORT");
                let _ = writeln!(f, "Time:     {}", ts);
                let _ = writeln!(f, "Version:  {}", env!("CARGO_PKG_VERSION"));
                let _ = writeln!(f, "OS:       {}", std::env::consts::OS);
                let _ = writeln!(f, "Arch:     {}", std::env::consts::ARCH);
                let _ = writeln!(f, "Location: {}", location);
                let _ = writeln!(f, "Message:  {}", payload);
                let _ = writeln!(f);
                let _ = writeln!(f, "Backtrace:");
                let _ = writeln!(f, "{}", bt);
                let _ = writeln!(f, "========================================");
                let _ = writeln!(f);
            }
            let pending = crash_pending_path();
            if let Some(parent) = pending.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let crash_ts = unix_timestamp();
            let _ = std::fs::write(
                &pending,
                format!("wettboi\n{}\n{}", env!("CARGO_PKG_VERSION"), crash_ts),
            );
            prev(info);
        }));
    });
}

fn unix_timestamp() -> String {
    let dur = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{} (unix)", dur.as_secs())
}

struct HardwaveWettBoi {
    params: Arc<WettBoiParams>,

    // DSP modules
    reverb: Reverb,
    delay: StereoDelay,
    sidechain: SidechainDetector,
    lfo: Lfo,

    // Editor communication
    editor_packet_tx: Sender<WbPacket>,
    editor_packet_rx: Arc<Mutex<Receiver<WbPacket>>>,
    update_counter: u32,

    // State
    sample_rate: f32,
    bpm: f32,
    duck_depth: f32,
    lfo_value: f32,
    /// Set per-buffer when any DSP sample evaluates to non-finite. The buffer
    /// still ships clean dry audio (graceful fallback) and we reset the
    /// affected DSP modules at end-of-buffer so the next pass starts clean.
    nan_detected: bool,

    // Metering
    input_peak_l: f32,
    input_peak_r: f32,
    output_peak_l: f32,
    output_peak_r: f32,
}

/// Build the wet signal for one sample, according to the routing mode.
///
/// `reverb` takes a mono sample and returns a stereo pair; `delay` takes a
/// stereo pair and returns its delayed taps only, not its input. Passing them in
/// as closures keeps this pure, so the levels can be measured in a test rather
/// than guessed at by ear.
///
/// One rule, in every mode: **the wet signal is the sum of the enabled effects,
/// each at its own level, and nothing else.** A disabled effect adds nothing and
/// its level control does nothing. Dry never appears here; the mix control adds
/// it back.
///
/// The serial modes broke both halves of that rule. `Reverb -> Delay` returned
/// `delay(rev * rev_wet) * dly_wet`, so the reverb itself never reached the
/// output (only its echoes did), and what got through was attenuated twice: at
/// 50% and 50% it arrived at 25%. `Delay -> Reverb` had the mirror image of the
/// same fault. That is why Parallel sounded so much louder than the other two.
/// Parallel was not too loud; the serial modes were quietly losing an effect.
#[allow(clippy::too_many_arguments)]
pub fn route_wet<R, D>(
    routing: RoutingMode,
    dry: (f32, f32),
    rev_enabled: bool,
    dly_enabled: bool,
    rev_wet: f32,
    dly_wet: f32,
    mut reverb: R,
    mut delay: D,
) -> (f32, f32)
where
    R: FnMut(f32) -> (f32, f32),
    D: FnMut(f32, f32) -> (f32, f32),
{
    let (dry_l, dry_r) = dry;
    let mono = |l: f32, r: f32| (l + r) * 0.5;

    match routing {
        // Both effects take the input and their outputs add. Two sources at once
        // carries more energy than one, which is what parallel means and why it
        // sounds fuller. With the mix control fixed it is no longer also louder
        // than the dry signal it replaces.
        RoutingMode::Parallel => {
            let (rev_l, rev_r) = if rev_enabled {
                reverb(mono(dry_l, dry_r))
            } else {
                (0.0, 0.0)
            };
            let (dly_l, dly_r) = if dly_enabled {
                delay(dry_l, dry_r)
            } else {
                (0.0, 0.0)
            };
            (
                rev_l * rev_wet + dly_l * dly_wet,
                rev_r * rev_wet + dly_r * dly_wet,
            )
        }

        // Reverb into delay. Both are heard: the reverb at its own level, plus
        // echoes of it at the delay's. With the reverb off the dry feeds the
        // delay untouched, and only the echoes are wet.
        RoutingMode::ReverbToDelay => {
            let (feed_l, feed_r, rev_is_wet) = if rev_enabled {
                let (rev_l, rev_r) = reverb(mono(dry_l, dry_r));
                (rev_l * rev_wet, rev_r * rev_wet, true)
            } else {
                (dry_l, dry_r, false)
            };
            // What the first stage contributes to the wet. Zero when it is off:
            // its signal is then only a feed, not an effect anybody asked for.
            let (base_l, base_r) = if rev_is_wet {
                (feed_l, feed_r)
            } else {
                (0.0, 0.0)
            };
            if !dly_enabled {
                return (base_l, base_r);
            }
            let (tap_l, tap_r) = delay(feed_l, feed_r);
            (base_l + tap_l * dly_wet, base_r + tap_r * dly_wet)
        }

        // Delay into reverb, the same shape in the other direction.
        RoutingMode::DelayToReverb => {
            let (feed_l, feed_r, dly_is_wet) = if dly_enabled {
                let (tap_l, tap_r) = delay(dry_l, dry_r);
                (tap_l * dly_wet, tap_r * dly_wet, true)
            } else {
                (dry_l, dry_r, false)
            };
            let (base_l, base_r) = if dly_is_wet {
                (feed_l, feed_r)
            } else {
                (0.0, 0.0)
            };
            if !rev_enabled {
                return (base_l, base_r);
            }
            let (rev_l, rev_r) = reverb(mono(feed_l, feed_r));
            (base_l + rev_l * rev_wet, base_r + rev_r * rev_wet)
        }
    }
}

/// Blend the dry signal against the processed one.
///
/// `mix` runs 0.0 (dry only) to 1.0 (wet only). This is a crossfade, not a sum:
/// an earlier version added the wet on top of a dry that stayed at full level,
/// so turning Mix up always made the plugin louder, and Parallel routing (which
/// sums reverb and delay into the wet) was louder still. Raising Mix now trades
/// dry for wet instead of adding to it.
#[inline]
pub fn mix_dry_wet(dry: f32, wet: f32, mix: f32) -> f32 {
    let m = mix.clamp(0.0, 1.0);
    dry * (1.0 - m) + wet * m
}

/// Check a saved state before any of it reaches a parameter.
///
/// State comes from a project file or a preset file. It is input like any
/// other: it can be truncated, hand-edited, written by an older build, or
/// simply corrupt. nih-plug hands whatever it parsed straight to
/// `set_plain_value`, which stores the number as-is — it does not clamp, and it
/// does not check for NaN. A delay time of 1e30 ms or a NaN mix then reaches
/// the DSP on the next buffer and takes the audio, and in some hosts the whole
/// process, with it.
///
/// So every entry is checked here first, against the parameter it claims to be:
///
/// * an unknown ID is dropped, because nothing can be done with it,
/// * a value of the wrong kind (a string for a float, say) is dropped,
/// * a non-finite float is dropped,
/// * a float outside the parameter's range is clamped into it,
/// * an enum variant index outside the enum is clamped to a real variant.
///
/// A dropped entry leaves that one parameter at its default, which is what
/// nih-plug already does for a parameter that is missing from the state
/// entirely. Everything a previous version saved is in range by construction
/// and passes through untouched, so old projects and presets load exactly as
/// they did.
pub fn sanitize_state(state: &mut PluginState) {
    let params = WettBoiParams::default();
    let known: std::collections::HashMap<String, ParamPtr> = params
        .param_map()
        .into_iter()
        .map(|(id, ptr, _group)| (id, ptr))
        .collect();

    state.params.retain(|id, value| {
        let ptr = match known.get(id) {
            Some(ptr) => *ptr,
            // A parameter this build does not have. nih-plug would log it and
            // skip it; drop it here so the log stays quiet and the intent is
            // explicit.
            None => return false,
        };

        // The two ends of the parameter's own range. A value between them is
        // left exactly as it was saved, bit for bit: re-deriving it through the
        // normalized form would move it by an ulp or two and change both the
        // sound and the bytes of the next save.
        // SAFETY: `ptr` points into `params`, a live local that outlives this
        // call, so dereferencing it here is sound. The same holds for the other
        // `ptr` calls below.
        let (lo, hi) = unsafe { (ptr.preview_plain(0.0), ptr.preview_plain(1.0)) };
        let (lo, hi) = (lo.min(hi), lo.max(hi));

        match (ptr, value) {
            (ParamPtr::FloatParam(_), ParamValue::F32(v)) => {
                if !v.is_finite() {
                    return false;
                }
                if *v < lo || *v > hi {
                    *v = v.clamp(lo, hi);
                }
                true
            }
            (ParamPtr::IntParam(_), ParamValue::I32(v))
            | (ParamPtr::EnumParam(_), ParamValue::I32(v)) => {
                let (lo, hi) = (lo.round() as i32, hi.round() as i32);
                if *v < lo || *v > hi {
                    *v = (*v).clamp(lo, hi);
                }
                true
            }
            (ParamPtr::BoolParam(_), ParamValue::Bool(_)) => true,
            // Enums may also be stored under a stable string ID. We do not set
            // those IDs, so nih-plug writes indices, but a hand-edited or
            // future state could still carry one; nih-plug checks it against
            // the enum itself and ignores an unknown one, so let it through.
            (ParamPtr::EnumParam(_), ParamValue::String(_)) => true,
            // Any other pairing is a value that does not belong to this
            // parameter at all.
            _ => false,
        }
    });
}

impl Default for HardwaveWettBoi {
    fn default() -> Self {
        // Install the panic hook before anything else can fault. Idempotent
        // via std::sync::Once.
        install_crash_handler();
        crash_reporter::install("wettboi");

        let sr = 44100.0;
        let (pkt_tx, pkt_rx) = crossbeam_channel::bounded(4);
        Self {
            params: Arc::new(WettBoiParams::default()),
            reverb: Reverb::new(sr),
            delay: StereoDelay::new(sr),
            sidechain: SidechainDetector::new(sr),
            lfo: Lfo::new(sr),
            editor_packet_tx: pkt_tx,
            editor_packet_rx: Arc::new(Mutex::new(pkt_rx)),
            update_counter: 0,
            sample_rate: sr,
            bpm: 150.0,
            duck_depth: 0.0,
            lfo_value: 0.0,
            nan_detected: false,
            input_peak_l: 0.0,
            input_peak_r: 0.0,
            output_peak_l: 0.0,
            output_peak_r: 0.0,
        }
    }
}

impl Plugin for HardwaveWettBoi {
    const NAME: &'static str = "Hardwave WettBoi";
    const VENDOR: &'static str = "Hardwave Studios";
    const URL: &'static str = "https://hardwavestudios.com";
    const EMAIL: &'static str = "hello@hardwavestudios.com";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[AudioIOLayout {
        main_input_channels: NonZeroU32::new(2),
        main_output_channels: NonZeroU32::new(2),
        // Stereo sidechain aux input — wires the SC Source = "Sidechain"
        // routing in the editor. Without this declaration the host has
        // nowhere to route a sidechain signal and the feature is dead.
        aux_input_ports: &[new_nonzero_u32(2)],
        aux_output_ports: &[],
        names: PortNames::const_default(),
    }];

    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    /// Called by nih-plug on every state load, from the host and from a preset
    /// alike, before a single value is applied. See [`sanitize_state`].
    fn filter_state(state: &mut PluginState) {
        sanitize_state(state);
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        eprintln!("[HardwaveWettBoi] editor() called — creating WettBoiEditor");
        let token = auth::load_token();
        eprintln!(
            "[HardwaveWettBoi] auth token: {}",
            if token.is_some() { "present" } else { "none" }
        );
        Some(Box::new(editor::WettBoiEditor::new(
            Arc::clone(&self.params),
            Arc::clone(&self.editor_packet_rx),
            token,
        )))
    }

    fn initialize(
        &mut self,
        _audio_io_layout: &AudioIOLayout,
        buffer_config: &BufferConfig,
        _context: &mut impl InitContext<Self>,
    ) -> bool {
        let sr = buffer_config.sample_rate;
        eprintln!(
            "[HardwaveWettBoi] initialize — sample_rate={}, buffer_size={}, version={}",
            sr,
            buffer_config.max_buffer_size,
            env!("CARGO_PKG_VERSION")
        );
        self.sample_rate = sr;
        self.reverb.set_sample_rate(sr);
        self.delay.set_sample_rate(sr);
        self.sidechain.set_sample_rate(sr);
        self.lfo.set_sample_rate(sr);
        // Defense-in-depth: clear any feedback-buffer state left over from a
        // previous instantiation (some hosts re-use plugin instances when
        // the user removes and re-adds them on the same track). Without
        // this, the first buffer can leak old samples through the reverb.
        self.reverb.reset();
        self.delay.reset();
        self.sidechain.reset();
        self.lfo.reset();
        self.duck_depth = 0.0;
        self.lfo_value = 0.0;
        self.nan_detected = false;
        true
    }

    fn reset(&mut self) {
        self.reverb.reset();
        self.delay.reset();
        self.sidechain.reset();
        self.lfo.reset();
        self.duck_depth = 0.0;
        self.lfo_value = 0.0;
        self.input_peak_l = 0.0;
        self.input_peak_r = 0.0;
        self.output_peak_l = 0.0;
        self.output_peak_r = 0.0;
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        aux: &mut AuxiliaryBuffers,
        context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        let transport = context.transport();
        if let Some(tempo) = transport.tempo {
            self.bpm = tempo as f32;
        }

        let p = &self.params;

        // Read all params once per buffer
        let rev_enabled = p.rev_enabled.value();
        let rev_type = p.rev_type.value();
        let rev_size = p.rev_size.value();
        let rev_decay = p.rev_decay.value();
        let rev_damp = p.rev_damp.value();
        let rev_predelay = p.rev_predelay.value();
        let rev_width = p.rev_width.value();
        let rev_wet_pct = p.rev_wet.value();
        let rev_freeze = p.rev_freeze.value();
        let rev_eq_hp = p.rev_eq_hp.value();
        let rev_eq_lp = p.rev_eq_lp.value();

        let sc_threshold = p.sc_threshold.value();
        let sc_attack = p.sc_attack.value();
        let sc_hold = p.sc_hold.value();
        let sc_release = p.sc_release.value();
        let sc_source = p.sc_source.value();

        let lfo_enabled = p.lfo_enabled.value();
        let lfo_rate = p.lfo_rate.value();
        let lfo_depth_pct = p.lfo_depth.value();
        let lfo_phase = p.lfo_phase.value();
        let lfo_shape = p.lfo_shape.value();
        let lfo_target = p.lfo_target.value();

        let dly_enabled = p.dly_enabled.value();
        let dly_sync = p.dly_sync.value();
        let dly_time_l = p.dly_time_l.value();
        let dly_time_r = p.dly_time_r.value();
        let dly_note_l = p.dly_note_l.value();
        let dly_note_r = p.dly_note_r.value();
        let dly_feedback = p.dly_feedback.value();
        let dly_hp = p.dly_hp.value();
        let dly_lp = p.dly_lp.value();
        let dly_ping_pong = p.dly_ping_pong.value();
        let dly_wet_pct = p.dly_wet.value();
        let dly_mod_rate = p.dly_mod_rate.value();
        let dly_mod_depth = p.dly_mod_depth.value();
        let dly_saturation = p.dly_saturation.value();

        let mix_pct = p.mix.value();
        let bypass = p.bypass.value();
        let routing = p.routing.value();

        // Snapshot for editor (before processing — param values only)
        let pkt_snapshot = editor::snapshot_params(p, self.bpm, self.duck_depth, self.lfo_value);

        // Configure DSP modules
        let dsp_rev_type = match rev_type {
            params::ReverbType::Room => DspReverbType::Room,
            params::ReverbType::Hall => DspReverbType::Hall,
            params::ReverbType::Plate => DspReverbType::Plate,
            params::ReverbType::Spring => DspReverbType::Spring,
        };
        self.reverb.set_type(dsp_rev_type);
        self.reverb.set_freeze(rev_freeze);
        self.reverb
            .set_params(rev_size, rev_decay, rev_damp, rev_predelay);
        self.reverb.set_eq(rev_eq_hp, rev_eq_lp);

        self.sidechain
            .set_params(sc_threshold, sc_attack, sc_hold, sc_release);

        if dly_sync {
            self.delay
                .set_time_sync(self.bpm, dly_note_l.beats(), dly_note_r.beats());
        } else {
            self.delay.set_time_ms(dly_time_l, dly_time_r);
        }
        self.delay.set_feedback(dly_feedback);
        self.delay.set_filter(dly_hp, dly_lp);
        self.delay.set_ping_pong(dly_ping_pong);
        self.delay.set_modulation(dly_mod_rate, dly_mod_depth);
        self.delay.set_saturation(dly_saturation);

        self.lfo.set_rate(lfo_rate);
        self.lfo.set_phase_offset(lfo_phase);
        self.lfo.set_shape(match lfo_shape {
            params::LfoShape::Sine => LfoShape::Sine,
            params::LfoShape::Tri => LfoShape::Tri,
            params::LfoShape::Saw => LfoShape::Saw,
            params::LfoShape::Square => LfoShape::Square,
            params::LfoShape::SampleAndHold => LfoShape::SampleAndHold,
        });

        let mix = mix_pct / 100.0;
        let rev_wet = rev_wet_pct / 100.0;
        let dly_wet = dly_wet_pct / 100.0;
        let lfo_depth = lfo_depth_pct / 100.0;

        let has_sidechain = !aux.inputs.is_empty() && aux.inputs[0].channels() >= 2;

        // Reset peak meters (decay)
        let decay = 0.9995_f32;
        self.input_peak_l *= decay;
        self.input_peak_r *= decay;
        self.output_peak_l *= decay;
        self.output_peak_r *= decay;

        for (sample_idx, mut frame) in buffer.iter_samples().enumerate() {
            if frame.len() < 2 {
                continue;
            }
            let dry_l = *frame.get_mut(0).unwrap();
            let dry_r = *frame.get_mut(1).unwrap();

            // Input metering
            self.input_peak_l = self.input_peak_l.max(dry_l.abs());
            self.input_peak_r = self.input_peak_r.max(dry_r.abs());

            if bypass {
                continue;
            }

            // Sidechain detection
            let sc_input = match sc_source {
                params::ScSource::Sidechain if has_sidechain => {
                    let sc_buf = aux.inputs[0].as_slice_immutable();
                    let sc_l = *sc_buf
                        .first()
                        .and_then(|ch| ch.get(sample_idx))
                        .unwrap_or(&0.0);
                    let sc_r = *sc_buf
                        .get(1)
                        .and_then(|ch| ch.get(sample_idx))
                        .unwrap_or(&0.0);
                    (sc_l + sc_r) * 0.5
                }
                _ => (dry_l + dry_r) * 0.5,
            };

            let duck = self.sidechain.process(sc_input);
            self.duck_depth = duck;

            // LFO
            let lfo_raw = if lfo_enabled { self.lfo.process() } else { 0.0 };
            self.lfo_value = lfo_raw;
            let lfo_val = lfo_raw * lfo_depth;

            // Modulated wet levels
            let mod_rev_wet = match lfo_target {
                LfoTarget::RevWet => (rev_wet + lfo_val * 0.5).clamp(0.0, 1.0),
                _ => rev_wet,
            };
            let mod_dly_wet = match lfo_target {
                LfoTarget::DlyWet => (dly_wet + lfo_val * 0.5).clamp(0.0, 1.0),
                _ => dly_wet,
            };
            if matches!(lfo_target, LfoTarget::DlyFeedback) {
                self.delay
                    .set_feedback((dly_feedback + lfo_val * 30.0).clamp(0.0, 95.0));
            }
            if matches!(lfo_target, LfoTarget::Filter) {
                let mod_lp = (dly_lp + lfo_val * 4000.0).clamp(1000.0, 20000.0);
                self.delay.set_filter(dly_hp, mod_lp);
            }

            // The wet signal, however the two effects are wired together.
            // `route_wet` is a free function so the levels can be measured.
            let reverb = &mut self.reverb;
            let delay = &mut self.delay;
            let (wet_l, wet_r) = route_wet(
                routing,
                (dry_l, dry_r),
                rev_enabled,
                dly_enabled,
                mod_rev_wet,
                mod_dly_wet,
                |mono| reverb.process(mono, rev_width),
                |l, r| delay.process(l, r),
            );

            // Apply sidechain ducking to wet signal
            let ducked_l = wet_l * (1.0 - duck);
            let ducked_r = wet_r * (1.0 - duck);

            // Mix dry against wet.
            //
            // This used to read `dry * (1 - mix) + (dry + wet) * mix`, which
            // multiplies out to `dry + wet * mix`: the dry signal stayed at full
            // level and the wet was piled on top, so raising Mix always raised
            // the output. Parallel routing sums reverb AND delay into that wet,
            // which is why parallel sounded louder than the serial modes. A mix
            // control has to trade one for the other.
            let out_l = mix_dry_wet(dry_l, ducked_l, mix);
            let out_r = mix_dry_wet(dry_r, ducked_r, mix);

            // Safety net: if a DSP regression ever produces NaN/Inf, fall back
            // to the user's dry signal instead of writing junk that the host
            // will mute. We also flag the buffer so DSP gets reset below.
            // f32::clamp passes NaN through unchanged, so the explicit
            // is_finite check is required.
            let (final_l, final_r) = if out_l.is_finite() && out_r.is_finite() {
                (out_l.clamp(-10.0, 10.0), out_r.clamp(-10.0, 10.0))
            } else {
                self.nan_detected = true;
                (dry_l, dry_r)
            };

            *frame.get_mut(0).unwrap() = final_l;
            *frame.get_mut(1).unwrap() = final_r;

            // Output metering
            self.output_peak_l = self.output_peak_l.max(final_l.abs());
            self.output_peak_r = self.output_peak_r.max(final_r.abs());
        }

        // If any sample in this buffer needed the NaN fallback, reset every
        // stateful DSP module so the next buffer starts from a clean state
        // instead of letting the bad value persist in feedback loops.
        if self.nan_detected {
            self.reverb.reset();
            self.delay.reset();
            self.sidechain.reset();
            self.lfo.reset();
            self.duck_depth = 0.0;
            self.lfo_value = 0.0;
            self.nan_detected = false;
        }

        // Send packet to editor at ~15 fps (every 4th buffer)
        self.update_counter += 1;
        if self.update_counter >= 4 {
            self.update_counter = 0;
            let mut packet = pkt_snapshot;
            packet.sc_duck_depth = self.duck_depth;
            packet.sc_key_level = self.sidechain.key_level();
            packet.sc_threshold_lin = self.sidechain.threshold_linear();
            packet.lfo_value = self.lfo_value;
            packet.input_peak_l = self.input_peak_l;
            packet.input_peak_r = self.input_peak_r;
            packet.output_peak_l = self.output_peak_l;
            packet.output_peak_r = self.output_peak_r;
            let _ = self.editor_packet_tx.try_send(packet);
        }

        ProcessStatus::Normal
    }
}

impl ClapPlugin for HardwaveWettBoi {
    const CLAP_ID: &'static str = "com.hardwavestudios.wettboi";
    const CLAP_DESCRIPTION: Option<&'static str> =
        Some("Sidechain reverb & delay with LFO modulation");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = Some("https://hardwavestudios.com/support");
    const CLAP_FEATURES: &'static [ClapFeature] = &[
        ClapFeature::AudioEffect,
        ClapFeature::Reverb,
        ClapFeature::Delay,
        ClapFeature::Stereo,
    ];
}

impl Vst3Plugin for HardwaveWettBoi {
    const VST3_CLASS_ID: [u8; 16] = *b"HWWettBoi_v001\0\0";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[
        Vst3SubCategory::Fx,
        Vst3SubCategory::Reverb,
        Vst3SubCategory::Delay,
        Vst3SubCategory::Stereo,
    ];
}

// The CLAP side is exported by `clap_export` instead of `nih_export_clap!`.
nih_export_vst3!(HardwaveWettBoi);

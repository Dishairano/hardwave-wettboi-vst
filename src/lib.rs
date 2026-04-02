//! Hardwave WettBoi — sidechain reverb & delay VST3/CLAP plugin.
//!
//! Signal chain (parallel mode):
//!   Input → Reverb (wet) + Delay (wet) → Sidechain Duck → LFO Modulation → Mix → Output
//! Serial modes:
//!   Rev→Dly: Input → Reverb → Delay → Sidechain → Mix → Output
//!   Dly→Rev: Input → Delay → Reverb → Sidechain → Mix → Output

use crossbeam_channel::{Sender, Receiver};
use nih_plug::prelude::*;
use parking_lot::Mutex;
use std::sync::Arc;

mod auth;
mod dsp;
mod editor;
mod params;
mod protocol;

use dsp::{Reverb, StereoDelay, SidechainDetector, Lfo};
use dsp::lfo::Shape as LfoShape;
use dsp::reverb::ReverbType as DspReverbType;
use params::{WettBoiParams, LfoTarget, RoutingMode};
use protocol::WbPacket;

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

    // Metering
    input_peak_l: f32,
    input_peak_r: f32,
    output_peak_l: f32,
    output_peak_r: f32,
}

impl Default for HardwaveWettBoi {
    fn default() -> Self {
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
        ..AudioIOLayout::const_default()
    }];

    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        eprintln!("[HardwaveWettBoi] editor() called — creating WettBoiEditor");
        let token = auth::load_token();
        eprintln!("[HardwaveWettBoi] auth token: {}", if token.is_some() { "present" } else { "none" });
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
        eprintln!("[HardwaveWettBoi] initialize — sample_rate={}, buffer_size={}, version={}",
            sr, buffer_config.max_buffer_size, env!("CARGO_PKG_VERSION"));
        self.sample_rate = sr;
        self.reverb.set_sample_rate(sr);
        self.delay.set_sample_rate(sr);
        self.sidechain.set_sample_rate(sr);
        self.lfo.set_sample_rate(sr);
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
        self.reverb.set_params(rev_size, rev_decay, rev_damp, rev_predelay);
        self.reverb.set_eq(rev_eq_hp, rev_eq_lp);

        self.sidechain.set_params(sc_threshold, sc_attack, sc_hold, sc_release);

        if dly_sync {
            self.delay.set_time_sync(self.bpm, dly_note_l.beats(), dly_note_r.beats());
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
            if frame.len() < 2 { continue; }
            let dry_l = *frame.get_mut(0).unwrap();
            let dry_r = *frame.get_mut(1).unwrap();

            // Input metering
            self.input_peak_l = self.input_peak_l.max(dry_l.abs());
            self.input_peak_r = self.input_peak_r.max(dry_r.abs());

            if bypass { continue; }

            // Sidechain detection
            let sc_input = match sc_source {
                params::ScSource::Sidechain if has_sidechain => {
                    let sc_buf = aux.inputs[0].as_slice_immutable();
                    let sc_l = *sc_buf.get(0).and_then(|ch| ch.get(sample_idx)).unwrap_or(&0.0);
                    let sc_r = *sc_buf.get(1).and_then(|ch| ch.get(sample_idx)).unwrap_or(&0.0);
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
                self.delay.set_feedback((dly_feedback + lfo_val * 30.0).clamp(0.0, 95.0));
            }
            if matches!(lfo_target, LfoTarget::Filter) {
                let mod_lp = (dly_lp + lfo_val * 4000.0).clamp(1000.0, 20000.0);
                self.delay.set_filter(dly_hp, mod_lp);
            }

            let mono_in = (dry_l + dry_r) * 0.5;

            // Process effects based on routing mode
            let (wet_l, wet_r) = match routing {
                RoutingMode::Parallel => {
                    // Reverb and delay process input independently
                    let (rev_l, rev_r) = if rev_enabled { self.reverb.process(mono_in, rev_width) } else { (0.0, 0.0) };
                    let (dly_l, dly_r) = if dly_enabled { self.delay.process(dry_l, dry_r) } else { (0.0, 0.0) };
                    (
                        rev_l * mod_rev_wet + dly_l * mod_dly_wet,
                        rev_r * mod_rev_wet + dly_r * mod_dly_wet,
                    )
                }
                RoutingMode::ReverbToDelay => {
                    // Reverb output feeds into delay
                    let (rev_l, rev_r) = if rev_enabled { self.reverb.process(mono_in, rev_width) } else { (dry_l, dry_r) };
                    let rev_out_l = rev_l * mod_rev_wet;
                    let rev_out_r = rev_r * mod_rev_wet;
                    let (dly_l, dly_r) = if dly_enabled { self.delay.process(rev_out_l, rev_out_r) } else { (rev_out_l, rev_out_r) };
                    (dly_l * mod_dly_wet, dly_r * mod_dly_wet)
                }
                RoutingMode::DelayToReverb => {
                    // Delay output feeds into reverb
                    let (dly_l, dly_r) = if dly_enabled { self.delay.process(dry_l, dry_r) } else { (dry_l, dry_r) };
                    let dly_out = (dly_l * mod_dly_wet + dly_r * mod_dly_wet) * 0.5;
                    let (rev_l, rev_r) = if rev_enabled { self.reverb.process(dly_out, rev_width) } else { (dly_l * mod_dly_wet, dly_r * mod_dly_wet) };
                    (rev_l * mod_rev_wet, rev_r * mod_rev_wet)
                }
            };

            // Apply sidechain ducking to wet signal
            let ducked_l = wet_l * (1.0 - duck);
            let ducked_r = wet_r * (1.0 - duck);

            // Mix dry + wet
            let out_l = dry_l * (1.0 - mix) + (dry_l + ducked_l) * mix;
            let out_r = dry_r * (1.0 - mix) + (dry_r + ducked_r) * mix;

            let final_l = out_l.clamp(-10.0, 10.0);
            let final_r = out_r.clamp(-10.0, 10.0);

            *frame.get_mut(0).unwrap() = final_l;
            *frame.get_mut(1).unwrap() = final_r;

            // Output metering
            self.output_peak_l = self.output_peak_l.max(final_l.abs());
            self.output_peak_r = self.output_peak_r.max(final_r.abs());
        }

        // Send packet to editor at ~15 fps (every 4th buffer)
        self.update_counter += 1;
        if self.update_counter >= 4 {
            self.update_counter = 0;
            let mut packet = pkt_snapshot;
            packet.sc_duck_depth = self.duck_depth;
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

nih_export_clap!(HardwaveWettBoi);
nih_export_vst3!(HardwaveWettBoi);

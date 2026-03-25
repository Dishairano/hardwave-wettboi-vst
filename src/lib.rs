//! Hardwave WettBoi — sidechain reverb & delay VST3/CLAP plugin.
//!
//! Signal chain:
//!   Input → Reverb (wet) + Delay (wet) → Sidechain Duck → LFO Modulation → Mix → Output
//!
//! The reverb and delay produce wet signals that are mixed together, then
//! ducked by the sidechain envelope follower (triggered by the input or
//! an external sidechain). An LFO can modulate the reverb wet, delay wet,
//! delay feedback, or a filter parameter.

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
use params::{WettBoiParams, LfoTarget};
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
}

impl Default for HardwaveWettBoi {
    fn default() -> Self {
        eprintln!("[HardwaveWettBoi] default() — constructing plugin");
        let sr = 44100.0;
        let (pkt_tx, pkt_rx) = crossbeam_channel::bounded(4);
        let inst = Self {
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
        };
        eprintln!("[HardwaveWettBoi] default() — construction complete");
        inst
    }
}

impl Plugin for HardwaveWettBoi {
    const NAME: &'static str = "Hardwave WettBoi";
    const VENDOR: &'static str = "Hardwave Studios";
    const URL: &'static str = "https://hardwavestudios.com";
    const EMAIL: &'static str = "hello@hardwavestudios.com";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    // Single stereo layout — FL Studio rejects plugins with multiple layouts.
    // Sidechain is handled at runtime by checking if aux buffers are available.
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
        // Editor disabled for debugging — return None to test if plugin loads without WebView
        None
    }

    fn initialize(
        &mut self,
        _audio_io_layout: &AudioIOLayout,
        buffer_config: &BufferConfig,
        _context: &mut impl InitContext<Self>,
    ) -> bool {
        eprintln!("[HardwaveWettBoi] initialize() — sr={}", buffer_config.sample_rate);
        let sr = buffer_config.sample_rate;
        self.sample_rate = sr;
        self.reverb.set_sample_rate(sr);
        self.delay.set_sample_rate(sr);
        self.sidechain.set_sample_rate(sr);
        self.lfo.set_sample_rate(sr);
        eprintln!("[HardwaveWettBoi] initialize() — complete");
        true
    }

    fn reset(&mut self) {
        self.reverb.reset();
        self.delay.reset();
        self.sidechain.reset();
        self.lfo.reset();
        self.duck_depth = 0.0;
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        aux: &mut AuxiliaryBuffers,
        context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        // Read transport BPM
        let transport = context.transport();
        if let Some(tempo) = transport.tempo {
            self.bpm = tempo as f32;
        }

        // Read all params
        let p = &self.params;
        let rev_enabled = p.rev_enabled.value();
        let rev_size = p.rev_size.value();
        let rev_decay = p.rev_decay.value();
        let rev_damp = p.rev_damp.value();
        let rev_predelay = p.rev_predelay.value();
        let rev_width = p.rev_width.value();
        let rev_wet_pct = p.rev_wet.value();

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

        let mix_pct = p.mix.value();
        let bypass = p.bypass.value();

        // Snapshot for editor
        let pkt_snapshot = editor::snapshot_params(p, self.bpm, self.duck_depth);

        // Update DSP parameters
        self.reverb.set_params(rev_size, rev_decay, rev_damp, rev_predelay);
        self.sidechain.set_params(sc_threshold, sc_attack, sc_hold, sc_release);

        // Delay timing
        if dly_sync {
            self.delay.set_time_sync(self.bpm, dly_note_l.beats(), dly_note_r.beats());
        } else {
            self.delay.set_time_ms(dly_time_l, dly_time_r);
        }
        self.delay.set_feedback(dly_feedback);
        self.delay.set_filter(dly_hp, dly_lp);
        self.delay.set_ping_pong(dly_ping_pong);

        // LFO
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

        // Check if sidechain aux is available
        let has_sidechain = !aux.inputs.is_empty()
            && aux.inputs[0].channels() >= 2;

        for (sample_idx, mut frame) in buffer.iter_samples().enumerate() {
            if frame.len() < 2 {
                continue;
            }

            let dry_l = *frame.get_mut(0).unwrap();
            let dry_r = *frame.get_mut(1).unwrap();

            if bypass {
                continue;
            }

            // Determine sidechain input
            let sc_input = match sc_source {
                params::ScSource::Sidechain if has_sidechain => {
                    let sc_buf = aux.inputs[0].as_slice_immutable();
                    let sc_l = *sc_buf.get(0).and_then(|ch| ch.get(sample_idx)).unwrap_or(&0.0);
                    let sc_r = *sc_buf.get(1).and_then(|ch| ch.get(sample_idx)).unwrap_or(&0.0);
                    (sc_l + sc_r) * 0.5
                }
                _ => {
                    // Internal: use the dry input as sidechain trigger
                    (dry_l + dry_r) * 0.5
                }
            };

            // Sidechain envelope
            let duck = self.sidechain.process(sc_input);
            self.duck_depth = duck;

            // LFO modulation
            let lfo_val = if lfo_enabled {
                self.lfo.process() * lfo_depth
            } else {
                0.0
            };

            // Apply LFO to target
            let mod_rev_wet = match lfo_target {
                LfoTarget::RevWet => (rev_wet + lfo_val * 0.5).clamp(0.0, 1.0),
                _ => rev_wet,
            };
            let mod_dly_wet = match lfo_target {
                LfoTarget::DlyWet => (dly_wet + lfo_val * 0.5).clamp(0.0, 1.0),
                _ => dly_wet,
            };
            let _mod_dly_fb = match lfo_target {
                LfoTarget::DlyFeedback => {
                    self.delay.set_feedback((dly_feedback + lfo_val * 30.0).clamp(0.0, 95.0));
                    dly_feedback
                }
                _ => dly_feedback,
            };
            // Filter modulation: shift delay LP
            if matches!(lfo_target, LfoTarget::Filter) {
                let mod_lp = (dly_lp + lfo_val * 4000.0).clamp(1000.0, 20000.0);
                self.delay.set_filter(dly_hp, mod_lp);
            }

            let mono_in = (dry_l + dry_r) * 0.5;

            // Reverb (wet only)
            let (rev_l, rev_r) = if rev_enabled {
                self.reverb.process(mono_in, rev_width)
            } else {
                (0.0, 0.0)
            };

            // Delay (wet only)
            let (dly_l, dly_r) = if dly_enabled {
                self.delay.process(dry_l, dry_r)
            } else {
                (0.0, 0.0)
            };

            // Mix wet signals
            let wet_l = rev_l * mod_rev_wet + dly_l * mod_dly_wet;
            let wet_r = rev_r * mod_rev_wet + dly_r * mod_dly_wet;

            // Apply sidechain ducking to wet signal
            let ducked_l = wet_l * (1.0 - duck);
            let ducked_r = wet_r * (1.0 - duck);

            // Final dry/wet mix
            let out_l = dry_l * (1.0 - mix) + (dry_l + ducked_l) * mix;
            let out_r = dry_r * (1.0 - mix) + (dry_r + ducked_r) * mix;

            // Clamp to prevent NaN/Inf
            *frame.get_mut(0).unwrap() = out_l.clamp(-10.0, 10.0);
            *frame.get_mut(1).unwrap() = out_r.clamp(-10.0, 10.0);
        }

        // Send state packet to editor (~60 fps)
        self.update_counter += 1;
        if self.update_counter >= 4 {
            self.update_counter = 0;
            let mut packet = pkt_snapshot;
            packet.sc_duck_depth = self.duck_depth;
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

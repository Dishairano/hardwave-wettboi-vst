use nih_plug::prelude::*;
use std::sync::Arc;

mod dsp;
mod params;

use dsp::{Reverb, StereoDelay, SidechainDetector, Lfo};
use dsp::lfo::Shape as LfoShape;
use params::{WettBoiParams, LfoTarget};

struct HardwaveWettBoi {
    params: Arc<WettBoiParams>,
    reverb: Reverb,
    delay: StereoDelay,
    sidechain: SidechainDetector,
    lfo: Lfo,
    sample_rate: f32,
    bpm: f32,
    duck_depth: f32,
}

impl Default for HardwaveWettBoi {
    fn default() -> Self {
        let sr = 44100.0;
        Self {
            params: Arc::new(WettBoiParams::default()),
            reverb: Reverb::new(sr),
            delay: StereoDelay::new(sr),
            sidechain: SidechainDetector::new(sr),
            lfo: Lfo::new(sr),
            sample_rate: sr,
            bpm: 150.0,
            duck_depth: 0.0,
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

    fn initialize(
        &mut self,
        _audio_io_layout: &AudioIOLayout,
        buffer_config: &BufferConfig,
        _context: &mut impl InitContext<Self>,
    ) -> bool {
        let sr = buffer_config.sample_rate;
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
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        let transport = context.transport();
        if let Some(tempo) = transport.tempo {
            self.bpm = tempo as f32;
        }

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

        self.reverb.set_params(rev_size, rev_decay, rev_damp, rev_predelay);
        self.sidechain.set_params(sc_threshold, sc_attack, sc_hold, sc_release);

        if dly_sync {
            self.delay.set_time_sync(self.bpm, dly_note_l.beats(), dly_note_r.beats());
        } else {
            self.delay.set_time_ms(dly_time_l, dly_time_r);
        }
        self.delay.set_feedback(dly_feedback);
        self.delay.set_filter(dly_hp, dly_lp);
        self.delay.set_ping_pong(dly_ping_pong);

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

        for (_sample_idx, mut frame) in buffer.iter_samples().enumerate() {
            if frame.len() < 2 { continue; }
            let dry_l = *frame.get_mut(0).unwrap();
            let dry_r = *frame.get_mut(1).unwrap();
            if bypass { continue; }

            let sc_input = (dry_l + dry_r) * 0.5;
            let duck = self.sidechain.process(sc_input);
            self.duck_depth = duck;

            let lfo_val = if lfo_enabled { self.lfo.process() * lfo_depth } else { 0.0 };

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
            let (rev_l, rev_r) = if rev_enabled { self.reverb.process(mono_in, rev_width) } else { (0.0, 0.0) };
            let (dly_l, dly_r) = if dly_enabled { self.delay.process(dry_l, dry_r) } else { (0.0, 0.0) };

            let wet_l = rev_l * mod_rev_wet + dly_l * mod_dly_wet;
            let wet_r = rev_r * mod_rev_wet + dly_r * mod_dly_wet;
            let ducked_l = wet_l * (1.0 - duck);
            let ducked_r = wet_r * (1.0 - duck);
            let out_l = dry_l * (1.0 - mix) + (dry_l + ducked_l) * mix;
            let out_r = dry_r * (1.0 - mix) + (dry_r + ducked_r) * mix;

            *frame.get_mut(0).unwrap() = out_l.clamp(-10.0, 10.0);
            *frame.get_mut(1).unwrap() = out_r.clamp(-10.0, 10.0);
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

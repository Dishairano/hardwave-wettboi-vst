//! WettBoi — wet signal processor VST3/CLAP plugin by Hardwave Studios.
//!
//! Architecture:
//! - Audio thread: reads params, applies filter → envelope → delay/reverb chain
//! - Editor thread: wry WebView loaded from wettboi.hardwavestudios.com
//! - Plugin → WebView: param state pushed at ~30Hz via crossbeam channel
//! - WebView → Plugin: param changes via IPC → setter channel → applied in process()

use crossbeam_channel::{bounded, Receiver, Sender};
use nih_plug::prelude::*;
use nih_plug::params::Param;
use parking_lot::Mutex;
use std::sync::Arc;

mod auth;
mod dsp;
#[cfg(feature = "gui")]
mod editor;
mod params;
mod protocol;

use dsp::biquad::{BiquadFilter, FilterType};
use dsp::delay::DelayLine;
use dsp::envelope::AdsrEnvelope;
use dsp::reverb::Reverb;
use params::{FxOrder, WettBoiParams};
use protocol::{ParamChange, WettBoiPacket};

/// How often we send param state to the editor (every N process calls).
const EDITOR_UPDATE_INTERVAL: u32 = 512;

pub struct HardwaveWettBoi {
    params: Arc<WettBoiParams>,

    // DSP state (per-channel where needed)
    hi_pass: [BiquadFilter; 2],
    lo_pass: [BiquadFilter; 2],
    envelope: [AdsrEnvelope; 2],
    delay: [DelayLine; 2],
    reverb: Reverb,

    sample_rate: f32,

    // Plugin → Editor: latest param state
    editor_packet_tx: Sender<WettBoiPacket>,
    editor_packet_rx: Arc<Mutex<Receiver<WettBoiPacket>>>,

    // Editor → Plugin: param changes from WebView
    param_change_tx: Arc<Mutex<Sender<ParamChange>>>,
    param_change_rx: Receiver<ParamChange>,

    // Counter for throttled editor updates
    update_counter: u32,
}

impl Default for HardwaveWettBoi {
    fn default() -> Self {
        let sr = 44100.0;
        let (pkt_tx, pkt_rx) = bounded::<WettBoiPacket>(4);
        let (chg_tx, chg_rx) = bounded::<ParamChange>(64);

        Self {
            params: Arc::new(WettBoiParams::default()),
            hi_pass: [
                BiquadFilter::new(FilterType::HighPass, sr),
                BiquadFilter::new(FilterType::HighPass, sr),
            ],
            lo_pass: [
                BiquadFilter::new(FilterType::LowPass, sr),
                BiquadFilter::new(FilterType::LowPass, sr),
            ],
            envelope: [AdsrEnvelope::new(sr), AdsrEnvelope::new(sr)],
            delay: [DelayLine::new(sr), DelayLine::new(sr)],
            reverb: Reverb::new(sr),
            sample_rate: sr,
            editor_packet_tx: pkt_tx,
            editor_packet_rx: Arc::new(Mutex::new(pkt_rx)),
            param_change_tx: Arc::new(Mutex::new(chg_tx)),
            param_change_rx: chg_rx,
            update_counter: 0,
        }
    }
}

impl Plugin for HardwaveWettBoi {
    const NAME: &'static str = "WettBoi";
    const VENDOR: &'static str = "Hardwave Studios";
    const URL: &'static str = "https://hardwavestudios.com";
    const EMAIL: &'static str = "hello@hardwavestudios.com";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[
        // Stereo
        AudioIOLayout {
            main_input_channels: NonZeroU32::new(2),
            main_output_channels: NonZeroU32::new(2),
            ..AudioIOLayout::const_default()
        },
        // Mono
        AudioIOLayout {
            main_input_channels: NonZeroU32::new(1),
            main_output_channels: NonZeroU32::new(1),
            ..AudioIOLayout::const_default()
        },
    ];

    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    #[cfg(feature = "gui")]
    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        let token = auth::load_token();
        Some(Box::new(editor::WettBoiEditor::new(
            Arc::clone(&self.editor_packet_rx),
            Arc::clone(&self.param_change_tx),
            token,
        )))
    }

    fn initialize(
        &mut self,
        _audio_io_layout: &AudioIOLayout,
        buffer_config: &BufferConfig,
        _context: &mut impl InitContext<Self>,
    ) -> bool {
        self.sample_rate = buffer_config.sample_rate;

        for f in &mut self.hi_pass { f.set_sample_rate(self.sample_rate); }
        for f in &mut self.lo_pass { f.set_sample_rate(self.sample_rate); }
        for e in &mut self.envelope { e.set_sample_rate(self.sample_rate); }
        for d in &mut self.delay { d.set_sample_rate(self.sample_rate); }
        self.reverb.set_sample_rate(self.sample_rate);

        true
    }

    fn reset(&mut self) {
        for f in &mut self.hi_pass { f.reset(); }
        for f in &mut self.lo_pass { f.reset(); }
        for e in &mut self.envelope { e.reset(); }
        for d in &mut self.delay { d.reset(); }
        self.reverb.reset();
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        _context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        // Apply any pending param changes from the WebView
        while let Ok(change) = self.param_change_rx.try_recv() {
            self.apply_param_change(&change);
        }

        // Read current param values
        let enabled = self.params.enabled.value();
        let wet = self.params.wet.value();
        let hp_on = self.params.hi_pass_enabled.value();
        let hp_freq = self.params.hi_pass_freq.value();
        let lp_on = self.params.lo_pass_enabled.value();
        let lp_freq = self.params.lo_pass_freq.value();
        let attack = self.params.attack.value();
        let decay = self.params.decay.value();
        let sustain = self.params.sustain.value();
        let release = self.params.release.value();
        let delay_on = self.params.delay_enabled.value();
        let delay_time = self.params.delay_time.value();
        let delay_fb = self.params.delay_feedback.value();
        let delay_mix = self.params.delay_mix.value();
        let reverb_on = self.params.reverb_enabled.value();
        let reverb_size = self.params.reverb_size.value();
        let reverb_damp = self.params.reverb_damping.value();
        let reverb_mix = self.params.reverb_mix.value();
        let fx_order = self.params.fx_order.value();

        // Update DSP parameters
        for f in &mut self.hi_pass { f.set_freq(hp_freq); }
        for f in &mut self.lo_pass { f.set_freq(lp_freq); }
        for e in &mut self.envelope {
            e.set_attack(attack);
            e.set_decay(decay);
            e.set_sustain(sustain);
            e.set_release(release);
        }
        for d in &mut self.delay {
            d.set_delay_ms(delay_time);
            d.set_feedback(delay_fb);
            d.set_mix(delay_mix);
        }
        self.reverb.set_size(reverb_size);
        self.reverb.set_damping(reverb_damp);
        self.reverb.set_mix(reverb_mix);

        if !enabled {
            // Bypass — audio passes through unchanged
            return ProcessStatus::Normal;
        }

        let num_channels = buffer.channels();
        let num_samples = buffer.samples();

        // Process sample-by-sample for stereo pair
        // We need to handle stereo reverb which needs both channels at once
        if num_channels >= 2 {
            let (left, right_and_rest) = buffer.as_slice().split_first_mut().unwrap();
            let right = &mut right_and_rest[0];

            for i in 0..num_samples {
                let mut l = left[i];
                let mut r = right[i];
                let dry_l = l;
                let dry_r = r;

                // Filters
                if hp_on {
                    l = self.hi_pass[0].process(l);
                    r = self.hi_pass[1].process(r);
                }
                if lp_on {
                    l = self.lo_pass[0].process(l);
                    r = self.lo_pass[1].process(r);
                }

                // ADSR envelope
                let env_l = self.envelope[0].process(l.abs());
                let env_r = self.envelope[1].process(r.abs());
                l *= env_l;
                r *= env_r;

                // FX chain (delay + reverb in configurable order)
                match fx_order {
                    FxOrder::DelayReverb => {
                        if delay_on {
                            l = self.delay[0].process(l);
                            r = self.delay[1].process(r);
                        }
                        if reverb_on {
                            let (rl, rr) = self.reverb.process(l, r);
                            l = rl;
                            r = rr;
                        }
                    }
                    FxOrder::ReverbDelay => {
                        if reverb_on {
                            let (rl, rr) = self.reverb.process(l, r);
                            l = rl;
                            r = rr;
                        }
                        if delay_on {
                            l = self.delay[0].process(l);
                            r = self.delay[1].process(r);
                        }
                    }
                }

                // Wet/dry mix
                left[i] = dry_l * (1.0 - wet) + l * wet;
                right[i] = dry_r * (1.0 - wet) + r * wet;
            }
        } else {
            // Mono
            let channels = buffer.as_slice();
            let mono = &mut channels[0];

            for i in 0..num_samples {
                let mut s = mono[i];
                let dry = s;

                if hp_on {
                    s = self.hi_pass[0].process(s);
                }
                if lp_on {
                    s = self.lo_pass[0].process(s);
                }

                let env = self.envelope[0].process(s.abs());
                s *= env;

                match fx_order {
                    FxOrder::DelayReverb => {
                        if delay_on { s = self.delay[0].process(s); }
                        if reverb_on {
                            let (rl, _) = self.reverb.process(s, s);
                            s = rl;
                        }
                    }
                    FxOrder::ReverbDelay => {
                        if reverb_on {
                            let (rl, _) = self.reverb.process(s, s);
                            s = rl;
                        }
                        if delay_on { s = self.delay[0].process(s); }
                    }
                }

                mono[i] = dry * (1.0 - wet) + s * wet;
            }
        }

        // Throttled editor update
        self.update_counter += num_samples as u32;
        if self.update_counter >= EDITOR_UPDATE_INTERVAL {
            self.update_counter = 0;
            let packet = WettBoiPacket {
                enabled,
                wet,
                hi_pass_enabled: hp_on,
                hi_pass_freq: hp_freq,
                lo_pass_enabled: lp_on,
                lo_pass_freq: lp_freq,
                attack,
                decay,
                sustain,
                release,
                delay_enabled: delay_on,
                delay_time,
                delay_feedback: delay_fb,
                delay_mix,
                reverb_enabled: reverb_on,
                reverb_size: reverb_size,
                reverb_damping: reverb_damp,
                reverb_mix: reverb_mix,
                fx_order: match fx_order {
                    FxOrder::DelayReverb => "delay-reverb".to_string(),
                    FxOrder::ReverbDelay => "reverb-delay".to_string(),
                },
            };
            let _ = self.editor_packet_tx.try_send(packet);
        }

        ProcessStatus::Normal
    }
}

impl HardwaveWettBoi {
    /// Apply a parameter change received from the WebView UI.
    /// Uses `set_plain_value` from the `Param` trait to update nih-plug params.
    fn apply_param_change(&self, change: &ParamChange) {
        let v = change.value as f32;
        match change.key.as_str() {
            "enabled" => self.params.enabled.set_plain_value(if change.value > 0.5 { 1.0 } else { 0.0 }),
            "wet" => self.params.wet.set_plain_value(v),
            "hiPassEnabled" => self.params.hi_pass_enabled.set_plain_value(if change.value > 0.5 { 1.0 } else { 0.0 }),
            "hiPassFreq" => self.params.hi_pass_freq.set_plain_value(v),
            "loPassEnabled" => self.params.lo_pass_enabled.set_plain_value(if change.value > 0.5 { 1.0 } else { 0.0 }),
            "loPassFreq" => self.params.lo_pass_freq.set_plain_value(v),
            "attack" => self.params.attack.set_plain_value(v),
            "decay" => self.params.decay.set_plain_value(v),
            "sustain" => self.params.sustain.set_plain_value(v),
            "release" => self.params.release.set_plain_value(v),
            "delayEnabled" => self.params.delay_enabled.set_plain_value(if change.value > 0.5 { 1.0 } else { 0.0 }),
            "delayTime" => self.params.delay_time.set_plain_value(v),
            "delayFeedback" => self.params.delay_feedback.set_plain_value(v),
            "delayMix" => self.params.delay_mix.set_plain_value(v),
            "reverbEnabled" => self.params.reverb_enabled.set_plain_value(if change.value > 0.5 { 1.0 } else { 0.0 }),
            "reverbSize" => self.params.reverb_size.set_plain_value(v),
            "reverbDamping" => self.params.reverb_damping.set_plain_value(v),
            "reverbMix" => self.params.reverb_mix.set_plain_value(v),
            "fxOrder" => {
                // 0 = delay-reverb, 1 = reverb-delay
                self.params.fx_order.set_plain_value(if change.value > 0.5 { 1.0 } else { 0.0 });
            }
            _ => {}
        }
    }
}

impl ClapPlugin for HardwaveWettBoi {
    const CLAP_ID: &'static str = "com.hardwavestudios.wettboi";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Wet signal processor — filters, ADSR, delay, reverb");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[
        ClapFeature::AudioEffect,
        ClapFeature::Filter,
        ClapFeature::Delay,
        ClapFeature::Reverb,
    ];
}

impl Vst3Plugin for HardwaveWettBoi {
    const VST3_CLASS_ID: [u8; 16] = *b"HWWettBoi__v001\0";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[
        Vst3SubCategory::Fx,
        Vst3SubCategory::Filter,
        Vst3SubCategory::Delay,
        Vst3SubCategory::Reverb,
    ];
}

nih_export_clap!(HardwaveWettBoi);
nih_export_vst3!(HardwaveWettBoi);

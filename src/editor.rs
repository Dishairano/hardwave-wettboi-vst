//! WebView-based editor for Hardwave WettBoi.
//!
//! Uses the same hwpacket bridge pattern as LoudLab/KickForge:
//! - Linux/macOS: Rust pushes state via `evaluate_script()`.
//! - Windows: Rust starts a local TCP server, JS polls via `fetch()`.

use crossbeam_channel::{unbounded, Receiver, Sender};
use nih_plug::editor::Editor;
use nih_plug::prelude::{GuiContext, Param, ParentWindowHandle};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::auth;
use crate::params::WettBoiParams;
use crate::protocol::WbPacket;

const WETTBOI_URL: &str = "https://wettboi.hardwavestudios.com/vst/wettboi";
const EDITOR_WIDTH: u32 = 1280;
const EDITOR_HEIGHT: u32 = 720;
const MIN_WIDTH: u32 = 600;
const MIN_HEIGHT: u32 = 380;
const MAX_WIDTH: u32 = 2560;
const MAX_HEIGHT: u32 = 1600;

struct RwhWrapper(usize);

unsafe impl Send for RwhWrapper {}
unsafe impl Sync for RwhWrapper {}

impl raw_window_handle::HasWindowHandle for RwhWrapper {
    fn window_handle(
        &self,
    ) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
        use raw_window_handle::RawWindowHandle;

        #[cfg(target_os = "linux")]
        let raw = {
            let h = raw_window_handle::XlibWindowHandle::new(self.0 as _);
            RawWindowHandle::Xlib(h)
        };

        #[cfg(target_os = "macos")]
        let raw = {
            let ns_view = std::ptr::NonNull::new(self.0 as *mut _)
                .ok_or(raw_window_handle::HandleError::Unavailable)?;
            let h = raw_window_handle::AppKitWindowHandle::new(ns_view);
            RawWindowHandle::AppKit(h)
        };

        #[cfg(target_os = "windows")]
        let raw = {
            let hwnd = std::num::NonZeroIsize::new(self.0 as isize)
                .ok_or(raw_window_handle::HandleError::Unavailable)?;
            let h = raw_window_handle::Win32WindowHandle::new(hwnd);
            RawWindowHandle::Win32(h)
        };

        Ok(unsafe { raw_window_handle::WindowHandle::borrow_raw(raw) })
    }
}

impl raw_window_handle::HasDisplayHandle for RwhWrapper {
    fn display_handle(
        &self,
    ) -> Result<raw_window_handle::DisplayHandle<'_>, raw_window_handle::HandleError> {
        use raw_window_handle::RawDisplayHandle;

        #[cfg(target_os = "linux")]
        let raw = RawDisplayHandle::Xlib(raw_window_handle::XlibDisplayHandle::new(None, 0));

        #[cfg(target_os = "macos")]
        let raw = RawDisplayHandle::AppKit(raw_window_handle::AppKitDisplayHandle::new());

        #[cfg(target_os = "windows")]
        let raw = RawDisplayHandle::Windows(raw_window_handle::WindowsDisplayHandle::new());

        Ok(unsafe { raw_window_handle::DisplayHandle::borrow_raw(raw) })
    }
}

/// Build a map of param ID strings to ParamPtr for the IPC handler.
fn build_param_map(params: &WettBoiParams) -> HashMap<String, nih_plug::prelude::ParamPtr> {
    elog!("[HardwaveWettBoi] Building param map...");
    let mut map = HashMap::new();

    // Reverb
    map.insert("rev_enabled".into(), params.rev_enabled.as_ptr());
    map.insert("rev_type".into(), params.rev_type.as_ptr());
    map.insert("rev_predelay".into(), params.rev_predelay.as_ptr());
    map.insert("rev_size".into(), params.rev_size.as_ptr());
    map.insert("rev_decay".into(), params.rev_decay.as_ptr());
    map.insert("rev_damp".into(), params.rev_damp.as_ptr());
    map.insert("rev_width".into(), params.rev_width.as_ptr());
    map.insert("rev_wet".into(), params.rev_wet.as_ptr());
    map.insert("rev_freeze".into(), params.rev_freeze.as_ptr());
    map.insert("rev_eq_hp".into(), params.rev_eq_hp.as_ptr());
    map.insert("rev_eq_lp".into(), params.rev_eq_lp.as_ptr());

    // Sidechain
    map.insert("sc_threshold".into(), params.sc_threshold.as_ptr());
    map.insert("sc_attack".into(), params.sc_attack.as_ptr());
    map.insert("sc_hold".into(), params.sc_hold.as_ptr());
    map.insert("sc_release".into(), params.sc_release.as_ptr());
    map.insert("sc_source".into(), params.sc_source.as_ptr());

    // LFO
    map.insert("lfo_enabled".into(), params.lfo_enabled.as_ptr());
    map.insert("lfo_rate".into(), params.lfo_rate.as_ptr());
    map.insert("lfo_depth".into(), params.lfo_depth.as_ptr());
    map.insert("lfo_phase".into(), params.lfo_phase.as_ptr());
    map.insert("lfo_shape".into(), params.lfo_shape.as_ptr());
    map.insert("lfo_target".into(), params.lfo_target.as_ptr());

    // Delay
    map.insert("dly_enabled".into(), params.dly_enabled.as_ptr());
    map.insert("dly_sync".into(), params.dly_sync.as_ptr());
    map.insert("dly_time_l".into(), params.dly_time_l.as_ptr());
    map.insert("dly_time_r".into(), params.dly_time_r.as_ptr());
    map.insert("dly_note_l".into(), params.dly_note_l.as_ptr());
    map.insert("dly_note_r".into(), params.dly_note_r.as_ptr());
    map.insert("dly_feedback".into(), params.dly_feedback.as_ptr());
    map.insert("dly_hp".into(), params.dly_hp.as_ptr());
    map.insert("dly_lp".into(), params.dly_lp.as_ptr());
    map.insert("dly_ping_pong".into(), params.dly_ping_pong.as_ptr());
    map.insert("dly_wet".into(), params.dly_wet.as_ptr());
    map.insert("dly_mod_rate".into(), params.dly_mod_rate.as_ptr());
    map.insert("dly_mod_depth".into(), params.dly_mod_depth.as_ptr());
    map.insert("dly_saturation".into(), params.dly_saturation.as_ptr());

    // Global
    map.insert("mix".into(), params.mix.as_ptr());
    map.insert("bypass".into(), params.bypass.as_ptr());
    map.insert("routing".into(), params.routing.as_ptr());

    elog!("[HardwaveWettBoi] Param map built: {} entries", map.len());
    map
}

/// Create a snapshot of the current DAW params as a `WbPacket`.
pub fn snapshot_params(
    params: &WettBoiParams,
    bpm: f32,
    duck_depth: f32,
    lfo_value: f32,
) -> WbPacket {
    use crate::params::{LfoShape, LfoTarget, NoteDiv, ReverbType, RoutingMode, ScSource};

    let rev_type_str = match params.rev_type.value() {
        ReverbType::Room => "room",
        ReverbType::Hall => "hall",
        ReverbType::Plate => "plate",
        ReverbType::Spring => "spring",
    };

    let sc_source_str = match params.sc_source.value() {
        ScSource::Internal => "internal",
        ScSource::Sidechain => "sidechain",
    };

    let lfo_shape_str = match params.lfo_shape.value() {
        LfoShape::Sine => "sine",
        LfoShape::Tri => "tri",
        LfoShape::Saw => "saw",
        LfoShape::Square => "square",
        LfoShape::SampleAndHold => "s&h",
    };

    let lfo_target_str = match params.lfo_target.value() {
        LfoTarget::RevWet => "rev_wet",
        LfoTarget::DlyWet => "dly_wet",
        LfoTarget::DlyFeedback => "dly_fb",
        LfoTarget::Filter => "filter",
    };

    let routing_str = match params.routing.value() {
        RoutingMode::Parallel => "parallel",
        RoutingMode::ReverbToDelay => "rev_to_dly",
        RoutingMode::DelayToReverb => "dly_to_rev",
    };

    // One table for both directions: a name the UI sends must be the same name
    // the plugin reports back, or the selected button stops matching what is set.
    let note_to_str = |n: NoteDiv| -> &'static str {
        NOTE_DIV_NAMES[match n {
            NoteDiv::Sixteenth => 0,
            NoteDiv::Eighth => 1,
            NoteDiv::DottedEighth => 2,
            NoteDiv::Quarter => 3,
            NoteDiv::DottedQuarter => 4,
            NoteDiv::Half => 5,
            NoteDiv::DottedHalf => 6,
            NoteDiv::Whole => 7,
        }]
    };

    WbPacket {
        bpm,
        rev_enabled: params.rev_enabled.value(),
        rev_predelay: params.rev_predelay.value(),
        rev_size: params.rev_size.value(),
        rev_decay: params.rev_decay.value(),
        rev_damp: params.rev_damp.value(),
        rev_width: params.rev_width.value(),
        rev_wet: params.rev_wet.value(),
        rev_type: rev_type_str.to_string(),
        rev_freeze: params.rev_freeze.value(),
        rev_eq_hp: params.rev_eq_hp.value(),
        rev_eq_lp: params.rev_eq_lp.value(),
        sc_threshold: params.sc_threshold.value(),
        sc_key_level: 0.0,
        sc_threshold_lin: 10.0_f32.powf(params.sc_threshold.value() / 20.0),
        sc_attack: params.sc_attack.value(),
        sc_hold: params.sc_hold.value(),
        sc_release: params.sc_release.value(),
        sc_source: sc_source_str.to_string(),
        sc_duck_depth: duck_depth,
        lfo_enabled: params.lfo_enabled.value(),
        lfo_rate: params.lfo_rate.value(),
        lfo_depth: params.lfo_depth.value(),
        lfo_phase: params.lfo_phase.value(),
        lfo_shape: lfo_shape_str.to_string(),
        lfo_target: lfo_target_str.to_string(),
        lfo_value,
        dly_enabled: params.dly_enabled.value(),
        dly_sync: params.dly_sync.value(),
        dly_time_l: params.dly_time_l.value(),
        dly_time_r: params.dly_time_r.value(),
        dly_note_l: note_to_str(params.dly_note_l.value()).to_string(),
        dly_note_r: note_to_str(params.dly_note_r.value()).to_string(),
        dly_feedback: params.dly_feedback.value(),
        dly_hp: params.dly_hp.value(),
        dly_lp: params.dly_lp.value(),
        dly_ping_pong: params.dly_ping_pong.value(),
        dly_wet: params.dly_wet.value(),
        dly_mod_rate: params.dly_mod_rate.value(),
        dly_mod_depth: params.dly_mod_depth.value(),
        dly_saturation: params.dly_saturation.value(),
        mix: params.mix.value(),
        bypass: params.bypass.value(),
        routing: routing_str.to_string(),
        preset: "Init".to_string(),
        input_peak_l: 0.0,
        input_peak_r: 0.0,
        output_peak_l: 0.0,
        output_peak_r: 0.0,
    }
}

/// Build the init JavaScript that gets injected into the webview on load.
fn ipc_init_script(params: &WettBoiParams, bpm: f32) -> String {
    let snapshot = snapshot_params(params, bpm, 0.0, 0.0);
    let initial_json = serde_json::to_string(&snapshot).unwrap_or_else(|_| "null".into());
    let version = env!("CARGO_PKG_VERSION");

    format!(
        r#"
(function() {{
    var _focusTimer = null;
    window.addEventListener('mouseup', function(e) {{
        if (e.target.tagName !== 'INPUT') {{
            clearTimeout(_focusTimer);
            _focusTimer = setTimeout(function() {{
                try {{ window.ipc.postMessage(JSON.stringify({{ type: 'release_focus' }})); }} catch(_) {{}}
            }}, 500);
        }}
    }}, true);
    document.addEventListener('blur', function(e) {{
        if (e.target.tagName === 'INPUT') {{
            clearTimeout(_focusTimer);
            try {{ window.ipc.postMessage(JSON.stringify({{ type: 'release_focus' }})); }} catch(_) {{}}
        }}
    }}, true);
}})();

window.__HARDWAVE_VST = true;
window.__HARDWAVE_VST_VERSION = '{version}';
window.__hardwave = {{
    postMessage: function(msg) {{
        window.ipc.postMessage(JSON.stringify(msg));
    }}
}};

(function() {{
    var _init = {initial_json};
    function pushInit() {{
        if (window.__onWbPacket) {{
            window.__onWbPacket(_init);
        }} else {{
            setTimeout(pushInit, 50);
        }}
    }}
    if (document.readyState === 'complete') {{ pushInit(); }}
    else {{ window.addEventListener('load', pushInit); }}
}})();
"#,
    )
}

/// Map string enum values from the JS UI to nih-plug plain param values (variant index).
/// The index of a note division as the UI names it, matching the order of the
/// `NoteDiv` variants in `params.rs`. Returns None for a name we do not have.
pub fn note_div_index(name: &str) -> Option<usize> {
    NOTE_DIV_NAMES.iter().position(|n| *n == name)
}

/// Every note division, in `NoteDiv` order. The UI shows exactly these.
pub const NOTE_DIV_NAMES: [&str; 8] = ["1/16", "1/8", "d1/8", "1/4", "d1/4", "1/2", "d1/2", "1/1"];

/// Every enum value the UI can send has to land on a variant index here, or the
/// parameter silently never moves. That is not a hypothetical: the two delay
/// note divisions were missing from this table in the shipped plugin, so every
/// click on a note button and every preset that set one did nothing at all. And
/// because tempo sync is on by default, those buttons were the only delay-time
/// control a user could see. Public so a test can walk the whole vocabulary.
pub fn string_to_param_value(param_id: &str, s: &str) -> Option<f32> {
    match param_id {
        "rev_type" => match s {
            "room" => Some(0.0),
            "hall" => Some(1.0),
            "plate" => Some(2.0),
            "spring" => Some(3.0),
            _ => None,
        },
        "sc_source" => match s {
            "internal" => Some(0.0),
            "sidechain" => Some(1.0),
            _ => None,
        },
        "lfo_shape" => match s {
            "sine" => Some(0.0),
            "tri" => Some(1.0),
            "saw" => Some(2.0),
            "square" => Some(3.0),
            "s&h" => Some(4.0),
            _ => None,
        },
        "lfo_target" => match s {
            "rev_wet" => Some(0.0),
            "dly_wet" => Some(1.0),
            "dly_fb" => Some(2.0),
            "filter" => Some(3.0),
            _ => None,
        },
        "routing" => match s {
            "parallel" => Some(0.0),
            "rev_to_dly" => Some(1.0),
            "dly_to_rev" => Some(2.0),
            _ => None,
        },
        // The note divisions were missing here, so every click on a delay note
        // fell through to None and the parameter never moved. Tempo sync is the
        // default mode, which made the note buttons the only visible time
        // control, and none of them did anything. Order must match the NoteDiv
        // variants in params.rs; note_div_index is the single source of truth
        // for that and is covered by a test.
        "dly_note_l" | "dly_note_r" => note_div_index(s).map(|i| i as f32),
        _ => None,
    }
}

/// Handle IPC messages from the webview.
fn handle_ipc(
    context: &Arc<dyn GuiContext>,
    param_map: &HashMap<String, nih_plug::prelude::ParamPtr>,
    raw_body: &str,
    _parent_hwnd: usize,
    editor_size: &Arc<Mutex<(u32, u32)>>,
    resize_tx: &Arc<Mutex<Option<Sender<(u32, u32)>>>>,
) {
    let msg: serde_json::Value = match serde_json::from_str(raw_body) {
        Ok(v) => v,
        Err(e) => {
            elog!(
                "[HardwaveWettBoi] IPC parse error: {} — raw: {}",
                e,
                &raw_body[..raw_body.len().min(200)]
            );
            return;
        }
    };

    let msg_type = msg.get("type").and_then(|t| t.as_str()).unwrap_or("");
    match msg_type {
        "set_param" => {
            let id = msg.get("id").and_then(|v| v.as_str()).unwrap_or("");
            let raw_value = msg.get("value");

            // Resolve value: number, boolean (true→1.0, false→0.0), or string enum
            let value: Option<f32> = raw_value.and_then(|v| {
                if let Some(f) = v.as_f64() {
                    Some(f as f32)
                } else if let Some(b) = v.as_bool() {
                    Some(if b { 1.0 } else { 0.0 })
                } else if let Some(s) = v.as_str() {
                    string_to_param_value(id, s)
                } else {
                    None
                }
            });

            if let (Some(val), Some(ptr)) = (value, param_map.get(id)) {
                unsafe {
                    let normalized = ptr.preview_normalized(val);
                    context.raw_begin_set_parameter(*ptr);
                    context.raw_set_parameter_normalized(*ptr, normalized);
                    context.raw_end_set_parameter(*ptr);
                }
            } else if value.is_none() {
                elog!(
                    "[HardwaveWettBoi] IPC set_param '{}': could not parse value {:?}",
                    id,
                    raw_value
                );
            } else {
                elog!("[HardwaveWettBoi] IPC set_param: unknown param id '{}'", id);
            }
        }
        "release_focus" => {
            #[cfg(target_os = "windows")]
            unsafe {
                use windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus;
                SetFocus(_parent_hwnd as windows_sys::Win32::Foundation::HWND);
            }
        }
        "resize" => {
            let w = msg.get("width").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
            let h = msg.get("height").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
            elog!("[HardwaveWettBoi] IPC resize: {}x{}", w, h);
            if (MIN_WIDTH..=MAX_WIDTH).contains(&w) && (MIN_HEIGHT..=MAX_HEIGHT).contains(&h) {
                *editor_size.lock() = (w, h);
                if context.request_resize() {
                    if let Some(tx) = resize_tx.lock().as_ref() {
                        let _ = tx.send((w, h));
                    }
                }
            } else {
                elog!(
                    "[HardwaveWettBoi] IPC resize: out of bounds ({}x{} not in {}x{}–{}x{})",
                    w,
                    h,
                    MIN_WIDTH,
                    MIN_HEIGHT,
                    MAX_WIDTH,
                    MAX_HEIGHT
                );
            }
        }
        "save_token" => {
            elog!("[HardwaveWettBoi] IPC save_token: persisting to disk");
            if let Some(token) = msg.get("token").and_then(|v| v.as_str()) {
                match auth::save_token(token) {
                    Ok(()) => elog!("[HardwaveWettBoi] Token saved successfully"),
                    Err(e) => elog!("[HardwaveWettBoi] Token save FAILED: {}", e),
                }
            }
        }
        "clear_token" => {
            elog!("[HardwaveWettBoi] IPC clear_token: removing from disk");
            match auth::clear_token() {
                Ok(()) => elog!("[HardwaveWettBoi] Token cleared"),
                Err(e) => elog!("[HardwaveWettBoi] Token clear FAILED: {}", e),
            }
        }
        other => {
            elog!("[HardwaveWettBoi] IPC unknown message type: '{}'", other);
        }
    }
}

pub struct WettBoiEditor {
    params: Arc<WettBoiParams>,
    packet_rx: Arc<Mutex<Receiver<WbPacket>>>,
    auth_token: Option<String>,
    scale_factor: Mutex<f32>,
    editor_size: Arc<Mutex<(u32, u32)>>,
    resize_tx: Arc<Mutex<Option<Sender<(u32, u32)>>>>,
}

impl WettBoiEditor {
    pub fn new(
        params: Arc<WettBoiParams>,
        packet_rx: Arc<Mutex<Receiver<WbPacket>>>,
        auth_token: Option<String>,
    ) -> Self {
        Self {
            params,
            packet_rx,
            auth_token,
            scale_factor: Mutex::new(1.0),
            editor_size: Arc::new(Mutex::new((EDITOR_WIDTH, EDITOR_HEIGHT))),
            resize_tx: Arc::new(Mutex::new(None)),
        }
    }

    fn scaled_size(&self) -> (u32, u32) {
        let (w, h) = *self.editor_size.lock();
        let f = *self.scale_factor.lock();
        ((w as f32 * f) as u32, (h as f32 * f) as u32)
    }
}

impl Editor for WettBoiEditor {
    fn spawn(
        &self,
        parent: ParentWindowHandle,
        context: Arc<dyn GuiContext>,
    ) -> Box<dyn std::any::Any + Send> {
        let scale = *self.scale_factor.lock();
        // First line of every run, so a log sent to support says which build
        // and which machine wrote the lines under it.
        elog!(
            "[HardwaveWettBoi] ---- editor opening: v{} on {} ----",
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS
        );
        elog!(
            "[HardwaveWettBoi] Editor::spawn — scale_factor={:.2}, auth_token={}",
            scale,
            if self.auth_token.is_some() {
                "present"
            } else {
                "none"
            }
        );
        #[cfg(target_os = "windows")]
        pin_own_module();

        let packet_rx = Arc::clone(&self.packet_rx);
        let (width, height) = self.scaled_size();
        elog!(
            "[HardwaveWettBoi] Editor size: {}x{} (scaled)",
            width,
            height
        );

        let version = env!("CARGO_PKG_VERSION");
        let url = match &self.auth_token {
            Some(t) => format!("{}?token={}&v={}", WETTBOI_URL, t, version),
            None => format!("{}?v={}", WETTBOI_URL, version),
        };
        elog!(
            "[HardwaveWettBoi] Loading URL: {} (token {})",
            WETTBOI_URL,
            if self.auth_token.is_some() {
                "injected"
            } else {
                "absent"
            }
        );

        let param_map = Arc::new(build_param_map(&self.params));
        let init_js = ipc_init_script(&self.params, 150.0);
        elog!("[HardwaveWettBoi] Init script: {} bytes", init_js.len());
        let raw_handle = extract_raw_handle(&parent);
        elog!("[HardwaveWettBoi] Parent window handle: 0x{:x}", raw_handle);

        let (resize_tx_val, resize_rx) = unbounded::<(u32, u32)>();
        *self.resize_tx.lock() = Some(resize_tx_val);

        let editor_size = Arc::clone(&self.editor_size);
        let resize_tx = Arc::clone(&self.resize_tx);

        #[cfg(target_os = "windows")]
        {
            elog!("[HardwaveWettBoi] Platform: Windows — using TCP polling bridge");
            spawn_windows(
                raw_handle,
                url,
                width,
                height,
                packet_rx,
                context,
                param_map,
                init_js,
                resize_rx,
                editor_size,
                resize_tx,
            )
        }

        #[cfg(target_os = "macos")]
        {
            spawn_macos(
                raw_handle,
                url,
                width,
                height,
                packet_rx,
                context,
                param_map,
                init_js,
                resize_rx,
                editor_size,
                resize_tx,
            )
        }

        #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
        {
            elog!("[HardwaveWettBoi] Platform: Unix — using evaluate_script bridge");
            spawn_unix(
                raw_handle,
                url,
                width,
                height,
                packet_rx,
                context,
                param_map,
                init_js,
                resize_rx,
                editor_size,
                resize_tx,
            )
        }
    }

    fn size(&self) -> (u32, u32) {
        self.scaled_size()
    }

    fn set_scale_factor(&self, factor: f32) -> bool {
        // Clamp host-supplied DPI scale to a sane range so a misbehaving
        // host can't shrink the editor to zero pixels.
        let clamped = factor.clamp(0.5, 4.0);
        *self.scale_factor.lock() = clamped;
        true
    }

    fn set_size(&self, width: u32, height: u32) {
        let w = width.clamp(MIN_WIDTH, MAX_WIDTH);
        let h = height.clamp(MIN_HEIGHT, MAX_HEIGHT);
        *self.editor_size.lock() = (w, h);
        if let Some(tx) = self.resize_tx.lock().as_ref() {
            let _ = tx.send((w, h));
        }
    }

    fn param_value_changed(&self, _id: &str, _normalized_value: f32) {}
    fn param_modulation_changed(&self, _id: &str, _modulation_offset: f32) {}
    fn param_values_changed(&self) {}
}

fn extract_raw_handle(parent: &ParentWindowHandle) -> usize {
    match *parent {
        #[cfg(target_os = "linux")]
        ParentWindowHandle::X11Window(id) => id as usize,
        #[cfg(target_os = "macos")]
        ParentWindowHandle::AppKitNsView(ptr) => ptr as usize,
        #[cfg(target_os = "windows")]
        ParentWindowHandle::Win32Hwnd(h) => h as usize,
        _ => 0,
    }
}

fn webview_data_dir() -> std::path::PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("hardwave")
        .join("wettboi-webview")
}

// ─── Windows: TCP polling approach ─────────────────────────────────────────

#[cfg(target_os = "windows")]
fn webview2_data_dir() -> std::path::PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("hardwave")
        .join("wettboi-webview2")
}

#[cfg(target_os = "windows")]
/// Keep this plug-in's own module loaded for the life of the host process (Windows only).
///
/// Reported from the field by a producer running MPC 3 and MPC 2 desktop, with a crash dump: the
/// host loads `hardwave-<slug>.vst3`, unloads it, and loads it again. The webview we build for the
/// editor registers a Win32 window class from inside this module, and a window class outlives the
/// module that registered it. On the second load `CreateWindowExW` reuses that class, whose
/// `lpfnWndProc` still points into the first, now freed, copy of the module, and the first message
/// the window receives jumps into unmapped memory. His dump showed exactly that: two load instances
/// at different base addresses, and the faulting jump through the older one, `!Unloaded`.
///
/// `GET_MODULE_HANDLE_EX_FLAG_PIN` adds a reference the loader never releases, so the module stays
/// mapped and those function pointers stay valid however many times the host loads it. It costs one
/// module's worth of address space for the life of the process, which is what a plug-in that is
/// opened twice costs anyway.
#[cfg(target_os = "windows")]
fn pin_own_module() -> bool {
    use std::sync::Once;
    static PIN: Once = Once::new();
    static PINNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

    const GET_MODULE_HANDLE_EX_FLAG_PIN: u32 = 0x0000_0001;
    const GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS: u32 = 0x0000_0004;

    #[link(name = "kernel32")]
    extern "system" {
        fn GetModuleHandleExW(flags: u32, module_name: *const u16, module: *mut isize) -> i32;
    }

    PIN.call_once(|| {
        // FROM_ADDRESS takes an address where a name would go: any address inside this module
        // identifies it, so the address of this function is used.
        let addr = pin_own_module as *const () as *const u16;
        let mut handle: isize = 0;
        let ok = unsafe {
            GetModuleHandleExW(
                GET_MODULE_HANDLE_EX_FLAG_PIN | GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS,
                addr,
                &mut handle,
            )
        };
        PINNED.store(ok != 0, std::sync::atomic::Ordering::Relaxed);
        if ok == 0 {
            elog!("[HardwaveWettBoi] could not pin the module; a reload may crash the host");
        } else {
            elog!("[HardwaveWettBoi] module pinned for the life of the process");
        }
    });
    PINNED.load(std::sync::atomic::Ordering::Relaxed)
}

/// One line at each call site: the interface when it can be reached, a page that says why not when
/// it cannot. Three platforms build the webview slightly differently, and none of them should have
/// to spell this out.
trait WithUrlOrOffline {
    fn with_url_or_offline(self, url: &str) -> Self;
}

impl WithUrlOrOffline for wry::WebViewBuilder<'_> {
    fn with_url_or_offline(self, url: &str) -> Self {
        if interface_reachable(url) {
            self.with_url(url)
        } else {
            self.with_html(offline_page(url))
        }
    }
}

/// Is the interface reachable right now?
///
/// The whole editor is a web page served from our own domain, so with no route to it the window
/// draws nothing at all: two producers have reported a plug-in that opens "blank", and a blank
/// window tells them nothing about why. This asks once, quickly, before the webview is built.
///
/// Anything other than a clear network failure counts as reachable: a redirect, a 403, a 500, all
/// mean something answered, and the page itself handles those far better than a guess here would.
/// Never called from the audio thread; `Editor::spawn` runs on the host's UI thread.
/// Ask, before the WebView opens, whether the interface can be reached.
///
/// Answering "no" costs the user their whole window, so this only answers "no"
/// when it is sure. The WebView has its own network stack: it follows the
/// system proxy, it has its own cache, and on Windows it runs in a process the
/// firewall may allow where it blocks the DAW. A slow or half-open network
/// therefore says nothing about whether the page would have loaded, and a
/// timeout here used to replace a working interface with an apology.
///
/// Only a refused connection or a name that does not resolve is treated as
/// offline. Anything else — a timeout, a TLS error, a proxy that will not talk
/// to us — loads the URL and lets the WebView try, because it may well succeed.
fn interface_reachable(url: &str) -> bool {
    let (reachable, failure) = probe_interface(url);
    if let Some(reason) = failure {
        elog!(
            "[HardwaveWettBoi] probe failed ({}): {}",
            reason,
            if reachable {
                "loading the interface anyway, the WebView may get through"
            } else {
                "showing the offline page"
            }
        );
    }
    reachable
}

/// The probe itself: whether to load the interface, and why the probe failed if it did.
///
/// The probe asks the address without its query string. The query carries the licence token, and
/// the probe has no use for it: it only asks whether the host answers. Before, the token went out
/// on this extra request, and because ureq writes the full URL into its error text, it also went
/// into `wettboi-editor.log` on every failed probe, the one file we ask users to send us.
///
/// The verdict is taken from ureq's error kind and the cause under it, never from the whole error
/// text. That text starts with the URL, so a token that happened to contain "dns" or "refused"
/// turned a timeout into a certain "offline" and took the window away from that one user.
fn probe_interface(url: &str) -> (bool, Option<String>) {
    match ureq::builder()
        .timeout_connect(std::time::Duration::from_millis(1500))
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .head(without_query(url))
        .call()
    {
        Ok(_) => (true, None),
        // A status code is an answer: the server is there.
        Err(ureq::Error::Status(_, _)) => (true, None),
        Err(ureq::Error::Transport(t)) => {
            let reason = transport_reason(&t);
            let definite = offline_is_certain(t.kind(), &reason);
            (!definite, Some(reason))
        }
    }
}

/// `url` up to its query string or fragment.
fn without_query(url: &str) -> &str {
    url.split(['?', '#']).next().unwrap_or(url)
}

/// What went wrong, without the URL ureq puts in front of it: the kind, ureq's own message, and
/// the error underneath, which is where the operating system says "refused" or "timed out".
fn transport_reason(t: &ureq::Transport) -> String {
    let mut reason = t.kind().to_string();
    if let Some(message) = t.message() {
        reason.push_str(": ");
        reason.push_str(message);
    }
    if let Some(source) = std::error::Error::source(t) {
        reason.push_str(": ");
        reason.push_str(&source.to_string());
    }
    reason
}

/// What the window shows when the interface cannot be reached, instead of nothing.
///
/// Plain HTML with no request of its own, because the one thing we know here is that requests are
/// failing. Trying again is a link back to the interface: if the connection has come back, the
/// window simply loads.
/// Does this transport error mean the machine cannot get there at all?
///
/// A refused connection and a name that does not resolve are answers: nothing
/// is listening, or the host does not exist for this machine. A timeout is not
/// an answer, and neither is a TLS or proxy failure, because the WebView uses
/// neither our sockets nor our trust store.
///
/// `reason` is the cause from [`transport_reason`], which holds no URL. Only a failed name lookup
/// or a failed connect can be certain; a TLS setup failure is also reported as a failed connect by
/// ureq, which is why the cause is read as well as the kind.
fn offline_is_certain(kind: ureq::ErrorKind, reason: &str) -> bool {
    let r = reason.to_ascii_lowercase();
    // ureq reports whatever the resolver returned as a DNS failure, and our own deadline runs
    // through the resolver too. A name server that is slow to answer therefore arrives here
    // looking exactly like a name that does not exist. A machine that timed out may be on a
    // network the WebView gets through, so a timeout is never a certain answer.
    if r.contains("timed out") || r.contains("timeout") || r.contains("would block") {
        return false;
    }
    match kind {
        ureq::ErrorKind::Dns => true,
        ureq::ErrorKind::ConnectionFailed => {
            r.contains("refused") || r.contains("unreachable") || r.contains("no route")
        }
        _ => false,
    }
}

fn offline_page(url: &str) -> String {
    format!(
        r#"<!doctype html><html><head><meta charset="utf-8"><title>HardwaveWettBoi</title>
<style>
  html,body {{ margin:0; height:100%; background:#0a0a0b; color:#c8c8c8;
    font:13px/1.6 -apple-system,Segoe UI,Roboto,Helvetica,Arial,sans-serif; }}
  .box {{ height:100%; display:flex; align-items:center; justify-content:center; }}
  .card {{ max-width:430px; padding:26px 28px; border:1px solid rgba(255,255,255,.09); border-radius:14px; }}
  h1 {{ font-size:15px; margin:0 0 10px; color:#fff; }}
  p {{ margin:0 0 10px; }}
  ul {{ margin:0 0 14px; padding-left:18px; }} li {{ margin:3px 0; }}
  a.retry {{ display:inline-block; padding:7px 14px; border-radius:8px; background:#DC2626;
    color:#fff; text-decoration:none; font-weight:700; }}
  .small {{ color:#6f6f6f; font-size:11.5px; margin-top:12px; }}
</style></head><body><div class="box"><div class="card">
<h1>The plug-in cannot reach its interface</h1>
<p>The controls are served from hardwavestudios.com, and this computer could not get there just now.
The audio side of the plug-in is unaffected: your project still plays.</p>
<ul>
  <li>Check this machine is online.</li>
  <li>A firewall, a VPN or a studio network may be blocking the DAW rather than the browser.</li>
  <li>If your DAW blocks internet access per plug-in, allow it for this one.</li>
</ul>
<a class="retry" href="{url}">Try again</a>
<div class="small">If it keeps happening, write to support@hardwavestudios.com and say which DAW and which network you are on.</div>
</div></div></body></html>"#,
        url = url
    )
}

#[cfg(target_os = "windows")]
fn spawn_windows(
    raw_handle: usize,
    url: String,
    width: u32,
    height: u32,
    packet_rx: Arc<Mutex<Receiver<WbPacket>>>,
    context: Arc<dyn GuiContext>,
    param_map: Arc<HashMap<String, nih_plug::prelude::ParamPtr>>,
    base_init_js: String,
    resize_rx: Receiver<(u32, u32)>,
    editor_size: Arc<Mutex<(u32, u32)>>,
    resize_tx: Arc<Mutex<Option<Sender<(u32, u32)>>>>,
) -> Box<dyn std::any::Any + Send> {
    use std::io::{Read as IoRead, Write as IoWrite};
    use std::net::TcpListener;

    let running = Arc::new(AtomicBool::new(true));
    let running_clone = Arc::clone(&running);

    let listener = match TcpListener::bind("127.0.0.1:0") {
        Ok(l) => l,
        Err(e) => {
            elog!("[HardwaveWettBoi] failed to bind TCP: {}", e);
            return Box::new(EditorHandle {
                running: running_clone,
                _webview: None,
                _web_context: None,
                _server_thread: None,
                _editor_thread: None,
            });
        }
    };
    let port = listener.local_addr().unwrap().port();
    elog!("[HardwaveWettBoi] TCP server bound on 127.0.0.1:{}", port);
    let latest_json = Arc::new(Mutex::new(String::from("{}")));
    let latest_json_server = Arc::clone(&latest_json);
    let running_server = Arc::clone(&running);

    let server_thread = std::thread::spawn(move || {
        listener.set_nonblocking(true).ok();
        while running_server.load(Ordering::Relaxed) {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf);
                let body = latest_json_server.lock().clone();
                let response = format!(
                    "HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes());
            }
            if let Some(rx) = packet_rx.try_lock() {
                while let Ok(pkt) = rx.try_recv() {
                    if let Ok(json) = serde_json::to_string(&pkt) {
                        *latest_json.lock() = json;
                    }
                }
            }
            while resize_rx.try_recv().is_ok() {}
            std::thread::sleep(std::time::Duration::from_millis(8));
        }
    });

    let poll_script = format!(
        r#"
(function() {{
    var _port = {port};
    function poll() {{
        fetch('http://127.0.0.1:' + _port)
            .then(function(r) {{ return r.json(); }})
            .then(function(data) {{
                if (window.__onWbPacket) window.__onWbPacket(data);
            }})
            .catch(function() {{}});
        setTimeout(poll, 16);
    }}
    poll();
}})();
"#,
    );

    let init_js = format!("{}\n{}", base_init_js, poll_script);
    // A WebView2 environment owns its user data folder for as long as it lives, and a second
    // environment cannot open a folder the first still holds. MPC loads this plug-in twice in one
    // process: once the module pin stopped the reload crash, the second load started failing here
    // instead, and a failure here leaves an empty window. So the shared folder is tried first,
    // because that is where the sign-in lives, and a folder of this instance's own is tried when
    // the shared one is taken.
    // Both loads are in one process, so the process id tells them apart from nothing. The second
    // one takes a fixed second folder, which keeps its sign-in between sessions like the first.
    // Only a third instance needs a folder named after the moment it opened, and that one is
    // rare enough to be worth the clutter it leaves behind.
    let shared_dir = webview2_data_dir();
    let name = shared_dir
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "wettboi-webview2".to_string());
    let second_dir = shared_dir.with_file_name(format!("{name}-2"));
    let unique_dir = shared_dir.with_file_name(format!(
        "{name}-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));

    let wrapper = RwhWrapper(raw_handle);
    let mut webview = None;
    let mut web_context = None;

    for dir in [shared_dir, second_dir, unique_dir] {
        elog!("[HardwaveWettBoi] WebView2 data dir: {:?}", dir);
        let _ = std::fs::create_dir_all(&dir);
        let mut context_attempt = wry::WebContext::new(Some(dir.clone()));

        let ctx = Arc::clone(&context);
        let pmap = Arc::clone(&param_map);
        let esize = Arc::clone(&editor_size);
        let rtx = Arc::clone(&resize_tx);

        elog!(
            "[HardwaveWettBoi] Creating WebView2 (Windows) {}x{} ...",
            width,
            height
        );
        use wry::WebViewBuilderExtWindows;
        let built = wry::WebViewBuilder::with_web_context(&mut context_attempt)
            .with_url_or_offline(&url)
            .with_initialization_script(&init_js)
            .with_ipc_handler(move |msg| {
                handle_ipc(&ctx, &pmap, &msg.body(), raw_handle, &esize, &rtx);
            })
            .with_bounds(wry::Rect {
                position: wry::dpi::Position::Logical(wry::dpi::LogicalPosition::new(0.0, 0.0)),
                size: wry::dpi::Size::Logical(wry::dpi::LogicalSize::new(
                    width as f64,
                    height as f64,
                )),
            })
            .with_transparent(false)
            .with_devtools(false)
            // Disable WebView2 browser accelerator keys (Ctrl+P / Ctrl+S /
            // Ctrl+R / F5 / F12 / Ctrl+Shift+I) at the OS level — belt and
            // braces with the JS keydown blocker.
            .with_browser_accelerator_keys(false)
            .with_background_color((10, 10, 11, 255))
            .build(&wrapper);

        match built {
            Ok(wv) => {
                elog!(
                    "[HardwaveWettBoi] WebView created successfully in {:?}",
                    dir
                );
                webview = Some(wv);
                web_context = Some(context_attempt);
                break;
            }
            Err(e) => {
                elog!(
                    "[HardwaveWettBoi] WebView creation FAILED in {:?}: {}",
                    dir,
                    e
                );
            }
        }
    }

    if webview.is_none() {
        elog!(
            "[HardwaveWettBoi] no WebView2 environment could be created; the window will be empty"
        );
    }

    Box::new(EditorHandle {
        running: running_clone,
        _webview: webview,
        _web_context: web_context,
        _server_thread: Some(server_thread),
        _editor_thread: None,
    })
}

// ─── Linux / macOS: evaluate_script approach ───────────────────────────────

#[cfg(not(target_os = "windows"))]
fn spawn_unix(
    raw_handle: usize,
    url: String,
    width: u32,
    height: u32,
    packet_rx: Arc<Mutex<Receiver<WbPacket>>>,
    context: Arc<dyn GuiContext>,
    param_map: Arc<HashMap<String, nih_plug::prelude::ParamPtr>>,
    init_js: String,
    resize_rx: Receiver<(u32, u32)>,
    editor_size: Arc<Mutex<(u32, u32)>>,
    resize_tx: Arc<Mutex<Option<Sender<(u32, u32)>>>>,
) -> Box<dyn std::any::Any + Send> {
    let running = Arc::new(AtomicBool::new(true));
    let running_clone = Arc::clone(&running);

    let editor_thread = std::thread::spawn(move || {
        #[cfg(target_os = "linux")]
        {
            elog!("[HardwaveWettBoi] Initialising GTK...");
            let _ = gtk::init();
            elog!("[HardwaveWettBoi] GTK initialised");
        }

        let wrapper = RwhWrapper(raw_handle);
        let ctx = Arc::clone(&context);
        let pmap = Arc::clone(&param_map);
        let esize = Arc::clone(&editor_size);
        let rtx = Arc::clone(&resize_tx);

        let data_dir = webview_data_dir();
        elog!("[HardwaveWettBoi] WebView data dir: {:?}", data_dir);
        let _ = std::fs::create_dir_all(&data_dir);
        let mut web_context = wry::WebContext::new(Some(data_dir));

        elog!(
            "[HardwaveWettBoi] Creating WebKitGTK/WebKit WebView {}x{} ...",
            width,
            height
        );
        let webview = match wry::WebViewBuilder::with_web_context(&mut web_context)
            .with_url_or_offline(&url)
            .with_initialization_script(&init_js)
            .with_ipc_handler(move |msg| {
                handle_ipc(&ctx, &pmap, msg.body(), raw_handle, &esize, &rtx);
            })
            .with_bounds(wry::Rect {
                position: wry::dpi::Position::Logical(wry::dpi::LogicalPosition::new(0.0, 0.0)),
                size: wry::dpi::Size::Logical(wry::dpi::LogicalSize::new(
                    width as f64,
                    height as f64,
                )),
            })
            .with_devtools(false)
            .build_as_child(&wrapper)
        {
            Ok(wv) => {
                elog!("[HardwaveWettBoi] WebView created successfully (Unix)");
                wv
            }
            Err(e) => {
                elog!("[HardwaveWettBoi] WebView creation FAILED (Unix): {}", e);
                return;
            }
        };

        elog!("[HardwaveWettBoi] Entering editor event loop");
        while running.load(Ordering::Relaxed) {
            while let Ok((w, h)) = resize_rx.try_recv() {
                let _ = webview.set_bounds(wry::Rect {
                    position: wry::dpi::Position::Logical(wry::dpi::LogicalPosition::new(0.0, 0.0)),
                    size: wry::dpi::Size::Logical(wry::dpi::LogicalSize::new(w as f64, h as f64)),
                });
            }

            if let Some(rx) = packet_rx.try_lock() {
                while let Ok(pkt) = rx.try_recv() {
                    if let Ok(json) = serde_json::to_string(&pkt) {
                        let js = format!("window.__onWbPacket && window.__onWbPacket({})", json);
                        let _ = webview.evaluate_script(&js);
                    }
                }
            }

            #[cfg(target_os = "linux")]
            {
                while gtk::events_pending() {
                    gtk::main_iteration_do(false);
                }
            }

            std::thread::sleep(std::time::Duration::from_millis(16));
        }
    });

    Box::new(EditorHandle {
        running: running_clone,
        _webview: None,
        _web_context: None,
        _server_thread: None,
        _editor_thread: Some(editor_thread),
    })
}

/// macOS needs its own path.
///
/// WKWebView may only be created — and only be called — on the main thread.
/// The Unix path builds it inside `std::thread::spawn`, and wry rejects that
/// outright: "WebView creation FAILED (Unix): not on the main thread". Every
/// macOS user saw a white panel because of it.
///
/// The host calls `Editor::spawn` on its UI thread, which on macOS is the main
/// thread, so the webview is built here directly. The 16 ms pump that feeds
/// parameter packets and resizes then has to run there too, and it cannot block
/// the main thread — so instead of a loop it reschedules itself on the main
/// queue, which lets the host keep running between ticks.
#[cfg(target_os = "macos")]
fn spawn_macos(
    raw_handle: usize,
    url: String,
    width: u32,
    height: u32,
    packet_rx: Arc<Mutex<Receiver<WbPacket>>>,
    context: Arc<dyn GuiContext>,
    param_map: Arc<HashMap<String, nih_plug::prelude::ParamPtr>>,
    init_js: String,
    resize_rx: Receiver<(u32, u32)>,
    editor_size: Arc<Mutex<(u32, u32)>>,
    resize_tx: Arc<Mutex<Option<Sender<(u32, u32)>>>>,
) -> Box<dyn std::any::Any + Send> {
    let running = Arc::new(AtomicBool::new(true));

    let wrapper = RwhWrapper(raw_handle);
    let ctx = Arc::clone(&context);
    let pmap = Arc::clone(&param_map);
    let esize = Arc::clone(&editor_size);
    let rtx = Arc::clone(&resize_tx);

    let data_dir = webview_data_dir();
    elog!("[HardwaveWettBoi] WebView data dir: {:?}", data_dir);
    let _ = std::fs::create_dir_all(&data_dir);
    let mut web_context = wry::WebContext::new(Some(data_dir));

    elog!(
        "[HardwaveWettBoi] Creating WKWebView {}x{} on the main thread ...",
        width,
        height
    );
    let webview = match wry::WebViewBuilder::with_web_context(&mut web_context)
        .with_url_or_offline(&url)
        .with_initialization_script(&init_js)
        .with_ipc_handler(move |msg| {
            handle_ipc(&ctx, &pmap, &msg.body(), raw_handle, &esize, &rtx);
        })
        .with_bounds(wry::Rect {
            position: wry::dpi::Position::Logical(wry::dpi::LogicalPosition::new(0.0, 0.0)),
            size: wry::dpi::Size::Logical(wry::dpi::LogicalSize::new(width as f64, height as f64)),
        })
        .with_devtools(false)
        .build_as_child(&wrapper)
    {
        Ok(w) => {
            elog!("[HardwaveWettBoi] WKWebView created");
            w
        }
        Err(e) => {
            elog!("[HardwaveWettBoi] WebView creation FAILED (macOS): {e}");
            return Box::new(EditorHandle {
                running,
                _webview: None,
                _web_context: None,
                _server_thread: None,
                _editor_thread: None,
            });
        }
    };

    // Everything below only ever runs on the main queue, so the raw pointer is
    // never shared across threads. It is freed by the tick that sees `running`
    // go false, which Drop sets.
    struct MacPump {
        webview: wry::WebView,
        _web_context: wry::WebContext,
        packet_rx: Arc<Mutex<Receiver<WbPacket>>>,
        resize_rx: Receiver<(u32, u32)>,
        running: Arc<AtomicBool>,
    }

    let pump = Box::into_raw(Box::new(MacPump {
        webview,
        _web_context: web_context,
        packet_rx,
        resize_rx,
        running: Arc::clone(&running),
    })) as usize;

    fn tick(ptr: usize) {
        dispatch::Queue::main().exec_after(std::time::Duration::from_millis(16), move || {
            // SAFETY: only ever dereferenced on the main queue, and the pointer
            // stays valid until the tick that frees it — after which no further
            // tick is scheduled.
            let p = ptr as *mut MacPump;
            let st = unsafe { &mut *p };

            if !st.running.load(Ordering::Relaxed) {
                elog!("[HardwaveWettBoi] Editor closed, releasing WebView");
                unsafe { drop(Box::from_raw(p)) };
                return;
            }

            while let Ok((w, h)) = st.resize_rx.try_recv() {
                let _ = st.webview.set_bounds(wry::Rect {
                    position: wry::dpi::Position::Logical(wry::dpi::LogicalPosition::new(0.0, 0.0)),
                    size: wry::dpi::Size::Logical(wry::dpi::LogicalSize::new(w as f64, h as f64)),
                });
            }

            if let Some(rx) = st.packet_rx.try_lock() {
                while let Ok(pkt) = rx.try_recv() {
                    if let Ok(json) = serde_json::to_string(&pkt) {
                        let _ = st.webview.evaluate_script(&format!(
                            "window.__onWbPacket && window.__onWbPacket({})",
                            json
                        ));
                    }
                }
            }

            tick(ptr);
        });
    }

    elog!("[HardwaveWettBoi] Entering editor pump on the main queue");
    tick(pump);

    Box::new(EditorHandle {
        running,
        _webview: None,
        _web_context: None,
        _server_thread: None,
        _editor_thread: None,
    })
}

// ─── Editor handle ─────────────────────────────────────────────────────────

struct EditorHandle {
    running: Arc<AtomicBool>,
    _webview: Option<wry::WebView>,
    _web_context: Option<wry::WebContext>,
    _server_thread: Option<std::thread::JoinHandle<()>>,
    _editor_thread: Option<std::thread::JoinHandle<()>>,
}

unsafe impl Send for EditorHandle {}

impl Drop for EditorHandle {
    fn drop(&mut self) {
        elog!("[HardwaveWettBoi] EditorHandle::drop — shutting down editor");
        self.running.store(false, Ordering::Relaxed);
    }
}

#[cfg(all(test, target_os = "windows"))]
mod pin_tests {
    use super::*;

    /// The crash this guards against only happens on the second load of the module, which no test
    /// here can stage. What a test can hold in place is that the guard is still asked for and that
    /// the loader accepts it: if `GetModuleHandleExW` ever starts refusing, this fails instead of a
    /// producer's DAW failing.
    #[test]
    fn pin_is_wired() {
        assert!(
            pin_own_module(),
            "the module must pin itself before any window is created"
        );
        assert!(
            pin_own_module(),
            "asking twice must stay true, it is a one-time pin"
        );
    }
}

#[cfg(test)]
mod offline_tests {
    use super::*;

    #[test]
    fn only_a_definite_failure_takes_the_interface_away() {
        use ureq::ErrorKind::{ConnectionFailed, Dns, Io, ProxyConnect};

        // Answers: nothing is there for this machine.
        assert!(offline_is_certain(
            ConnectionFailed,
            "Connection Failed: Connect error: Connection refused (os error 111)"
        ));
        assert!(offline_is_certain(
            Dns,
            "Dns Failed: resolve dns name 'example.com:443': failed to lookup address information"
        ));
        assert!(offline_is_certain(
            ConnectionFailed,
            "Connection Failed: Connect error: Network is unreachable (os error 101)"
        ));
        assert!(offline_is_certain(
            ConnectionFailed,
            "Connection Failed: Connect error: No route to host (os error 113)"
        ));

        // A slow name server is not a missing name: our own deadline runs through the resolver,
        // so a timeout arrives here wearing the DNS label.
        assert!(!offline_is_certain(
            Dns,
            "Dns Failed: resolve dns name 'hardwavestudios.com:443': timed out"
        ));

        // Not answers: the WebView may still get the page.
        assert!(!offline_is_certain(
            Io,
            "Network Error: timed out reading response"
        ));
        assert!(!offline_is_certain(
            ConnectionFailed,
            "Connection Failed: Connect error: Connection timed out (os error 110)"
        ));
        assert!(!offline_is_certain(
            ConnectionFailed,
            "Connection Failed: tls connection init failed: invalid peer certificate: UnknownIssuer"
        ));
        assert!(!offline_is_certain(
            ProxyConnect,
            "Proxy failed to connect: 407 Proxy Authentication Required"
        ));
        // Words that only mean something in a lookup or a connect mean nothing elsewhere.
        assert!(!offline_is_certain(Io, "Network Error: dns refused"));
    }

    #[test]
    fn the_probe_leaves_the_query_behind() {
        assert_eq!(
            without_query("https://example.com/vst/thing?token=abc&v=1"),
            "https://example.com/vst/thing"
        );
        assert_eq!(
            without_query("https://example.com/vst/thing#x"),
            "https://example.com/vst/thing"
        );
        assert_eq!(
            without_query("https://example.com/vst/thing"),
            "https://example.com/vst/thing"
        );
    }

    /// A probe that fails must not put the token in the reason that goes to the log file.
    #[test]
    fn a_failed_probe_does_not_log_the_token() {
        // Bind and drop at once, so the port is almost certainly closed: a refused connect.
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .and_then(|l| l.local_addr())
            .map(|a| a.port())
            .expect("a free local port");
        let url = format!("http://127.0.0.1:{port}/vst/wettboi?token=SECRETTOKEN&v=1");

        // Whether this ends as "refused" or as a connect timeout depends on the platform (Windows
        // retries a refused connect for longer than the probe waits), so only the reason is checked.
        let (_, reason) = probe_interface(&url);
        let reason = reason.expect("nothing listens there, so the probe must fail");
        assert!(
            !reason.contains("SECRETTOKEN"),
            "token in the log: {reason}"
        );
        assert!(!reason.contains("token="), "query in the log: {reason}");
    }

    /// The verdict must not depend on what the token spells. A timeout with a token that contains
    /// "refused" used to count as a refused connection and replaced the interface with the offline
    /// page for that user alone.
    #[test]
    fn the_token_cannot_change_the_verdict() {
        // Accepts the connection at the kernel level and never answers: the probe times out.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a local port");
        let port = listener.local_addr().expect("its address").port();
        let url = format!("http://127.0.0.1:{port}/vst/wettboi?token=xDNSrefusedUnreachable");

        let (reachable, reason) = probe_interface(&url);
        drop(listener);
        assert!(reachable, "a timeout must load the interface: {reason:?}");
    }

    /// The page shown when the interface cannot be reached must be able to stand on its own: no
    /// script, no stylesheet, no image, nothing that needs the connection that has just failed.
    #[test]
    fn offline_page_needs_nothing_from_the_network() {
        let html = offline_page("https://example.com/vst/thing?token=abc");
        assert!(
            !html.contains("<script"),
            "the offline page must not run script"
        );
        assert!(
            !html.contains("src="),
            "the offline page must not fetch anything"
        );
        assert!(
            html.contains("https://example.com/vst/thing?token=abc"),
            "trying again has to go back to the interface"
        );
        assert!(
            html.contains("support@hardwavestudios.com"),
            "say where to write when it keeps failing"
        );
    }
}

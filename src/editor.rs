//! WebView-based editor for Hardwave WettBoi.
//!
//! Uses the same hwpacket bridge pattern as LoudLab/KickForge:
//! - Linux/macOS: Rust pushes state via `evaluate_script()`.
//! - Windows: Rust starts a local TCP server, JS polls via `fetch()`.

use crossbeam_channel::{Receiver, Sender, unbounded};
use nih_plug::editor::Editor;
use nih_plug::prelude::{GuiContext, ParentWindowHandle, Param};
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
    fn window_handle(&self) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
        use raw_window_handle::RawWindowHandle;

        #[cfg(target_os = "linux")]
        let raw = {
            let h = raw_window_handle::XlibWindowHandle::new(self.0 as _);
            RawWindowHandle::Xlib(h)
        };

        #[cfg(target_os = "macos")]
        let raw = {
            let ns_view = std::ptr::NonNull::new(self.0 as *mut _).expect("null NSView");
            let h = raw_window_handle::AppKitWindowHandle::new(ns_view);
            RawWindowHandle::AppKit(h)
        };

        #[cfg(target_os = "windows")]
        let raw = {
            let hwnd = std::num::NonZeroIsize::new(self.0 as isize).expect("null HWND");
            let h = raw_window_handle::Win32WindowHandle::new(hwnd);
            RawWindowHandle::Win32(h)
        };

        Ok(unsafe { raw_window_handle::WindowHandle::borrow_raw(raw) })
    }
}

impl raw_window_handle::HasDisplayHandle for RwhWrapper {
    fn display_handle(&self) -> Result<raw_window_handle::DisplayHandle<'_>, raw_window_handle::HandleError> {
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
    eprintln!("[HardwaveWettBoi] Building param map...");
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

    eprintln!("[HardwaveWettBoi] Param map built: {} entries", map.len());
    map
}

/// Create a snapshot of the current DAW params as a `WbPacket`.
pub fn snapshot_params(params: &WettBoiParams, bpm: f32, duck_depth: f32, lfo_value: f32) -> WbPacket {
    use crate::params::{ReverbType, ScSource, LfoShape, LfoTarget, NoteDiv, RoutingMode};

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

    let note_to_str = |n: NoteDiv| -> &'static str {
        match n {
            NoteDiv::Sixteenth => "1/16",
            NoteDiv::Eighth => "1/8",
            NoteDiv::DottedEighth => "d1/8",
            NoteDiv::Quarter => "1/4",
            NoteDiv::DottedQuarter => "d1/4",
            NoteDiv::Half => "1/2",
            NoteDiv::DottedHalf => "d1/2",
            NoteDiv::Whole => "1/1",
        }
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
fn string_to_param_value(param_id: &str, s: &str) -> Option<f32> {
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
            eprintln!("[HardwaveWettBoi] IPC parse error: {} — raw: {}", e, &raw_body[..raw_body.len().min(200)]);
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
                eprintln!("[HardwaveWettBoi] IPC set_param '{}': could not parse value {:?}", id, raw_value);
            } else {
                eprintln!("[HardwaveWettBoi] IPC set_param: unknown param id '{}'", id);
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
            eprintln!("[HardwaveWettBoi] IPC resize: {}x{}", w, h);
            if w >= MIN_WIDTH && w <= MAX_WIDTH && h >= MIN_HEIGHT && h <= MAX_HEIGHT {
                *editor_size.lock() = (w, h);
                if context.request_resize() {
                    if let Some(tx) = resize_tx.lock().as_ref() {
                        let _ = tx.send((w, h));
                    }
                }
            } else {
                eprintln!("[HardwaveWettBoi] IPC resize: out of bounds ({}x{} not in {}x{}–{}x{})", w, h, MIN_WIDTH, MIN_HEIGHT, MAX_WIDTH, MAX_HEIGHT);
            }
        }
        "save_token" => {
            eprintln!("[HardwaveWettBoi] IPC save_token: persisting to disk");
            if let Some(token) = msg.get("token").and_then(|v| v.as_str()) {
                match auth::save_token(token) {
                    Ok(()) => eprintln!("[HardwaveWettBoi] Token saved successfully"),
                    Err(e) => eprintln!("[HardwaveWettBoi] Token save FAILED: {}", e),
                }
            }
        }
        "clear_token" => {
            eprintln!("[HardwaveWettBoi] IPC clear_token: removing from disk");
            match auth::clear_token() {
                Ok(()) => eprintln!("[HardwaveWettBoi] Token cleared"),
                Err(e) => eprintln!("[HardwaveWettBoi] Token clear FAILED: {}", e),
            }
        }
        other => {
            eprintln!("[HardwaveWettBoi] IPC unknown message type: '{}'", other);
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
        eprintln!("[HardwaveWettBoi] Editor::spawn — scale_factor={:.2}, auth_token={}", scale, if self.auth_token.is_some() { "present" } else { "none" });
        let packet_rx = Arc::clone(&self.packet_rx);
        let (width, height) = self.scaled_size();
        eprintln!("[HardwaveWettBoi] Editor size: {}x{} (scaled)", width, height);

        let version = env!("CARGO_PKG_VERSION");
        let url = match &self.auth_token {
            Some(t) => format!("{}?token={}&v={}", WETTBOI_URL, t, version),
            None => format!("{}?v={}", WETTBOI_URL, version),
        };
        eprintln!("[HardwaveWettBoi] Loading URL: {} (token {})", WETTBOI_URL, if self.auth_token.is_some() { "injected" } else { "absent" });

        let param_map = Arc::new(build_param_map(&self.params));
        let init_js = ipc_init_script(&self.params, 150.0);
        eprintln!("[HardwaveWettBoi] Init script: {} bytes", init_js.len());
        let raw_handle = extract_raw_handle(&parent);
        eprintln!("[HardwaveWettBoi] Parent window handle: 0x{:x}", raw_handle);

        let (resize_tx_val, resize_rx) = unbounded::<(u32, u32)>();
        *self.resize_tx.lock() = Some(resize_tx_val);

        let editor_size = Arc::clone(&self.editor_size);
        let resize_tx = Arc::clone(&self.resize_tx);

        #[cfg(target_os = "windows")]
        {
            eprintln!("[HardwaveWettBoi] Platform: Windows — using TCP polling bridge");
            spawn_windows(raw_handle, url, width, height, packet_rx, context, param_map, init_js, resize_rx, editor_size, resize_tx)
        }

        #[cfg(not(target_os = "windows"))]
        {
            eprintln!("[HardwaveWettBoi] Platform: Unix — using evaluate_script bridge");
            spawn_unix(raw_handle, url, width, height, packet_rx, context, param_map, init_js, resize_rx, editor_size, resize_tx)
        }
    }

    fn size(&self) -> (u32, u32) {
        self.scaled_size()
    }

    fn set_scale_factor(&self, factor: f32) -> bool {
        *self.scale_factor.lock() = factor;
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
            eprintln!("[HardwaveWettBoi] failed to bind TCP: {}", e);
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
    eprintln!("[HardwaveWettBoi] TCP server bound on 127.0.0.1:{}", port);
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
    let ctx = Arc::clone(&context);
    let pmap = Arc::clone(&param_map);
    let esize = Arc::clone(&editor_size);
    let rtx = Arc::clone(&resize_tx);

    let data_dir = webview2_data_dir();
    eprintln!("[HardwaveWettBoi] WebView2 data dir: {:?}", data_dir);
    let _ = std::fs::create_dir_all(&data_dir);
    let mut web_context = wry::WebContext::new(Some(data_dir));

    let wrapper = RwhWrapper(raw_handle);

    eprintln!("[HardwaveWettBoi] Creating WebView2 (Windows) {}x{} ...", width, height);
    use wry::WebViewBuilderExtWindows;
    let webview = wry::WebViewBuilder::with_web_context(&mut web_context)
        .with_url(&url)
        .with_initialization_script(&init_js)
        .with_ipc_handler(move |msg| {
            handle_ipc(&ctx, &pmap, &msg.body(), raw_handle, &esize, &rtx);
        })
        .with_bounds(wry::Rect {
            position: wry::dpi::Position::Logical(wry::dpi::LogicalPosition::new(0.0, 0.0)),
            size: wry::dpi::Size::Logical(wry::dpi::LogicalSize::new(width as f64, height as f64)),
        })
        .with_transparent(false)
        .with_devtools(false)
        .with_background_color((10, 10, 11, 255))
        .build(&wrapper);

    let webview = match webview {
        Ok(wv) => {
            eprintln!("[HardwaveWettBoi] WebView created successfully");
            Some(wv)
        }
        Err(e) => {
            eprintln!("[HardwaveWettBoi] WebView creation FAILED: {}", e);
            None
        }
    };

    Box::new(EditorHandle {
        running: running_clone,
        _webview: webview,
        _web_context: Some(web_context),
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
            eprintln!("[HardwaveWettBoi] Initialising GTK...");
            let _ = gtk::init();
            eprintln!("[HardwaveWettBoi] GTK initialised");
        }

        let wrapper = RwhWrapper(raw_handle);
        let ctx = Arc::clone(&context);
        let pmap = Arc::clone(&param_map);
        let esize = Arc::clone(&editor_size);
        let rtx = Arc::clone(&resize_tx);

        let data_dir = webview_data_dir();
        eprintln!("[HardwaveWettBoi] WebView data dir: {:?}", data_dir);
        let _ = std::fs::create_dir_all(&data_dir);
        let mut web_context = wry::WebContext::new(Some(data_dir));

        eprintln!("[HardwaveWettBoi] Creating WebKitGTK/WebKit WebView {}x{} ...", width, height);
        let webview = match wry::WebViewBuilder::with_web_context(&mut web_context)
            .with_url(&url)
            .with_initialization_script(&init_js)
            .with_ipc_handler(move |msg| {
                handle_ipc(&ctx, &pmap, &msg.body(), raw_handle, &esize, &rtx);
            })
            .with_bounds(wry::Rect {
                position: wry::dpi::Position::Logical(wry::dpi::LogicalPosition::new(0.0, 0.0)),
                size: wry::dpi::Size::Logical(wry::dpi::LogicalSize::new(width as f64, height as f64)),
            })
            .build_as_child(&wrapper)
        {
            Ok(wv) => {
                eprintln!("[HardwaveWettBoi] WebView created successfully (Unix)");
                wv
            }
            Err(e) => {
                eprintln!("[HardwaveWettBoi] WebView creation FAILED (Unix): {}", e);
                return;
            }
        };

        eprintln!("[HardwaveWettBoi] Entering editor event loop");
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
                        let js = format!(
                            "window.__onWbPacket && window.__onWbPacket({})",
                            json
                        );
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
        eprintln!("[HardwaveWettBoi] EditorHandle::drop — shutting down editor");
        self.running.store(false, Ordering::Relaxed);
    }
}

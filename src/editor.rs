//! WebView-based editor for the WettBoi VST plugin.
//!
//! Embeds a wry WebView that loads `wettboi.hardwavestudios.com/vst/wettboi`.
//!
//! Communication:
//! - **Plugin → WebView**: param state pushed via `evaluate_script()` (Linux/macOS)
//!   or via a local TCP HTTP server polled by JS (Windows).
//! - **WebView → Plugin**: `window.__hardwave.setParam(key, value)` calls
//!   `window.ipc.postMessage()` → GuiContext sets the nih-plug param.

use crossbeam_channel::Receiver;
use nih_plug::editor::Editor;
use nih_plug::prelude::{GuiContext, ParentWindowHandle, Param};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::auth;
use crate::params::WettBoiParams;
use crate::protocol::WettBoiPacket;

const WETTBOI_URL: &str = "https://wettboi.hardwavestudios.com/vst/wettboi";
const EDITOR_WIDTH: u32 = 1100;
const EDITOR_HEIGHT: u32 = 700;

/// Wraps a raw window handle value (usize) so wry can use it via rwh 0.6.
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

/// Build a map of camelCase param keys to ParamPtr for GuiContext param setting.
fn build_param_map(params: &WettBoiParams) -> HashMap<String, nih_plug::prelude::ParamPtr> {
    let mut map = HashMap::new();
    map.insert("enabled".into(), params.enabled.as_ptr());
    map.insert("wet".into(), params.wet.as_ptr());
    map.insert("hiPassEnabled".into(), params.hi_pass_enabled.as_ptr());
    map.insert("hiPassFreq".into(), params.hi_pass_freq.as_ptr());
    map.insert("loPassEnabled".into(), params.lo_pass_enabled.as_ptr());
    map.insert("loPassFreq".into(), params.lo_pass_freq.as_ptr());
    map.insert("attack".into(), params.attack.as_ptr());
    map.insert("decay".into(), params.decay.as_ptr());
    map.insert("sustain".into(), params.sustain.as_ptr());
    map.insert("release".into(), params.release.as_ptr());
    map.insert("delayEnabled".into(), params.delay_enabled.as_ptr());
    map.insert("delayTime".into(), params.delay_time.as_ptr());
    map.insert("delayFeedback".into(), params.delay_feedback.as_ptr());
    map.insert("delayMix".into(), params.delay_mix.as_ptr());
    map.insert("reverbEnabled".into(), params.reverb_enabled.as_ptr());
    map.insert("reverbSize".into(), params.reverb_size.as_ptr());
    map.insert("reverbDamping".into(), params.reverb_damping.as_ptr());
    map.insert("reverbMix".into(), params.reverb_mix.as_ptr());
    map.insert("fxOrder".into(), params.fx_order.as_ptr());
    map
}

pub struct WettBoiEditor {
    params: Arc<WettBoiParams>,
    packet_rx: Arc<Mutex<Receiver<WettBoiPacket>>>,
    auth_token: Option<String>,
    size: (u32, u32),
}

impl WettBoiEditor {
    pub fn new(
        params: Arc<WettBoiParams>,
        packet_rx: Arc<Mutex<Receiver<WettBoiPacket>>>,
        auth_token: Option<String>,
    ) -> Self {
        Self {
            params,
            packet_rx,
            auth_token,
            size: (EDITOR_WIDTH, EDITOR_HEIGHT),
        }
    }
}

impl Editor for WettBoiEditor {
    fn spawn(
        &self,
        parent: ParentWindowHandle,
        context: Arc<dyn GuiContext>,
    ) -> Box<dyn std::any::Any + Send> {
        let packet_rx = Arc::clone(&self.packet_rx);
        let (width, height) = self.size;

        // Build the URL with token and version
        let version = env!("CARGO_PKG_VERSION");
        let url = match &self.auth_token {
            Some(t) => format!("{}?token={}&v={}", WETTBOI_URL, t, version),
            None => format!("{}?v={}", WETTBOI_URL, version),
        };

        // Build param map for IPC handler
        let param_map = Arc::new(build_param_map(&self.params));

        // Extract raw handle value BEFORE spawning threads (ParentWindowHandle isn't Send)
        let raw_handle = extract_raw_handle(&parent);

        #[cfg(target_os = "windows")]
        {
            spawn_windows(raw_handle, url, width, height, packet_rx, context, param_map)
        }

        #[cfg(not(target_os = "windows"))]
        {
            spawn_unix(raw_handle, url, width, height, packet_rx, context, param_map)
        }
    }

    fn size(&self) -> (u32, u32) {
        self.size
    }

    fn set_scale_factor(&self, _factor: f32) -> bool {
        false
    }

    fn param_value_changed(&self, _id: &str, _normalized_value: f32) {}
    fn param_modulation_changed(&self, _id: &str, _modulation_offset: f32) {}
    fn param_values_changed(&self) {}
}

/// Extract the raw handle value from ParentWindowHandle so we can send it across threads.
fn extract_raw_handle(parent: &ParentWindowHandle) -> usize {
    match *parent {
        #[cfg(target_os = "linux")]
        ParentWindowHandle::X11Window(id) => id as usize,
        #[cfg(target_os = "macos")]
        ParentWindowHandle::AppKitNsView(ptr) => ptr as usize,
        #[cfg(target_os = "windows")]
        ParentWindowHandle::Win32Hwnd(h) => h as usize,
        _ => 0, // Fallback — editor will fail gracefully
    }
}

/// IPC init script injected into the WebView.
fn ipc_init_script() -> String {
    format!(
        r#"
    window.__HARDWAVE_VST = true;
    window.__HARDWAVE_VST_VERSION = '{}';
    window.__hardwave = {{
        setParam: function(key, value) {{
            var v = value;
            if (typeof v === 'boolean') v = v ? 1 : 0;
            if (key === 'fxOrder') v = (v === 'delay-reverb') ? 0 : 1;
            window.ipc.postMessage('setParam:' + key + ':' + v);
        }},
        saveToken: function(token) {{
            window.ipc.postMessage('saveToken:' + token);
        }}
    }};
    "#,
        env!("CARGO_PKG_VERSION")
    )
}

/// Handle IPC messages from the WebView. Uses GuiContext to properly set nih-plug params.
fn handle_ipc(
    context: &Arc<dyn GuiContext>,
    param_map: &Arc<HashMap<String, nih_plug::prelude::ParamPtr>>,
    message: &str,
) {
    if let Some(rest) = message.strip_prefix("setParam:") {
        if let Some((key, val_str)) = rest.split_once(':') {
            if let Ok(value) = val_str.parse::<f64>() {
                if let Some(&param_ptr) = param_map.get(key) {
                    // SAFETY: param_ptr is valid for the lifetime of the plugin.
                    // We hold Arc<WettBoiParams> which keeps the params alive.
                    unsafe {
                        let normalized = param_ptr.preview_normalized(value as f32);
                        context.raw_begin_set_parameter(param_ptr);
                        context.raw_set_parameter_normalized(param_ptr, normalized);
                        context.raw_end_set_parameter(param_ptr);
                    }
                }
            }
        }
    } else if let Some(token) = message.strip_prefix("saveToken:") {
        auth::save_token(token.trim());
    }
}

// ─── Windows: TCP packet server approach ────────────────────────────────────

#[cfg(target_os = "windows")]
fn webview2_data_dir() -> std::path::PathBuf {
    dirs::data_local_dir()
        .map(|d| d.join("Hardwave").join("WettBoi").join("WebView2"))
        .unwrap_or_else(|| std::path::PathBuf::from("C:\\HardwaveWebView2Data"))
}

#[cfg(target_os = "windows")]
fn spawn_windows(
    raw_handle: usize,
    url: String,
    width: u32,
    height: u32,
    packet_rx: Arc<Mutex<Receiver<WettBoiPacket>>>,
    context: Arc<dyn GuiContext>,
    param_map: Arc<HashMap<String, nih_plug::prelude::ParamPtr>>,
) -> Box<dyn std::any::Any + Send> {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    let running = Arc::new(AtomicBool::new(true));
    let running_clone = Arc::clone(&running);

    let (port_tx, port_rx) = crossbeam_channel::bounded::<u16>(1);
    let packet_rx_server = Arc::clone(&packet_rx);
    let running_server = Arc::clone(&running);

    let server_thread = std::thread::spawn(move || {
        let listener = match TcpListener::bind("127.0.0.1:0") {
            Ok(l) => l,
            Err(_) => return,
        };
        let port = match listener.local_addr() {
            Ok(addr) => addr.port(),
            Err(_) => return,
        };
        let _ = port_tx.send(port);
        listener.set_nonblocking(true).ok();

        let mut latest_json = String::from("null");

        while running_server.load(Ordering::Relaxed) {
            if let Some(rx) = packet_rx_server.try_lock() {
                while let Ok(pkt) = rx.try_recv() {
                    if let Ok(json) = serde_json::to_string(&pkt) {
                        latest_json = json;
                    }
                }
            }

            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf);
                let response = format!(
                    "HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nContent-Type: application/json\r\nCache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    latest_json.len(),
                    latest_json
                );
                let _ = stream.write_all(response.as_bytes());
            }

            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    });

    let port = match port_rx.recv() {
        Ok(p) => p,
        Err(_) => {
            return Box::new(EditorHandle {
                running: running_clone,
                _webview: None,
                _web_context: None,
                _server_thread: Some(server_thread),
                _editor_thread: None,
            });
        }
    };

    let wrapper = RwhWrapper(raw_handle);

    let poll_script = format!(
        r#"
        (function() {{
            const POLL_URL = 'http://127.0.0.1:{}/';
            function poll() {{
                fetch(POLL_URL).then(r => r.json()).then(data => {{
                    if (window.__onWettBoiPacket) window.__onWettBoiPacket(data);
                }}).catch(() => {{}});
                requestAnimationFrame(poll);
            }}
            poll();
        }})();
        "#,
        port
    );

    let init_js = format!("{}\n{}", ipc_init_script(), poll_script);
    let ctx = Arc::clone(&context);
    let pmap = Arc::clone(&param_map);

    // Create a writable WebView2 data directory to avoid E_ACCESSDENIED
    // when the DAW is installed in Program Files (read-only).
    let data_dir = webview2_data_dir();
    let _ = std::fs::create_dir_all(&data_dir);
    let mut web_context = wry::WebContext::new(Some(data_dir));

    // with_web_context is a constructor (replaces ::new())
    use wry::WebViewBuilderExtWindows;
    let webview = match wry::WebViewBuilder::with_web_context(&mut web_context)
        .with_url(&url)
        .with_initialization_script(&init_js)
        .with_ipc_handler(move |msg| {
            handle_ipc(&ctx, &pmap, &msg.body());
        })
        .with_bounds(wry::Rect {
            position: wry::dpi::Position::Logical(wry::dpi::LogicalPosition::new(0.0, 0.0)),
            size: wry::dpi::Size::Logical(wry::dpi::LogicalSize::new(width as f64, height as f64)),
        })
        .with_transparent(false)
        .with_devtools(false)
        .with_background_color((10, 10, 11, 255))
        .with_additional_browser_args("--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection --allow-insecure-localhost")
        .build(&wrapper)
    {
        Ok(wv) => wv,
        Err(e) => {
            eprintln!("[WettBoi] failed to create WebView: {}", e);
            return Box::new(EditorHandle {
                running: running_clone,
                _webview: None,
                _web_context: Some(web_context),
                _server_thread: Some(server_thread),
                _editor_thread: None,
            });
        }
    };

    Box::new(EditorHandle {
        running: running_clone,
        _webview: Some(webview),
        _web_context: Some(web_context),
        _server_thread: Some(server_thread),
        _editor_thread: None,
    })
}

// ─── Linux / macOS: evaluate_script approach ────────────────────────────────

#[cfg(not(target_os = "windows"))]
fn spawn_unix(
    raw_handle: usize,
    url: String,
    width: u32,
    height: u32,
    packet_rx: Arc<Mutex<Receiver<WettBoiPacket>>>,
    context: Arc<dyn GuiContext>,
    param_map: Arc<HashMap<String, nih_plug::prelude::ParamPtr>>,
) -> Box<dyn std::any::Any + Send> {
    let running = Arc::new(AtomicBool::new(true));
    let running_clone = Arc::clone(&running);

    let editor_thread = std::thread::spawn(move || {
        #[cfg(target_os = "linux")]
        {
            let _ = gtk::init();
        }

        let wrapper = RwhWrapper(raw_handle);
        let ctx = Arc::clone(&context);
        let pmap = Arc::clone(&param_map);

        let webview = match wry::WebViewBuilder::new()
            .with_url(&url)
            .with_initialization_script(&ipc_init_script())
            .with_ipc_handler(move |msg| {
                handle_ipc(&ctx, &pmap, &msg.body());
            })
            .with_bounds(wry::Rect {
                position: wry::dpi::Position::Logical(wry::dpi::LogicalPosition::new(0.0, 0.0)),
                size: wry::dpi::Size::Logical(wry::dpi::LogicalSize::new(width as f64, height as f64)),
            })
            .build_as_child(&wrapper)
        {
            Ok(wv) => wv,
            Err(e) => {
                eprintln!("[WettBoi] failed to create WebView: {}", e);
                return;
            }
        };

        while running.load(Ordering::Relaxed) {
            if let Some(rx) = packet_rx.try_lock() {
                while let Ok(pkt) = rx.try_recv() {
                    if let Ok(json) = serde_json::to_string(&pkt) {
                        let js = format!(
                            "window.__onWettBoiPacket && window.__onWettBoiPacket({})",
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
        self.running.store(false, Ordering::Relaxed);
        if let Some(handle) = self._server_thread.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self._editor_thread.take() {
            let _ = handle.join();
        }
    }
}

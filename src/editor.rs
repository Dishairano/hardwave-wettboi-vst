//! WebView-based editor for the WettBoi VST plugin.
//!
//! Embeds a wry WebView that loads `wettboi.hardwavestudios.com/vst/wettboi`.
//!
//! Communication:
//! - **Plugin → WebView**: param state pushed via `evaluate_script()` (Linux/macOS)
//!   or via a local TCP HTTP server polled by JS (Windows).
//! - **WebView → Plugin**: `window.__hardwave.setParam(key, value)` calls
//!   `window.ipc.postMessage()` which routes back through `param_change_tx`.

use crossbeam_channel::{Receiver, Sender};
use nih_plug::editor::Editor;
use nih_plug::prelude::ParentWindowHandle;
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::auth;
use crate::protocol::{ParamChange, WettBoiPacket};

const WETTBOI_URL: &str = "https://wettboi.hardwavestudios.com/vst/wettboi";
const EDITOR_WIDTH: u32 = 1100;
const EDITOR_HEIGHT: u32 = 700;

/// Wraps a raw-window-handle 0.5 (from nih-plug) so wry can use it via
/// raw-window-handle 0.6.
///
/// On **Linux** the inner value is an Xlib window ID.
/// On **macOS** it's an `NSView*`.
/// On **Windows** it's an `HWND`.
struct RwhWrapper(usize);

// SAFETY: the pointer/ID outlives the WebView.
unsafe impl Send for RwhWrapper {}
unsafe impl Sync for RwhWrapper {}

impl raw_window_handle::HasWindowHandle for RwhWrapper {
    fn window_handle(&self) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
        use raw_window_handle::RawWindowHandle;

        #[cfg(target_os = "linux")]
        let raw = {
            let mut h = raw_window_handle::XlibWindowHandle::new(self.0 as _);
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
            let hwnd = std::ptr::NonNull::new(self.0 as *mut _).expect("null HWND");
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

pub struct WettBoiEditor {
    packet_rx: Arc<Mutex<Receiver<WettBoiPacket>>>,
    param_change_tx: Arc<Mutex<Sender<ParamChange>>>,
    auth_token: Option<String>,
    size: (u32, u32),
}

impl WettBoiEditor {
    pub fn new(
        packet_rx: Arc<Mutex<Receiver<WettBoiPacket>>>,
        param_change_tx: Arc<Mutex<Sender<ParamChange>>>,
        auth_token: Option<String>,
    ) -> Self {
        Self {
            packet_rx,
            param_change_tx,
            auth_token,
            size: (EDITOR_WIDTH, EDITOR_HEIGHT),
        }
    }
}

impl Editor for WettBoiEditor {
    fn spawn(
        &self,
        parent: ParentWindowHandle,
        _context: Arc<dyn nih_plug::prelude::GuiContext>,
    ) -> Box<dyn std::any::Any + Send> {
        let packet_rx = Arc::clone(&self.packet_rx);
        let param_change_tx = Arc::clone(&self.param_change_tx);
        let token = self.auth_token.clone();
        let (width, height) = self.size;

        // Build the URL with token if available
        let url = match &token {
            Some(t) => format!("{}?token={}", WETTBOI_URL, t),
            None => WETTBOI_URL.to_string(),
        };

        #[cfg(target_os = "windows")]
        {
            spawn_windows(parent, url, width, height, packet_rx, param_change_tx)
        }

        #[cfg(not(target_os = "windows"))]
        {
            spawn_unix(parent, url, width, height, packet_rx, param_change_tx)
        }
    }

    fn size(&self) -> (u32, u32) {
        self.size
    }

    fn set_scale_factor(&self, _factor: f32) -> bool {
        false
    }

    fn param_value_changed(&self, _id: &str, _normalized_value: f32) {
        // We push full state packets at ~30Hz instead of individual changes
    }

    fn param_modulation_changed(&self, _id: &str, _modulation_offset: f32) {}
    fn param_values_changed(&self) {}
}

/// IPC init script injected into the WebView.
/// Provides `window.__hardwave.setParam(key, value)` for the UI to call.
fn ipc_init_script() -> String {
    r#"
    window.__HARDWAVE_VST = true;
    window.__hardwave = {
        setParam: function(key, value) {
            window.ipc.postMessage('setParam:' + key + ':' + value);
        },
        saveToken: function(token) {
            window.ipc.postMessage('saveToken:' + token);
        }
    };
    "#.to_string()
}

// ─── Windows: TCP packet server approach ────────────────────────────────────

#[cfg(target_os = "windows")]
fn spawn_windows(
    parent: ParentWindowHandle,
    url: String,
    width: u32,
    height: u32,
    packet_rx: Arc<Mutex<Receiver<WettBoiPacket>>>,
    param_change_tx: Arc<Mutex<Sender<ParamChange>>>,
) -> Box<dyn std::any::Any + Send> {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    let running = Arc::new(AtomicBool::new(true));
    let running_clone = Arc::clone(&running);

    // Start a local HTTP server that serves the latest param state as JSON
    let (port_tx, port_rx) = crossbeam_channel::bounded::<u16>(1);
    let packet_rx_server = Arc::clone(&packet_rx);
    let running_server = Arc::clone(&running);

    let server_thread = std::thread::spawn(move || {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind packet server");
        let port = listener.local_addr().unwrap().port();
        let _ = port_tx.send(port);
        listener.set_nonblocking(true).ok();

        let mut latest_json = String::from("{}");

        while running_server.load(Ordering::Relaxed) {
            // Update latest packet
            if let Ok(rx) = packet_rx_server.try_lock() {
                while let Ok(pkt) = rx.try_recv() {
                    if let Ok(json) = serde_json::to_string(&pkt) {
                        latest_json = json;
                    }
                }
            }

            // Accept connections
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf);
                let response = format!(
                    "HTTP/1.1 200 OK\r\n\
                     Access-Control-Allow-Origin: *\r\n\
                     Content-Type: application/json\r\n\
                     Content-Length: {}\r\n\
                     Connection: close\r\n\
                     \r\n\
                     {}",
                    latest_json.len(),
                    latest_json
                );
                let _ = stream.write_all(response.as_bytes());
            }

            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    });

    let port = port_rx.recv().expect("receive packet server port");

    // Extract raw HWND from nih-plug's ParentWindowHandle
    let hwnd = match parent {
        ParentWindowHandle::Win32Hwnd(h) => h as usize,
        _ => panic!("expected Win32 HWND"),
    };
    let wrapper = RwhWrapper(hwnd);

    // JS polling script — polls the local HTTP server at ~60fps
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
    let param_tx = Arc::clone(&param_change_tx);

    let webview = wry::WebViewBuilder::new()
        .with_url(&url)
        .with_initialization_script(&init_js)
        .with_ipc_handler(move |msg| {
            handle_ipc(&param_tx, &msg.body());
        })
        .with_bounds(wry::Rect {
            position: wry::dpi::Position::Logical(wry::dpi::LogicalPosition::new(0.0, 0.0)),
            size: wry::dpi::Size::Logical(wry::dpi::LogicalSize::new(
                width as f64,
                height as f64,
            )),
        })
        .build(&wrapper)
        .expect("build WebView");

    Box::new(EditorHandle {
        running: running_clone,
        _webview: Some(webview),
        _server_thread: Some(server_thread),
        _editor_thread: None,
    })
}

// ─── Linux / macOS: evaluate_script approach ────────────────────────────────

#[cfg(not(target_os = "windows"))]
fn spawn_unix(
    parent: ParentWindowHandle,
    url: String,
    width: u32,
    height: u32,
    packet_rx: Arc<Mutex<Receiver<WettBoiPacket>>>,
    param_change_tx: Arc<Mutex<Sender<ParamChange>>>,
) -> Box<dyn std::any::Any + Send> {
    let running = Arc::new(AtomicBool::new(true));
    let running_clone = Arc::clone(&running);

    let editor_thread = std::thread::spawn(move || {
        #[cfg(target_os = "linux")]
        {
            let _ = gtk::init();
        }

        // Extract raw handle
        let raw = match parent {
            #[cfg(target_os = "linux")]
            ParentWindowHandle::X11Window(id) => id as usize,
            #[cfg(target_os = "macos")]
            ParentWindowHandle::AppKitNsView(ptr) => ptr as usize,
            _ => {
                eprintln!("[WettBoi] unsupported parent window handle");
                return;
            }
        };

        let wrapper = RwhWrapper(raw);
        let param_tx = Arc::clone(&param_change_tx);

        let webview = match wry::WebViewBuilder::new()
            .with_url(&url)
            .with_initialization_script(&ipc_init_script())
            .with_ipc_handler(move |msg| {
                handle_ipc(&param_tx, &msg.body());
            })
            .with_bounds(wry::Rect {
                position: wry::dpi::Position::Logical(wry::dpi::LogicalPosition::new(0.0, 0.0)),
                size: wry::dpi::Size::Logical(wry::dpi::LogicalSize::new(
                    width as f64,
                    height as f64,
                )),
            })
            .build_as_child(&wrapper)
        {
            Ok(wv) => wv,
            Err(e) => {
                eprintln!("[WettBoi] failed to create WebView: {}", e);
                return;
            }
        };

        // Push param state via evaluate_script at ~60fps
        while running.load(Ordering::Relaxed) {
            if let Ok(rx) = packet_rx.try_lock() {
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
        _server_thread: None,
        _editor_thread: Some(editor_thread),
    })
}

/// Handle IPC messages from the WebView.
fn handle_ipc(param_tx: &Arc<Mutex<Sender<ParamChange>>>, message: &str) {
    if let Some(rest) = message.strip_prefix("setParam:") {
        // Format: "setParam:key:value"
        if let Some((key, val_str)) = rest.split_once(':') {
            if let Ok(value) = val_str.parse::<f64>() {
                if let Ok(tx) = param_tx.try_lock() {
                    let _ = tx.try_send(ParamChange {
                        key: key.to_string(),
                        value,
                    });
                }
            }
        }
    } else if let Some(token) = message.strip_prefix("saveToken:") {
        auth::save_token(token.trim());
    }
}

/// Drop handle — signals the editor to shut down when the DAW closes the plugin window.
struct EditorHandle {
    running: Arc<AtomicBool>,
    _webview: Option<wry::WebView>,
    _server_thread: Option<std::thread::JoinHandle<()>>,
    _editor_thread: Option<std::thread::JoinHandle<()>>,
}

// SAFETY: wry::WebView is created on the thread that uses it.
// The handle is only used for dropping (setting running=false).
unsafe impl Send for EditorHandle {}

impl Drop for EditorHandle {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        // Let threads finish naturally
        if let Some(handle) = self._server_thread.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self._editor_thread.take() {
            let _ = handle.join();
        }
    }
}

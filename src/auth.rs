//! Token persistence for the VST webview editor.
//!
//! Stores the user's JWT at `~/.hardwave/vst-token` so they don't have to
//! log in every time the plugin window is opened.
//!
//! Also checks the Hardwave Suite token file (`data_dir/hardwave/auth_token`)
//! as a fallback, so users already logged into the Suite don't need a separate
//! WettBoi login.

use std::fs;
use std::path::PathBuf;

/// Get the path to the plugin-specific token file.
fn token_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".hardwave").join("vst-token"))
}

/// Token path written by the Hardwave Suite on login/logout.
fn suite_token_path() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("hardwave").join("auth_token"))
}

/// Load a previously-saved JWT token from disk.
///
/// Checks two locations in order:
///   1. `~/.hardwave/vst-token` — written by WettBoi itself after login.
///   2. Suite token (`data_dir/hardwave/auth_token`) — written by the Hardwave
///      Suite on login, so users already logged into the Suite get automatic
///      single sign-on without a separate WettBoi login.
pub fn load_token() -> Option<String> {
    for path_opt in &[token_path(), suite_token_path()] {
        let Some(path) = path_opt else { continue };
        if let Ok(s) = fs::read_to_string(path) {
            let t = s.trim().to_string();
            if !t.is_empty() {
                return Some(t);
            }
        }
    }
    None
}

/// Save a JWT token to disk.
pub fn save_token(token: &str) {
    if let Some(path) = token_path() {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(path, token);
    }
}

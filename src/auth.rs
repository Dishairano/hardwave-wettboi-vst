//! Shared auth token persistence.
//!
//! All Hardwave VST plugins share the same token file at
//! `~/.local/share/hardwave/auth_token` (Linux/macOS) or the platform
//! equivalent via `dirs::data_dir()`.

use std::fs;
use std::path::PathBuf;

fn token_path() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("hardwave").join("auth_token"))
}

pub fn load_token() -> Option<String> {
    token_path()
        .and_then(|p| fs::read_to_string(p).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

pub fn save_token(token: &str) -> Result<(), String> {
    let p = token_path().ok_or("No data dir")?;
    fs::create_dir_all(p.parent().unwrap()).map_err(|e| e.to_string())?;
    fs::write(p, token).map_err(|e| e.to_string())
}

pub fn clear_token() -> Result<(), String> {
    if let Some(p) = token_path() {
        if p.exists() {
            fs::remove_file(p).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// What is hosting this plug-in, when we can tell.
///
/// Our own plug-ins are free inside the Hardwave DAW and paid in
/// every other host, so the window has to be able to say which it is
/// in. The DAW marks its own process before anything loads, and a
/// plug-in loaded into it inherits that.
///
/// Nothing is claimed when the variable is absent: a plug-in in FL or
/// Ableton simply says nothing, and the page falls back to the
/// licence the user has.
pub fn host_kind() -> Option<String> {
    // Set by the Hardwave DAW for its own process and inherited by
    // the sandbox helper it starts.
    if let Ok(host) = std::env::var("HARDWAVE_HOST") {
        let host = host.trim().to_lowercase();
        if !host.is_empty() {
            return Some(host);
        }
    }
    // The same answer by another road, for a host that strips the
    // environment: the program we are loaded into is named after
    // itself.
    let exe = std::env::current_exe().ok()?;
    let name = exe.file_stem()?.to_string_lossy().to_lowercase();
    name.contains("hardwave-daw")
        .then(|| "hardwave-daw".to_string())
}

/// The query the window is opened with, so the page knows the version
/// it is talking to and where it is running.
pub fn url_query(token: Option<&str>, version: &str) -> String {
    let mut query = String::new();
    if let Some(token) = token {
        query.push_str(&format!("?token={token}&v={version}"));
    } else {
        query.push_str(&format!("?v={version}"));
    }
    if let Some(host) = host_kind() {
        query.push_str(&format!("&host={host}"));
    }
    query
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_and_a_version_always_go_in() {
        let query = url_query(Some("abc"), "1.2.3");
        assert!(query.starts_with("?token=abc&v=1.2.3"), "{query}");
        let anonymous = url_query(None, "1.2.3");
        assert!(anonymous.starts_with("?v=1.2.3"), "{anonymous}");
    }

    #[test]
    fn the_host_is_named_only_when_it_is_known() {
        // The test process is not the DAW, so nothing is claimed.
        if std::env::var("HARDWAVE_HOST").is_err() {
            let query = url_query(None, "1.0.0");
            assert!(
                !query.contains("host="),
                "a plug-in in someone else's host claims nothing: {query}"
            );
        }
    }
}

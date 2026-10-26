//! The sign-in token the editor's page needs, shared with the Suite and every other plug-in.
//!
//! One sign-in is meant to be enough: made in the Suite or in any plug-in window, it is written to
//! `data_dir/hardwave/auth_token`, the file the Suite and every plug-in read, and the next window
//! of any of them opens signed in. Signing out removes it again.
//!
//! `~/.hardwave/vst-token` is read as well. WettBoi never wrote it, but older Analyser and
//! KickForge builds did, and for a person who signed in there it holds the only token. Of the
//! tokens found, the newest one that has not expired is used, and signing out removes both files,
//! so a copy left in the older one cannot sign the next window back in.
//!
//! Every file is read fresh when a window opens. The files come from other programs, so nothing in
//! them is trusted: a token has to look like a JWT, stay under a size limit and carry a readable
//! payload.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Longer than any token the Hardwave server issues; anything bigger is not a token.
const MAX_TOKEN_LEN: usize = 8 * 1024;

/// The file the Suite and every plug-in read: the one place a sign-in lives.
fn shared_token_path() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("hardwave").join("auth_token"))
}

/// The file older Analyser and KickForge builds wrote (see the module notes).
fn legacy_token_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".hardwave").join("vst-token"))
}

/// Every file a token may be in, the shared one first.
fn token_paths() -> Vec<PathBuf> {
    [shared_token_path(), legacy_token_path()]
        .into_iter()
        .flatten()
        .collect()
}

/// Seconds since 1970, now.
fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Decode unpadded base64url, the alphabet a JWT is written in. `None` for anything else.
fn base64url_decode(s: &str) -> Option<Vec<u8>> {
    fn value(c: u8) -> Option<u32> {
        match c {
            b'A'..=b'Z' => Some(u32::from(c - b'A')),
            b'a'..=b'z' => Some(u32::from(c - b'a') + 26),
            b'0'..=b'9' => Some(u32::from(c - b'0') + 52),
            b'-' => Some(62),
            b'_' => Some(63),
            _ => None,
        }
    }
    let s = s.trim_end_matches('=');
    // One character left over carries fewer than 8 bits: not base64 at all.
    if s.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    for chunk in s.as_bytes().chunks(4) {
        let mut acc = 0u32;
        for &c in chunk {
            acc = (acc << 6) | value(c)?;
        }
        // `chunk.len()` is 2, 3 or 4 here, so the shift and the byte count stay in range.
        acc <<= 6 * (4 - chunk.len() as u32);
        let bytes = acc.to_be_bytes();
        out.extend_from_slice(&bytes[1..chunk.len()]);
    }
    Some(out)
}

/// What a token's payload says, when it is shaped like a JWT: `Some(exp)` with the expiry in
/// seconds since 1970 when it has one, `Some(None)` when it has none. `None` when it is not a
/// JWT at all: too long, not three base64url parts, or a payload that is not a JSON object.
///
/// The signature is not checked: the server does that. This only decides which of the tokens on
/// disk is worth handing to the page.
fn token_expiry(token: &str) -> Option<Option<u64>> {
    if token.is_empty() || token.len() > MAX_TOKEN_LEN {
        return None;
    }
    let mut parts = token.split('.');
    let (Some(header), Some(payload), Some(_signature), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return None;
    };
    if header.is_empty() || payload.is_empty() || base64url_decode(header).is_none() {
        return None;
    }
    let payload = base64url_decode(payload)?;
    let claims: serde_json::Value = serde_json::from_slice(&payload).ok()?;
    let claims = claims.as_object()?;
    // A fractional expiry is read as the whole second it falls in, and a value that is not a
    // number at all counts as unreadable rather than as "never expires".
    Some(match claims.get("exp") {
        None => None,
        Some(exp) => Some(exp.as_u64().or_else(|| {
            exp.as_f64()
                .filter(|e| e.is_finite() && *e >= 0.0 && *e < u64::MAX as f64)
                .map(|e| e as u64)
        })?),
    })
}

/// A token worth handing to the page at `now`: shaped like a JWT and not expired. A token without
/// an expiry is left to the server to judge.
fn token_usable(token: &str, now: u64) -> bool {
    match token_expiry(token) {
        Some(Some(exp)) => exp > now,
        Some(None) => true,
        None => false,
    }
}

/// Of tokens found on disk with the time each file was written, the newest that is usable at `now`.
fn choose_token(found: Vec<(String, SystemTime)>, now: u64) -> Option<String> {
    found
        .into_iter()
        .filter(|(token, _)| token_usable(token, now))
        .max_by_key(|(_, written)| *written)
        .map(|(token, _)| token)
}

/// The token in `path`, trimmed, with when the file was written. Files larger than any token are
/// not read.
fn read_token_file(path: &Path) -> Option<(String, SystemTime)> {
    let meta = fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_TOKEN_LEN as u64 + 64 {
        return None;
    }
    let text = fs::read_to_string(path).ok()?;
    let token = text.trim();
    if token.is_empty() {
        return None;
    }
    Some((token.to_string(), meta.modified().unwrap_or(UNIX_EPOCH)))
}

/// The sign-in to open a window with: the newest token on disk that has not expired.
///
/// Called every time a window opens, never kept from an earlier one, so a sign-in made since (in
/// the Suite or in another plug-in's window) is there.
pub fn load_token() -> Option<String> {
    let found = token_paths()
        .iter()
        .filter_map(|path| read_token_file(path))
        .collect();
    choose_token(found, now_secs())
}

/// Write `token` to `path` readable by this user only (0600 on macOS and Linux; the user's AppData
/// folder already is on Windows), replacing the file in one step so the Suite or another plug-in
/// never reads half a token.
fn write_token_file(path: &Path, token: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut tmp_name = path.file_name().unwrap_or_default().to_os_string();
    tmp_name.push(format!(".{}.tmp", std::process::id()));
    let tmp = path.with_file_name(tmp_name);
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let written = options.open(&tmp).and_then(|mut f| {
        // A file left from an earlier run keeps its old mode through `open`: set it again.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            f.set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        f.write_all(token.as_bytes())?;
        f.sync_all()
    });
    let renamed = written.and_then(|()| fs::rename(&tmp, path));
    if renamed.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    renamed
}

/// Remove the token file at `path`. A file that is not there is already what was asked for.
fn remove_token_file(path: &Path) -> std::io::Result<()> {
    match fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// Keep a sign-in made in this window for the Suite and every plug-in, in the shared file.
///
/// The page is the only caller and the file is read by other programs, so a string that is not a
/// usable token is refused and nothing is written.
pub fn save_token(token: &str) -> Result<(), String> {
    let token = token.trim();
    if !token_usable(token, now_secs()) {
        return Err("not a usable token".to_string());
    }
    let path = shared_token_path().ok_or("no data folder on this system")?;
    write_token_file(&path, token).map_err(|e| e.to_string())
}

/// Sign out: remove the token from every file it may be in, so no window of any plug-in opens
/// with it again. A copy left in either file would be picked up at the next open.
pub fn clear_token() -> Result<(), String> {
    let mut failed = None;
    for path in token_paths() {
        if let Err(e) = remove_token_file(&path) {
            failed = Some(e.to_string());
        }
    }
    failed.map_or(Ok(()), Err)
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
///
/// The sign-in token never goes in here. A URL ends up in server access
/// logs, history and anything else that records it; the token reaches
/// the page through the editor's initialization script instead.
pub fn url_query(version: &str) -> String {
    let mut query = format!("?v={version}");
    if let Some(host) = host_kind() {
        query.push_str(&format!("&host={host}"));
    }
    query
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_version_always_goes_in() {
        let query = url_query("1.2.3");
        assert!(query.starts_with("?v=1.2.3"), "{query}");
    }

    #[test]
    fn the_query_never_carries_a_token() {
        let query = url_query("1.2.3");
        assert!(!query.contains("token"), "{query}");
    }

    #[test]
    fn the_host_is_named_only_when_it_is_known() {
        // The test process is not the DAW, so nothing is claimed.
        if std::env::var("HARDWAVE_HOST").is_err() {
            let query = url_query("1.0.0");
            assert!(
                !query.contains("host="),
                "a plug-in in someone else's host claims nothing: {query}"
            );
        }
    }
}

#[cfg(test)]
mod token_tests {
    use super::*;
    use std::time::Duration;

    const NOW: u64 = 1_800_000_000;

    fn b64url(bytes: &[u8]) -> String {
        const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let mut acc = 0u32;
            for (i, &b) in chunk.iter().enumerate() {
                acc |= u32::from(b) << (16 - 8 * i);
            }
            for i in 0..=chunk.len() {
                out.push(A[((acc >> (18 - 6 * i)) & 63) as usize] as char);
            }
        }
        out
    }

    /// A JWT whose payload is `claims`; the signature is filler the plug-in never reads.
    fn jwt(claims: &str) -> String {
        format!(
            "{}.{}.c2lnbmF0dXJl",
            b64url(br#"{"alg":"HS256","typ":"JWT"}"#),
            b64url(claims.as_bytes())
        )
    }

    fn at(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs)
    }

    /// A folder of its own under the temp dir, removed again when the test ends.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "wettboi-auth-{name}-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            ));
            fs::create_dir_all(&dir).expect("a temp folder");
            TempDir(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn base64url_round_trips() {
        for text in [
            "",
            "a",
            "ab",
            "abc",
            "abcd",
            "{\"exp\":1}",
            "\u{ff}\u{fe}?>",
        ] {
            assert_eq!(
                base64url_decode(&b64url(text.as_bytes())).as_deref(),
                Some(text.as_bytes()),
                "{text}"
            );
        }
        assert_eq!(
            base64url_decode("a+b/"),
            None,
            "standard base64 is not base64url"
        );
        assert_eq!(base64url_decode("abcde"), None, "a lone sixth bit group");
    }

    #[test]
    fn the_expiry_is_read_from_the_payload() {
        assert_eq!(
            token_expiry(&jwt(r#"{"sub":"u","exp":1700000000}"#)),
            Some(Some(1_700_000_000))
        );
        assert_eq!(
            token_expiry(&jwt(r#"{"exp":1700000000.5}"#)),
            Some(Some(1_700_000_000))
        );
        assert_eq!(token_expiry(&jwt(r#"{"sub":"u"}"#)), Some(None));
        assert_eq!(token_expiry(&jwt(r#"{"exp":"tomorrow"}"#)), None);
        assert_eq!(token_expiry(&jwt(r#"{"exp":-5}"#)), None);
        assert_eq!(token_expiry(&jwt(r#"{"exp":1e300}"#)), None);
    }

    #[test]
    fn anything_but_a_jwt_is_refused() {
        assert!(!token_usable("", NOW));
        assert!(!token_usable("not-a-token", NOW));
        assert!(!token_usable("a.b", NOW));
        assert!(!token_usable("a.b.c.d", NOW));
        assert!(
            !token_usable(&format!("eyJ9.{}.y", b64url(b"[1,2]")), NOW),
            "payload is not an object"
        );
        assert!(!token_usable("x.!!!.y", NOW), "payload is not base64url");
        let huge = jwt(&format!(r#"{{"pad":"{}"}}"#, "a".repeat(MAX_TOKEN_LEN)));
        assert!(!token_usable(&huge, NOW), "longer than any real token");
    }

    #[test]
    fn the_newest_valid_token_wins() {
        let old = jwt(&format!(r#"{{"exp":{}}}"#, NOW + 3600));
        let new = jwt(&format!(r#"{{"exp":{}}}"#, NOW + 60));
        let chosen = choose_token(vec![(old.clone(), at(100)), (new.clone(), at(200))], NOW);
        assert_eq!(
            chosen,
            Some(new.clone()),
            "written last wins, whatever its expiry"
        );
        let chosen = choose_token(vec![(new.clone(), at(200)), (old, at(100))], NOW);
        assert_eq!(
            chosen,
            Some(new),
            "the order the files are read in does not matter"
        );
    }

    #[test]
    fn an_expired_token_is_skipped_even_when_newest() {
        let valid = jwt(&format!(r#"{{"exp":{}}}"#, NOW + 3600));
        let expired = jwt(&format!(r#"{{"exp":{}}}"#, NOW - 1));
        let at_now = jwt(&format!(r#"{{"exp":{NOW}}}"#));
        let chosen = choose_token(
            vec![
                (valid.clone(), at(100)),
                (expired.clone(), at(300)),
                (at_now, at(400)),
                ("garbage".to_string(), at(500)),
            ],
            NOW,
        );
        assert_eq!(chosen, Some(valid));
        assert_eq!(choose_token(vec![(expired, at(300))], NOW), None);
        assert_eq!(choose_token(Vec::new(), NOW), None);
    }

    #[test]
    fn a_token_without_an_expiry_is_left_to_the_server() {
        let no_exp = jwt(r#"{"sub":"1"}"#);
        assert_eq!(
            choose_token(vec![(no_exp.clone(), at(1))], NOW),
            Some(no_exp)
        );
    }

    #[test]
    fn a_token_is_written_for_this_user_only_and_removed_again() {
        let dir = TempDir::new("write");
        let path = dir.0.join("hardwave").join("auth_token");
        let token = jwt(&format!(r#"{{"exp":{}}}"#, NOW));

        write_token_file(&path, "older").expect("first write");
        write_token_file(&path, &token).expect("write over an older token");
        assert_eq!(fs::read_to_string(&path).expect("read back"), token);
        let (read, _) = read_token_file(&path).expect("the file reads as a token");
        assert_eq!(read, token);
        let leftovers: Vec<_> = fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name() != "auth_token")
            .collect();
        assert!(leftovers.is_empty(), "no temporary file is left behind");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "only the user may read the token");
        }

        remove_token_file(&path).expect("remove");
        assert!(!path.exists());
        remove_token_file(&path).expect("removing a token that is gone is not an error");
        assert!(read_token_file(&path).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn a_file_left_readable_by_others_is_tightened() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new("mode");
        let path = dir.0.join("auth_token");
        fs::write(&path, "old").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        write_token_file(&path, "new").unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn a_token_file_is_read_trimmed_and_an_empty_one_is_nothing() {
        let dir = TempDir::new("read");
        let path = dir.0.join("auth_token");
        fs::write(&path, "  a.b.c\n").unwrap();
        assert_eq!(read_token_file(&path).map(|(t, _)| t), Some("a.b.c".into()));
        fs::write(&path, "\n").unwrap();
        assert_eq!(read_token_file(&path), None);
        fs::write(&path, "x".repeat(MAX_TOKEN_LEN * 2)).unwrap();
        assert_eq!(read_token_file(&path), None, "an oversized file is ignored");
        assert_eq!(read_token_file(&dir.0.join("missing")), None);
        assert_eq!(read_token_file(&dir.0), None, "a folder is not a token");
    }

    #[test]
    fn the_page_cannot_save_something_that_is_not_a_token() {
        // Refused before any file is touched, so this test writes nothing to the real data folder.
        assert!(save_token("").is_err());
        assert!(save_token("</script>").is_err());
        assert!(save_token(&jwt(&format!(r#"{{"exp":{}}}"#, 1))).is_err());
    }
}

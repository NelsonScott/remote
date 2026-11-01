//! On-disk cache of channel icons: `~/.cache/remote/icons/<device>/<app id>.png`.
//!
//! `ROKU_CACHE_DIR` overrides the root (tests and mock mode).

use std::fs;
use std::io;
use std::path::PathBuf;

use crate::ecp::{self, Client};

pub fn cache_root() -> PathBuf {
    if let Some(dir) = std::env::var_os("ROKU_CACHE_DIR") {
        return PathBuf::from(dir);
    }
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("remote")
}

fn safe_component(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let cleaned = cleaned.trim_matches('.').to_string();
    if cleaned.is_empty() {
        "_".into()
    } else {
        cleaned
    }
}

pub fn icon_path(device_key: &str, app_id: &str) -> PathBuf {
    cache_root()
        .join("icons")
        .join(safe_component(device_key))
        .join(format!("{}.png", safe_component(app_id)))
}

#[derive(Debug)]
pub enum IconError {
    Ecp(ecp::Error),
    Io(io::Error),
}

impl std::fmt::Display for IconError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IconError::Ecp(e) => write!(f, "{e}"),
            IconError::Io(e) => write!(f, "{e}"),
        }
    }
}

/// Returns the cached file, downloading it first if missing. Downloads are
/// written atomically so a half-written file never gets served.
pub fn fetch_icon(client: &Client, device_key: &str, app_id: &str) -> Result<PathBuf, IconError> {
    let path = icon_path(device_key, app_id);
    if path.is_file() && fs::metadata(&path).map(|m| m.len() > 0).unwrap_or(false) {
        return Ok(path);
    }
    let bytes = client.icon(app_id).map_err(IconError::Ecp)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(IconError::Io)?;
    }
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    fs::write(&tmp, &bytes).map_err(IconError::Io)?;
    fs::rename(&tmp, &path).map_err(IconError::Io)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_sanitised() {
        std::env::set_var("ROKU_CACHE_DIR", "/tmp/roku-icons-test-root");
        let p = icon_path("DEV/../x", "tvinput.hdmi1");
        assert_eq!(
            p,
            PathBuf::from("/tmp/roku-icons-test-root/icons/DEV_.._x/tvinput.hdmi1.png")
        );
        let p = icon_path("", "../../etc/passwd");
        assert!(p.ends_with("icons/_/_.._etc_passwd.png"), "{p:?}");
        std::env::remove_var("ROKU_CACHE_DIR");
    }
}

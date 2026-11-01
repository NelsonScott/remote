//! `~/.config/remote/config.toml`: the last device we talked to and the
//! theme preference.
//!
//! `ROKU_CONFIG_DIR` overrides the directory (tests and mock mode use it so
//! they never touch the real file).

use std::fs;
use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Config {
    pub device: Option<SavedDevice>,
    /// Off (default): always Roam's dark charcoal. On: follow the GNOME
    /// light/dark preference.
    #[serde(default)]
    pub follow_system_theme: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SavedDevice {
    /// `ip:port`
    pub host: String,
    pub name: String,
    #[serde(default)]
    pub usn: String,
}

pub fn config_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("ROKU_CONFIG_DIR") {
        return PathBuf::from(dir);
    }
    let base = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
    let dir = base.join("remote");
    // The app was called "roku" before 0.2.0: adopt that config once.
    let old = base.join("roku").join("config.toml");
    if !dir.join("config.toml").exists() && old.exists() && std::fs::create_dir_all(&dir).is_ok() {
        let _ = std::fs::copy(&old, dir.join("config.toml"));
    }
    dir
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.toml")
}

impl Config {
    /// Missing or unreadable config is just the default; a corrupt file is
    /// logged and ignored rather than crashing the app.
    pub fn load() -> Config {
        let path = config_path();
        match fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).unwrap_or_else(|e| {
                log::warn!("ignoring unparsable {}: {e}", path.display());
                Config::default()
            }),
            Err(_) => Config::default(),
        }
    }

    /// Load, change, save: keeps the fields the caller does not touch.
    pub fn update(f: impl FnOnce(&mut Config)) -> io::Result<()> {
        let mut cfg = Config::load();
        f(&mut cfg);
        cfg.save()
    }

    pub fn save(&self) -> io::Result<()> {
        let path = config_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let text = toml::to_string_pretty(self).map_err(io::Error::other)?;
        let tmp = path.with_extension("toml.tmp");
        fs::write(&tmp, text)?;
        fs::rename(&tmp, &path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // env vars are process-global; serialise tests that set them
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn temp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("roku-cfg-test-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn round_trip() {
        let _g = ENV_LOCK.lock().unwrap();
        let dir = temp_dir("rt");
        std::env::set_var("ROKU_CONFIG_DIR", &dir);
        assert_eq!(Config::load(), Config::default());
        let cfg = Config {
            device: Some(SavedDevice {
                host: "192.168.1.221:8060".into(),
                name: "Living Room TV".into(),
                usn: "uuid:roku:ecp:YH00AB123456".into(),
            }),
            follow_system_theme: false,
        };
        cfg.save().unwrap();
        assert_eq!(Config::load(), cfg);
        let text = fs::read_to_string(dir.join("config.toml")).unwrap();
        assert!(text.contains("192.168.1.221:8060"));
        std::env::remove_var("ROKU_CONFIG_DIR");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn theme_defaults_off_and_survives_device_saves() {
        let _g = ENV_LOCK.lock().unwrap();
        let dir = temp_dir("theme");
        fs::create_dir_all(&dir).unwrap();
        // a config written before the setting existed
        fs::write(
            dir.join("config.toml"),
            "[device]\nhost = \"1.2.3.4:8060\"\nname = \"TV\"\n",
        )
        .unwrap();
        std::env::set_var("ROKU_CONFIG_DIR", &dir);
        assert!(!Config::load().follow_system_theme);
        Config::update(|c| c.follow_system_theme = true).unwrap();
        Config::update(|c| {
            c.device = Some(SavedDevice {
                host: "5.6.7.8:8060".into(),
                name: "Other".into(),
                usn: String::new(),
            })
        })
        .unwrap();
        let cfg = Config::load();
        assert!(cfg.follow_system_theme);
        assert_eq!(cfg.device.unwrap().host, "5.6.7.8:8060");
        std::env::remove_var("ROKU_CONFIG_DIR");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_file_is_ignored() {
        let _g = ENV_LOCK.lock().unwrap();
        let dir = temp_dir("corrupt");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("config.toml"), "this is = not [ toml").unwrap();
        std::env::set_var("ROKU_CONFIG_DIR", &dir);
        assert_eq!(Config::load(), Config::default());
        std::env::remove_var("ROKU_CONFIG_DIR");
        let _ = fs::remove_dir_all(&dir);
    }
}

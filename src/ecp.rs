//! Roku External Control Protocol (ECP) client.
//!
//! ECP is plain HTTP on port 8060. Everything here is synchronous (ureq) and
//! `Send`, so the UI runs it on `gio::spawn_blocking` threads.

use std::fmt;
use std::time::Duration;

use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};

/// Everything except RFC 3986 "unreserved" characters gets percent-encoded.
const PATH_ESCAPE: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

pub const ECP_PORT: u16 = 8060;

/// Timeout for key presses and typed characters.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(3);
/// Timeout for `/launch`: a TV starting a channel can hold the response
/// for several seconds (the channel still launches).
const LAUNCH_TIMEOUT: Duration = Duration::from_secs(10);
/// Timeout for the liveness probe (device-info). A busy TV can take well
/// over a second; one miss is not "offline" (see `liveness`).
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(3);
/// Timeout for bulk reads (apps list, icons).
const QUERY_TIMEOUT: Duration = Duration::from_secs(8);

/// Remote keys understood by `/keypress`, `/keydown`, `/keyup`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Key {
    Home,
    Rev,
    Fwd,
    Play,
    Select,
    Left,
    Right,
    Down,
    Up,
    Back,
    InstantReplay,
    /// The `*` (Options) button. ECP calls it `Info`.
    Info,
    Backspace,
    Search,
    Enter,
    VolumeUp,
    VolumeDown,
    VolumeMute,
    PowerOff,
    PowerOn,
    /// Toggle; Roku TVs accept `Power` and flip state.
    Power,
    ChannelUp,
    ChannelDown,
    InputTuner,
    InputHDMI1,
    InputHDMI2,
    InputHDMI3,
    InputHDMI4,
    InputAV1,
}

impl Key {
    pub const ALL: &'static [Key] = &[
        Key::Home,
        Key::Rev,
        Key::Fwd,
        Key::Play,
        Key::Select,
        Key::Left,
        Key::Right,
        Key::Down,
        Key::Up,
        Key::Back,
        Key::InstantReplay,
        Key::Info,
        Key::Backspace,
        Key::Search,
        Key::Enter,
        Key::VolumeUp,
        Key::VolumeDown,
        Key::VolumeMute,
        Key::PowerOff,
        Key::PowerOn,
        Key::Power,
        Key::ChannelUp,
        Key::ChannelDown,
        Key::InputTuner,
        Key::InputHDMI1,
        Key::InputHDMI2,
        Key::InputHDMI3,
        Key::InputHDMI4,
        Key::InputAV1,
    ];

    /// The exact token ECP expects in the URL path.
    pub fn as_str(self) -> &'static str {
        match self {
            Key::Home => "Home",
            Key::Rev => "Rev",
            Key::Fwd => "Fwd",
            Key::Play => "Play",
            Key::Select => "Select",
            Key::Left => "Left",
            Key::Right => "Right",
            Key::Down => "Down",
            Key::Up => "Up",
            Key::Back => "Back",
            Key::InstantReplay => "InstantReplay",
            Key::Info => "Info",
            Key::Backspace => "Backspace",
            Key::Search => "Search",
            Key::Enter => "Enter",
            Key::VolumeUp => "VolumeUp",
            Key::VolumeDown => "VolumeDown",
            Key::VolumeMute => "VolumeMute",
            Key::PowerOff => "PowerOff",
            Key::PowerOn => "PowerOn",
            Key::Power => "Power",
            Key::ChannelUp => "ChannelUp",
            Key::ChannelDown => "ChannelDown",
            Key::InputTuner => "InputTuner",
            Key::InputHDMI1 => "InputHDMI1",
            Key::InputHDMI2 => "InputHDMI2",
            Key::InputHDMI3 => "InputHDMI3",
            Key::InputHDMI4 => "InputHDMI4",
            Key::InputAV1 => "InputAV1",
        }
    }

    pub fn parse(s: &str) -> Option<Key> {
        Key::ALL.iter().copied().find(|k| k.as_str() == s)
    }

    /// Keys where holding the button should auto-repeat on the TV
    /// (sent as keydown/keyup instead of a single keypress).
    pub fn is_repeatable(self) -> bool {
        matches!(
            self,
            Key::Up
                | Key::Down
                | Key::Left
                | Key::Right
                | Key::VolumeUp
                | Key::VolumeDown
                | Key::Rev
                | Key::Fwd
        )
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Parsed `/query/device-info`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeviceInfo {
    pub udn: String,
    pub serial_number: String,
    pub device_id: String,
    pub model_name: String,
    pub model_number: String,
    /// `user-device-name`, the name the user gave the TV.
    pub user_device_name: String,
    /// `friendly-device-name`, usually the same as above.
    pub friendly_device_name: String,
    pub software_version: String,
    pub software_build: String,
    pub power_mode: String,
    pub is_tv: bool,
    pub supports_private_listening: bool,
    pub supports_ecs_textedit: bool,
}

impl DeviceInfo {
    /// Best display name for the header bar.
    pub fn display_name(&self) -> String {
        for candidate in [&self.user_device_name, &self.friendly_device_name] {
            if !candidate.trim().is_empty() {
                return candidate.trim().to_string();
            }
        }
        if !self.model_name.trim().is_empty() {
            return self.model_name.trim().to_string();
        }
        "Roku".to_string()
    }

    /// Stable key for per-device caches.
    pub fn cache_key(&self) -> String {
        let raw = if !self.device_id.is_empty() {
            &self.device_id
        } else if !self.serial_number.is_empty() {
            &self.serial_number
        } else {
            &self.udn
        };
        let cleaned: String = raw
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
            .collect();
        if cleaned.is_empty() {
            "unknown".into()
        } else {
            cleaned
        }
    }

    pub fn is_awake(&self) -> bool {
        // Roku reports "PowerOn", "Headless", "DisplayOff", "Ready" (standby).
        self.power_mode.eq_ignore_ascii_case("PowerOn")
    }
}

/// One entry of `/query/apps`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct App {
    pub id: String,
    pub name: String,
    /// `appl`, `menu`, `tvin`, ...
    pub kind: String,
    pub version: String,
}

/// Parsed `/query/active-app`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ActiveApp {
    /// `None` on the home screen.
    pub id: Option<String>,
    pub name: String,
}

#[derive(Debug)]
pub enum Error {
    /// Network level failure (timeout, refused, DNS).
    Transport(String),
    /// Non-2xx response.
    Status { code: u16, path: String },
    /// Body not parseable.
    Parse(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Transport(m) => write!(f, "network error: {m}"),
            Error::Status { code, path } => write!(f, "HTTP {code} for {path}"),
            Error::Parse(m) => write!(f, "bad response: {m}"),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

/// HTTP client bound to one Roku.
#[derive(Clone)]
pub struct Client {
    base: String,
    command: ureq::Agent,
    launch: ureq::Agent,
    probe: ureq::Agent,
    query: ureq::Agent,
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client").field("base", &self.base).finish()
    }
}

fn agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
        .user_agent(concat!("Roku/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

impl Client {
    /// `host` is `ip` or `ip:port`; port defaults to 8060.
    pub fn new(host: &str) -> Client {
        let host = host.trim().trim_end_matches('/');
        let host = host
            .strip_prefix("http://")
            .or_else(|| host.strip_prefix("https://"))
            .unwrap_or(host);
        let authority = if host.contains(':') {
            host.to_string()
        } else {
            format!("{host}:{ECP_PORT}")
        };
        Client {
            base: format!("http://{authority}"),
            command: agent(COMMAND_TIMEOUT),
            launch: agent(LAUNCH_TIMEOUT),
            probe: agent(PROBE_TIMEOUT),
            query: agent(QUERY_TIMEOUT),
        }
    }

    /// Same client with a different probe timeout (tests use a short one).
    pub fn with_probe_timeout(mut self, timeout: Duration) -> Client {
        self.probe = agent(timeout);
        self
    }

    /// `http://ip:port` with no trailing slash.
    pub fn base_url(&self) -> &str {
        &self.base
    }

    /// `ip:port`.
    pub fn host(&self) -> &str {
        self.base.trim_start_matches("http://")
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    fn post(&self, path: &str) -> Result<()> {
        self.post_with(&self.command, path)
    }

    fn post_with(&self, agent: &ureq::Agent, path: &str) -> Result<()> {
        let url = self.url(path);
        let resp = agent
            .post(&url)
            .send_empty()
            .map_err(|e| Error::Transport(e.to_string()))?;
        let code = resp.status().as_u16();
        if (200..300).contains(&code) {
            Ok(())
        } else {
            Err(Error::Status {
                code,
                path: path.to_string(),
            })
        }
    }

    fn get_bytes(&self, agent: &ureq::Agent, path: &str) -> Result<Vec<u8>> {
        let url = self.url(path);
        let mut resp = agent
            .get(&url)
            .call()
            .map_err(|e| Error::Transport(e.to_string()))?;
        let code = resp.status().as_u16();
        if !(200..300).contains(&code) {
            return Err(Error::Status {
                code,
                path: path.to_string(),
            });
        }
        resp.body_mut()
            .with_config()
            .limit(8 * 1024 * 1024)
            .read_to_vec()
            .map_err(|e| Error::Transport(e.to_string()))
    }

    fn get_text(&self, agent: &ureq::Agent, path: &str) -> Result<String> {
        let bytes = self.get_bytes(agent, path)?;
        String::from_utf8(bytes).map_err(|e| Error::Parse(e.to_string()))
    }

    // --- commands -------------------------------------------------------

    pub fn keypress(&self, key: Key) -> Result<()> {
        self.post(&format!("/keypress/{}", key.as_str()))
    }

    pub fn keydown(&self, key: Key) -> Result<()> {
        self.post(&format!("/keydown/{}", key.as_str()))
    }

    pub fn keyup(&self, key: Key) -> Result<()> {
        self.post(&format!("/keyup/{}", key.as_str()))
    }

    /// Types one character into the focused TV text field.
    pub fn type_char(&self, c: char) -> Result<()> {
        self.post(&format!("/keypress/{}", lit_token(c)))
    }

    /// Types a string, one `Lit_` keypress per character, stopping at the
    /// first failure.
    pub fn type_text(&self, text: &str) -> Result<()> {
        for c in text.chars() {
            self.type_char(c)?;
        }
        Ok(())
    }

    pub fn launch(&self, app_id: &str) -> Result<()> {
        let id = utf8_percent_encode(app_id, PATH_ESCAPE).to_string();
        self.post_with(&self.launch, &format!("/launch/{id}"))
    }

    // --- queries --------------------------------------------------------

    /// Liveness check + metadata (`PROBE_TIMEOUT`).
    pub fn device_info(&self) -> Result<DeviceInfo> {
        let xml = self.get_text(&self.probe, "/query/device-info")?;
        parse_device_info(&xml)
    }

    pub fn apps(&self) -> Result<Vec<App>> {
        let xml = self.get_text(&self.query, "/query/apps")?;
        parse_apps(&xml)
    }

    pub fn active_app(&self) -> Result<ActiveApp> {
        let xml = self.get_text(&self.probe, "/query/active-app")?;
        parse_active_app(&xml)
    }

    /// Raw PNG/JPEG bytes of the channel icon.
    pub fn icon(&self, app_id: &str) -> Result<Vec<u8>> {
        let id = utf8_percent_encode(app_id, PATH_ESCAPE).to_string();
        self.get_bytes(&self.query, &format!("/query/icon/{id}"))
    }
}

/// `Lit_` token for one character, e.g. `Lit_a`, `Lit_%20`, `Lit_%C3%A9`.
pub fn lit_token(c: char) -> String {
    let mut buf = [0u8; 4];
    let s = c.encode_utf8(&mut buf);
    format!("Lit_{}", utf8_percent_encode(s, PATH_ESCAPE))
}

// --- XML parsing ---------------------------------------------------------

fn child_text<'a>(root: roxmltree::Node<'a, 'a>, tag: &str) -> String {
    root.children()
        .find(|n| n.is_element() && n.tag_name().name() == tag)
        .and_then(|n| n.text())
        .map(|t| t.trim().to_string())
        .unwrap_or_default()
}

fn child_bool(root: roxmltree::Node<'_, '_>, tag: &str) -> bool {
    child_text(root, tag).eq_ignore_ascii_case("true")
}

pub fn parse_device_info(xml: &str) -> Result<DeviceInfo> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| Error::Parse(e.to_string()))?;
    let root = doc.root_element();
    if root.tag_name().name() != "device-info" {
        return Err(Error::Parse(format!(
            "expected <device-info>, got <{}>",
            root.tag_name().name()
        )));
    }
    Ok(DeviceInfo {
        udn: child_text(root, "udn"),
        serial_number: child_text(root, "serial-number"),
        device_id: child_text(root, "device-id"),
        model_name: child_text(root, "model-name"),
        model_number: child_text(root, "model-number"),
        user_device_name: child_text(root, "user-device-name"),
        friendly_device_name: child_text(root, "friendly-device-name"),
        software_version: child_text(root, "software-version"),
        software_build: child_text(root, "software-build"),
        power_mode: child_text(root, "power-mode"),
        is_tv: child_bool(root, "is-tv"),
        supports_private_listening: child_bool(root, "supports-private-listening"),
        supports_ecs_textedit: child_bool(root, "supports-ecs-textedit"),
    })
}

pub fn parse_apps(xml: &str) -> Result<Vec<App>> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| Error::Parse(e.to_string()))?;
    let root = doc.root_element();
    if root.tag_name().name() != "apps" {
        return Err(Error::Parse(format!(
            "expected <apps>, got <{}>",
            root.tag_name().name()
        )));
    }
    Ok(root
        .children()
        .filter(|n| n.is_element() && n.tag_name().name() == "app")
        .filter_map(|n| {
            let id = n.attribute("id")?.trim().to_string();
            if id.is_empty() {
                return None;
            }
            Some(App {
                id,
                name: n.text().unwrap_or("").trim().to_string(),
                kind: n.attribute("type").unwrap_or("").to_string(),
                version: n.attribute("version").unwrap_or("").to_string(),
            })
        })
        .collect())
}

pub fn parse_active_app(xml: &str) -> Result<ActiveApp> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| Error::Parse(e.to_string()))?;
    let root = doc.root_element();
    let app = root
        .children()
        .find(|n| n.is_element() && n.tag_name().name() == "app")
        .ok_or_else(|| Error::Parse("no <app> in active-app".into()))?;
    Ok(ActiveApp {
        id: app
            .attribute("id")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
        name: app.text().unwrap_or("").trim().to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_round_trip() {
        for k in Key::ALL {
            assert_eq!(Key::parse(k.as_str()), Some(*k));
        }
        assert_eq!(Key::parse("Nope"), None);
    }

    #[test]
    fn lit_tokens() {
        assert_eq!(lit_token('a'), "Lit_a");
        assert_eq!(lit_token('Z'), "Lit_Z");
        assert_eq!(lit_token('7'), "Lit_7");
        assert_eq!(lit_token(' '), "Lit_%20");
        assert_eq!(lit_token('&'), "Lit_%26");
        assert_eq!(lit_token('/'), "Lit_%2F");
        assert_eq!(lit_token('é'), "Lit_%C3%A9");
        assert_eq!(lit_token('.'), "Lit_.");
        assert_eq!(lit_token('-'), "Lit_-");
    }

    #[test]
    fn client_host_forms() {
        assert_eq!(
            Client::new("192.168.1.221").base_url(),
            "http://192.168.1.221:8060"
        );
        assert_eq!(Client::new("127.0.0.1:9999").host(), "127.0.0.1:9999");
        assert_eq!(
            Client::new("http://192.168.1.221:8060/").base_url(),
            "http://192.168.1.221:8060"
        );
    }

    #[test]
    fn parses_device_info() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8" ?>
<device-info>
  <udn>29380007-0000-1000-8000-d8313d1a2b3c</udn>
  <serial-number>YH00AB123456</serial-number>
  <device-id>S0A1B2C3D4E5</device-id>
  <model-name>4660X</model-name>
  <model-number>7000X</model-number>
  <user-device-name>Living Room TV</user-device-name>
  <friendly-device-name>Living Room TV</friendly-device-name>
  <software-version>15.3.4</software-version>
  <software-build>4190</software-build>
  <is-tv>true</is-tv>
  <power-mode>PowerOn</power-mode>
  <supports-private-listening>true</supports-private-listening>
  <supports-ecs-textedit>true</supports-ecs-textedit>
</device-info>"#;
        let info = parse_device_info(xml).unwrap();
        assert_eq!(info.display_name(), "Living Room TV");
        assert_eq!(info.model_name, "4660X");
        assert_eq!(info.software_version, "15.3.4");
        assert!(info.is_tv);
        assert!(info.is_awake());
        assert!(info.supports_private_listening);
        assert_eq!(info.cache_key(), "S0A1B2C3D4E5");
    }

    #[test]
    fn device_info_rejects_other_roots() {
        assert!(parse_device_info("<apps/>").is_err());
        assert!(parse_device_info("not xml").is_err());
    }

    #[test]
    fn parses_apps() {
        let xml = r#"<apps>
  <app id="12" type="appl" version="4.2.81">Netflix</app>
  <app id="837" type="appl" version="2.0.0">YouTube</app>
  <app id="tvinput.hdmi1" type="tvin" version="1.0.0">PlayStation</app>
  <app type="appl">no id, skipped</app>
</apps>"#;
        let apps = parse_apps(xml).unwrap();
        assert_eq!(apps.len(), 3);
        assert_eq!(apps[0].id, "12");
        assert_eq!(apps[0].name, "Netflix");
        assert_eq!(apps[0].kind, "appl");
        assert_eq!(apps[2].kind, "tvin");
    }

    #[test]
    fn parses_active_app() {
        let home = parse_active_app("<active-app><app>Roku</app></active-app>").unwrap();
        assert_eq!(home.id, None);
        assert_eq!(home.name, "Roku");
        let yt = parse_active_app(
            r#"<active-app><app id="837" type="appl" version="2.0.0">YouTube</app></active-app>"#,
        )
        .unwrap();
        assert_eq!(yt.id.as_deref(), Some("837"));
    }
}

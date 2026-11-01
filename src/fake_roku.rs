//! An in-process fake Roku ECP server (tiny_http) used by the test suite and
//! by `ROKU_MOCK=1`. It records every request so tests can assert on them.

use std::io::Cursor;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use tiny_http::{Header, Method, Response, Server, StatusCode};

use crate::ecp::Key;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedRequest {
    pub method: String,
    pub path: String,
    /// when it arrived (the QA request log prints it)
    pub at: std::time::SystemTime,
}

pub const FAKE_DEVICE_NAME: &str = "Living Room TV (mock)";
pub const FAKE_DEVICE_ID: &str = "MOCK00000001";

/// (id, name, type, rgb colour used for the generated icon)
pub const FAKE_APPS: &[(&str, &str, &str, [u8; 3])] = &[
    ("12", "Netflix", "appl", [229, 9, 20]),
    ("837", "YouTube", "appl", [255, 0, 0]),
    ("13", "Prime Video", "appl", [0, 168, 225]),
    ("2285", "Hulu", "appl", [28, 231, 131]),
    ("291097", "Disney Plus", "appl", [17, 60, 207]),
    ("551012", "Apple TV", "appl", [40, 40, 40]),
    ("61322", "HBO Max", "appl", [91, 46, 255]),
    ("2213", "Roku Media Player", "appl", [102, 45, 145]),
    ("tvinput.hdmi1", "PlayStation 5", "tvin", [0, 55, 145]),
    ("tvinput.hdmi2", "Nintendo Switch", "tvin", [230, 0, 18]),
    ("31012", "Plex", "appl", [229, 160, 13]),
    ("151908", "The Roku Channel", "appl", [103, 58, 183]),
    ("tvinput.dtv", "Live TV", "tvin", [0, 0, 0]),
];

/// Apps whose `/query/icon` answers 404, so the UI placeholder is exercised.
pub const FAKE_NO_ICON: &[&str] = &["tvinput.dtv"];

pub struct FakeRoku {
    port: u16,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
    /// When set, every request gets this status (to test error handling).
    pub force_status: Arc<Mutex<Option<u16>>>,
    faults: Arc<Mutex<Faults>>,
}

/// Slow or absent answers, to mimic a TV that is busy launching a channel or
/// has gone away. Delayed answers are sent from their own thread, so other
/// requests are not held up.
#[derive(Debug, Default)]
struct Faults {
    /// the next `stalls` device-info requests answer only after `stall`
    stalls: u32,
    stall: Duration,
    /// every `/launch` answers after this delay
    launch_delay: Duration,
    /// after a launch, device-info stalls by `busy_stall` for this long
    busy_after_launch: Duration,
    busy_stall: Duration,
    busy_until: Option<Instant>,
    /// nothing answers (requests are held this long, then dropped)
    down: Option<Duration>,
}
impl FakeRoku {
    /// Starts the server on an ephemeral localhost port.
    pub fn start() -> FakeRoku {
        FakeRoku::start_on(0)
    }

    /// Starts the server on a fixed localhost port (0 = ephemeral).
    pub fn start_on(port: u16) -> FakeRoku {
        let server = Server::http(("127.0.0.1", port)).expect("bind fake roku");
        let port = match server.server_addr() {
            tiny_http::ListenAddr::IP(addr) => addr.port(),
            #[allow(unreachable_patterns)]
            _ => panic!("fake roku bound to a non-IP address"),
        };
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let force_status = Arc::new(Mutex::new(None));
        let faults = Arc::new(Mutex::new(Faults::default()));
        let handle = {
            let requests = requests.clone();
            let stop = stop.clone();
            let force_status = force_status.clone();
            let faults = faults.clone();
            std::thread::Builder::new()
                .name("fake-roku".into())
                .spawn(move || serve(server, requests, stop, force_status, faults))
                .expect("spawn fake roku thread")
        };
        FakeRoku {
            port,
            requests,
            stop,
            handle: Some(handle),
            force_status,
            faults,
        }
    }

    /// The next `count` `/query/device-info` requests answer after `delay`
    /// (longer than the client's probe timeout = a missed probe).
    pub fn stall_device_info(&self, count: u32, delay: Duration) {
        let mut f = self.faults.lock().unwrap();
        f.stalls = count;
        f.stall = delay;
    }

    /// Mimic a TV starting a channel: `/launch` answers after `launch_delay`
    /// and device-info stalls by `stall` for `busy_for` after each launch.
    pub fn busy_on_launch(&self, launch_delay: Duration, busy_for: Duration, stall: Duration) {
        let mut f = self.faults.lock().unwrap();
        f.launch_delay = launch_delay;
        f.busy_after_launch = busy_for;
        f.busy_stall = stall;
    }

    /// `Some(hold)`: stop answering anything (each request is held for
    /// `hold`, then dropped). `None`: back to normal.
    pub fn set_down(&self, hold: Option<Duration>) {
        self.faults.lock().unwrap().down = hold;
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// `127.0.0.1:<port>`, ready for `ecp::Client::new`.
    pub fn host(&self) -> String {
        format!("127.0.0.1:{}", self.port)
    }

    pub fn requests(&self) -> Vec<RecordedRequest> {
        self.requests.lock().unwrap().clone()
    }

    pub fn clear_requests(&self) {
        self.requests.lock().unwrap().clear();
    }

    /// Paths of recorded requests with the given method.
    pub fn paths(&self, method: &str) -> Vec<String> {
        self.requests()
            .into_iter()
            .filter(|r| r.method == method)
            .map(|r| r.path)
            .collect()
    }
}

impl Drop for FakeRoku {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn serve(
    server: Server,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
    stop: Arc<AtomicBool>,
    force_status: Arc<Mutex<Option<u16>>>,
    faults: Arc<Mutex<Faults>>,
) {
    while !stop.load(Ordering::SeqCst) {
        let req = match server.recv_timeout(Duration::from_millis(50)) {
            Ok(Some(r)) => r,
            Ok(None) => continue,
            Err(_) => break,
        };
        let method = req.method().clone();
        let path = req.url().to_string();
        requests.lock().unwrap().push(RecordedRequest {
            method: method.as_str().to_string(),
            path: path.clone(),
            at: std::time::SystemTime::now(),
        });
        let delay = {
            let mut f = faults.lock().unwrap();
            let now = Instant::now();
            if let Some(hold) = f.down {
                std::thread::spawn(move || {
                    std::thread::sleep(hold);
                    drop(req);
                });
                continue;
            }
            if path == "/query/device-info" {
                if f.stalls > 0 {
                    f.stalls -= 1;
                    f.stall
                } else if f.busy_until.is_some_and(|t| now < t) {
                    f.busy_stall
                } else {
                    Duration::ZERO
                }
            } else if path.starts_with("/launch/") {
                if !f.busy_after_launch.is_zero() {
                    f.busy_until = Some(now + f.launch_delay + f.busy_after_launch);
                }
                f.launch_delay
            } else {
                Duration::ZERO
            }
        };
        if delay.is_zero() {
            answer(req, &method, &path, &force_status);
        } else {
            let force_status = force_status.clone();
            std::thread::spawn(move || {
                std::thread::sleep(delay);
                answer(req, &method, &path, &force_status);
            });
        }
    }
}

fn answer(req: tiny_http::Request, method: &Method, path: &str, force_status: &Mutex<Option<u16>>) {
    if let Some(code) = *force_status.lock().unwrap() {
        let _ = req.respond(Response::empty(StatusCode(code)));
        return;
    }
    let (status, body, content_type) = route(method, path);
    let mut resp = Response::new(
        StatusCode(status),
        Vec::new(),
        Cursor::new(body.clone()),
        Some(body.len()),
        None,
    );
    if let Some(ct) = content_type {
        resp.add_header(Header::from_bytes("Content-Type", ct).unwrap());
    }
    let _ = req.respond(resp);
}

fn route(method: &Method, path: &str) -> (u16, Vec<u8>, Option<&'static str>) {
    let xml = Some("text/xml; charset=\"utf-8\"");
    match (method, path) {
        (&Method::Get, "/query/device-info") => (200, device_info_xml().into_bytes(), xml),
        (&Method::Get, "/query/apps") => (200, apps_xml().into_bytes(), xml),
        (&Method::Get, "/query/active-app") => (
            200,
            b"<?xml version=\"1.0\" encoding=\"UTF-8\" ?>\n<active-app>\n\t<app>Roku</app>\n</active-app>\n"
                .to_vec(),
            xml,
        ),
        (&Method::Get, p) if p.starts_with("/query/icon/") => {
            let id = &p["/query/icon/".len()..];
            match FAKE_APPS
                .iter()
                .find(|(aid, ..)| *aid == id && !FAKE_NO_ICON.contains(aid))
            {
                Some((_, name, _, rgb)) => (200, icon_png(name, *rgb), Some("image/png")),
                None => (404, Vec::new(), None),
            }
        }
        (&Method::Post, p)
            if p.starts_with("/keypress/") || p.starts_with("/keydown/") || p.starts_with("/keyup/") =>
        {
            let key = p.rsplit('/').next().unwrap_or("");
            if key.starts_with("Lit_") || Key::parse(key).is_some() {
                (200, Vec::new(), None)
            } else {
                (400, Vec::new(), None)
            }
        }
        (&Method::Post, p) if p.starts_with("/launch/") => {
            let id = &p["/launch/".len()..];
            if FAKE_APPS.iter().any(|(aid, ..)| *aid == id) {
                (200, Vec::new(), None)
            } else {
                (404, Vec::new(), None)
            }
        }
        _ => (404, Vec::new(), None),
    }
}

fn device_info_xml() -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" ?>
<device-info>
	<udn>29380007-0000-1000-8000-aabbccddeeff</udn>
	<serial-number>YH00MOCK0001</serial-number>
	<device-id>{FAKE_DEVICE_ID}</device-id>
	<advertising-id>00000000-0000-0000-0000-000000000000</advertising-id>
	<vendor-name>Roku</vendor-name>
	<model-name>4660X</model-name>
	<model-number>7000X</model-number>
	<model-region>US</model-region>
	<is-tv>true</is-tv>
	<is-stick>false</is-stick>
	<user-device-name>{FAKE_DEVICE_NAME}</user-device-name>
	<friendly-device-name>{FAKE_DEVICE_NAME}</friendly-device-name>
	<friendly-model-name>Roku Ultra</friendly-model-name>
	<software-version>15.3.4</software-version>
	<software-build>4190</software-build>
	<power-mode>PowerOn</power-mode>
	<supports-private-listening>true</supports-private-listening>
	<supports-ecs-textedit>true</supports-ecs-textedit>
	<supports-find-remote>false</supports-find-remote>
</device-info>
"#
    )
}

fn apps_xml() -> String {
    let mut s = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\" ?>\n<apps>\n");
    for (id, name, kind, _) in FAKE_APPS {
        s.push_str(&format!(
            "\t<app id=\"{id}\" type=\"{kind}\" version=\"1.0.0\">{name}</app>\n"
        ));
    }
    s.push_str("</apps>\n");
    s
}

// --- minimal PNG writer (no deps) ----------------------------------------

const ICON_W: usize = 290;
const ICON_H: usize = 218;

/// A 290x218 (real Roku icon size) solid tile with a lighter band, so the
/// mock app grid visibly shows distinct icons.
pub fn icon_png(name: &str, rgb: [u8; 3]) -> Vec<u8> {
    let mut raw = Vec::with_capacity((ICON_W * 3 + 1) * ICON_H);
    // deterministic "logo" block position from the name
    let seed = name
        .bytes()
        .fold(7u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32));
    let bx = 40 + (seed % 120) as usize;
    let bw = 60 + (seed / 7 % 90) as usize;
    for y in 0..ICON_H {
        raw.push(0); // filter: none
        for x in 0..ICON_W {
            let in_band = (70..150).contains(&y) && x >= bx && x < bx + bw;
            let px = if in_band {
                [
                    rgb[0].saturating_add(90),
                    rgb[1].saturating_add(90),
                    rgb[2].saturating_add(90),
                ]
            } else {
                rgb
            };
            raw.extend_from_slice(&px);
        }
    }
    let mut png = Vec::new();
    png.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(ICON_W as u32).to_be_bytes());
    ihdr.extend_from_slice(&(ICON_H as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]); // 8-bit RGB
    chunk(&mut png, b"IHDR", &ihdr);
    chunk(&mut png, b"IDAT", &zlib_stored(&raw));
    chunk(&mut png, b"IEND", &[]);
    png
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let crc = crc32(&out[start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

/// zlib stream using uncompressed (stored) deflate blocks.
fn zlib_stored(data: &[u8]) -> Vec<u8> {
    let mut out = vec![0x78, 0x01];
    let mut chunks = data.chunks(65535).peekable();
    if chunks.peek().is_none() {
        out.extend_from_slice(&[1, 0, 0, 0xFF, 0xFF]);
    }
    while let Some(c) = chunks.next() {
        let last = chunks.peek().is_none();
        out.push(if last { 1 } else { 0 });
        let len = c.len() as u16;
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(!len).to_le_bytes());
        out.extend_from_slice(c);
    }
    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &d in data {
        a = (a + d as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            let mask = (!(crc & 1)).wrapping_add(1);
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_has_valid_signature_and_chunks() {
        let png = icon_png("Netflix", [229, 9, 20]);
        assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        assert_eq!(&png[12..16], b"IHDR");
        assert_eq!(&png[png.len() - 8..png.len() - 4], b"IEND");
        // IEND CRC is a known constant
        assert_eq!(&png[png.len() - 4..], &[0xAE, 0x42, 0x60, 0x82]);
    }

    #[test]
    fn crc_and_adler_known_values() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
    }
}

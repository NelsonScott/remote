//! End-to-end tests of the ECP client against the in-process fake Roku.

use std::sync::Mutex;

use remote::ecp::{Client, Error, Key};
use remote::fake_roku::{FakeRoku, FAKE_APPS, FAKE_DEVICE_ID, FAKE_DEVICE_NAME};
use remote::icons;

// icon tests set ROKU_CACHE_DIR, which is process-global
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn setup() -> (FakeRoku, Client) {
    let server = FakeRoku::start();
    let client = Client::new(&server.host());
    (server, client)
}

#[test]
fn device_info_round_trip() {
    let (server, client) = setup();
    let info = client.device_info().expect("device-info");
    assert_eq!(info.display_name(), FAKE_DEVICE_NAME);
    assert_eq!(info.cache_key(), FAKE_DEVICE_ID);
    assert_eq!(info.model_name, "4660X");
    assert!(info.is_tv);
    assert_eq!(server.paths("GET"), vec!["/query/device-info"]);
}

#[test]
fn apps_and_active_app() {
    let (server, client) = setup();
    let apps = client.apps().expect("apps");
    assert_eq!(apps.len(), FAKE_APPS.len());
    assert_eq!(apps[0].name, "Netflix");
    assert_eq!(apps[0].id, "12");
    assert!(apps.iter().any(|a| a.kind == "tvin"));
    let active = client.active_app().expect("active-app");
    assert_eq!(active.id, None);
    assert_eq!(active.name, "Roku");
    assert_eq!(
        server.paths("GET"),
        vec!["/query/apps", "/query/active-app"]
    );
}

#[test]
fn keypress_keydown_keyup_and_launch_hit_the_right_paths() {
    let (server, client) = setup();
    client.keypress(Key::Home).unwrap();
    client.keydown(Key::VolumeUp).unwrap();
    client.keyup(Key::VolumeUp).unwrap();
    client.keypress(Key::Info).unwrap();
    client.launch("837").unwrap();
    client.launch("tvinput.hdmi1").unwrap();
    assert_eq!(
        server.paths("POST"),
        vec![
            "/keypress/Home",
            "/keydown/VolumeUp",
            "/keyup/VolumeUp",
            "/keypress/Info",
            "/launch/837",
            "/launch/tvinput.hdmi1",
        ]
    );
    // nothing was issued as GET
    assert!(server.paths("GET").is_empty());
}

#[test]
fn typing_sends_lit_tokens() {
    let (server, client) = setup();
    client.type_text("hi 2&").unwrap();
    client.keypress(Key::Backspace).unwrap();
    client.keypress(Key::Enter).unwrap();
    assert_eq!(
        server.paths("POST"),
        vec![
            "/keypress/Lit_h",
            "/keypress/Lit_i",
            "/keypress/Lit_%20",
            "/keypress/Lit_2",
            "/keypress/Lit_%26",
            "/keypress/Backspace",
            "/keypress/Enter",
        ]
    );
}

#[test]
fn launch_unknown_app_is_a_status_error() {
    let (_server, client) = setup();
    match client.launch("does-not-exist") {
        Err(Error::Status { code: 404, path }) => assert_eq!(path, "/launch/does-not-exist"),
        other => panic!("expected 404, got {other:?}"),
    }
}

#[test]
fn server_errors_surface_as_status() {
    let (server, client) = setup();
    *server.force_status.lock().unwrap() = Some(503);
    match client.keypress(Key::Play) {
        Err(Error::Status { code: 503, .. }) => {}
        other => panic!("expected 503, got {other:?}"),
    }
    match client.device_info() {
        Err(Error::Status { code: 503, .. }) => {}
        other => panic!("expected 503, got {other:?}"),
    }
}

#[test]
fn unreachable_host_is_a_transport_error() {
    // nothing listens here; the connect is refused immediately
    let dead = FakeRoku::start();
    let host = dead.host();
    drop(dead);
    let client = Client::new(&host);
    match client.device_info() {
        Err(Error::Transport(_)) => {}
        other => panic!("expected transport error, got {other:?}"),
    }
}

#[test]
fn icon_is_downloaded_once_then_cached() {
    let _g = ENV_LOCK.lock().unwrap();
    let dir = std::env::temp_dir().join(format!("roku-icons-it-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::env::set_var("ROKU_CACHE_DIR", &dir);

    let (server, client) = setup();
    let p1 = icons::fetch_icon(&client, FAKE_DEVICE_ID, "12").expect("icon");
    assert!(p1.is_file());
    let bytes = std::fs::read(&p1).unwrap();
    assert_eq!(&bytes[1..4], b"PNG");
    assert!(p1.ends_with("icons/MOCK00000001/12.png"));

    let p2 = icons::fetch_icon(&client, FAKE_DEVICE_ID, "12").expect("icon again");
    assert_eq!(p1, p2);
    assert_eq!(
        server.paths("GET"),
        vec!["/query/icon/12"],
        "second call must hit the cache"
    );

    assert!(icons::fetch_icon(&client, FAKE_DEVICE_ID, "nope").is_err());

    std::env::remove_var("ROKU_CACHE_DIR");
    let _ = std::fs::remove_dir_all(&dir);
}

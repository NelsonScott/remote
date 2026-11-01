//! Liveness against the fake Roku: real HTTP, real timeouts, the same
//! `Liveness` calls the UI makes. A short probe timeout keeps it quick.

use std::time::{Duration, Instant};

use remote::ecp::{Client, Error, Key};
use remote::fake_roku::FakeRoku;
use remote::liveness::{Liveness, Status};

const PROBE: Duration = Duration::from_millis(300);
/// longer than PROBE: the fake answers too late, the client times out
const STALL: Duration = Duration::from_millis(900);

fn setup() -> (FakeRoku, Client, Liveness) {
    let server = FakeRoku::start();
    let client = Client::new(&server.host()).with_probe_timeout(PROBE);
    (server, client, Liveness::new())
}

/// One status probe, fed to `lv` exactly as `ui::poll_status` does.
fn probe(client: &Client, lv: &mut Liveness) -> (Status, bool) {
    let started = Instant::now();
    let result = client.device_info();
    let timed_out = matches!(result, Err(Error::Transport(_)));
    (lv.probe_result(&result, started, Instant::now()), timed_out)
}

#[test]
fn probe_timing_out_once_then_recovering_keeps_status() {
    let (server, client, mut lv) = setup();
    server.stall_device_info(1, STALL);
    let (status, timed_out) = probe(&client, &mut lv);
    assert!(timed_out, "the stalled probe must actually time out");
    assert_eq!(status, Status::Online);
    let (status, timed_out) = probe(&client, &mut lv);
    assert!(!timed_out);
    assert_eq!(status, Status::Online);
    assert_eq!(lv.failures(), 0);
}

#[test]
fn three_probe_timeouts_in_a_row_mean_unreachable() {
    let (server, client, mut lv) = setup();
    server.stall_device_info(3, STALL);
    let statuses: Vec<Status> = (0..3).map(|_| probe(&client, &mut lv).0).collect();
    assert_eq!(
        statuses,
        vec![Status::Online, Status::Online, Status::Unreachable]
    );
    // and the TV coming back brings it straight online
    assert_eq!(probe(&client, &mut lv).0, Status::Online);
}

#[test]
fn successful_keypress_after_two_misses_resets() {
    let (server, client, mut lv) = setup();
    server.stall_device_info(2, STALL);
    assert_eq!(probe(&client, &mut lv).0, Status::Online);
    assert_eq!(probe(&client, &mut lv).0, Status::Online);
    assert_eq!(lv.failures(), 2);

    lv.command_sent(Instant::now());
    let r = client.keypress(Key::Home);
    assert!(r.is_ok());
    assert_eq!(lv.command_result(&r, Instant::now()), Status::Online);
    assert_eq!(lv.failures(), 0);

    // a third miss after the reset is only the first of a new run
    server.stall_device_info(1, STALL);
    let (status, timed_out) = probe(&client, &mut lv);
    assert!(timed_out);
    assert_eq!(status, Status::Online);
    assert_eq!(lv.failures(), 1);
}

#[test]
fn slow_launch_still_succeeds_and_mid_launch_probe_does_not_count() {
    let (server, client, mut lv) = setup();
    // the TV holds /launch for 1.2 s and is busy for 2 s after it
    server.busy_on_launch(Duration::from_millis(1200), Duration::from_secs(2), STALL);
    let launch_client = client.clone();
    let probe_client = client.clone();
    let started = Instant::now();
    lv.command_sent(started);
    let launch = std::thread::spawn(move || launch_client.launch("291097"));
    std::thread::sleep(Duration::from_millis(100));
    // a probe that was already overlapping the launch: no strike
    let probe_started = Instant::now() - Duration::from_millis(200);
    let r = probe_client.device_info();
    assert!(matches!(r, Err(Error::Transport(_))), "{r:?}");
    assert_eq!(
        lv.probe_result(&r, probe_started, Instant::now()),
        Status::Online
    );
    assert_eq!(lv.failures(), 0);

    let r = launch.join().unwrap();
    assert!(r.is_ok(), "a 1.2 s launch must not time out: {r:?}");
    assert_eq!(lv.command_result(&r, Instant::now()), Status::Online);
    // right after the launch the probe is debounced
    assert!(!lv.should_probe(Instant::now()));
    assert_eq!(server.paths("POST"), vec!["/launch/291097"]);
}

#[test]
fn a_tv_that_is_gone_ends_up_unreachable() {
    let (server, client, mut lv) = setup();
    server.set_down(Some(Duration::from_secs(2)));
    for _ in 0..3 {
        probe(&client, &mut lv);
    }
    assert_eq!(lv.status(), Status::Unreachable);
    assert!(!lv.offer_search(Instant::now()));
    assert!(lv.offer_search(Instant::now() + Duration::from_secs(15)));
    server.set_down(None);
    assert_eq!(probe(&client, &mut lv).0, Status::Online);
}

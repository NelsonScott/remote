//! Standalone fake Roku for manual poking: `cargo run --bin fake-roku [port]`.
//! Prints each request it receives. Default port 18060.

use std::time::Duration;

fn main() {
    let port: u16 = std::env::args()
        .nth(1)
        .and_then(|p| p.parse().ok())
        .unwrap_or(18060);
    let server = remote::fake_roku::FakeRoku::start_on(port);
    println!(
        "fake roku listening on http://{}  (device-info, apps, icons, keypress, launch)",
        server.host()
    );
    let mut seen = 0;
    loop {
        std::thread::sleep(Duration::from_millis(200));
        let reqs = server.requests();
        for r in &reqs[seen..] {
            println!("{} {}", r.method, r.path);
        }
        seen = reqs.len();
    }
}

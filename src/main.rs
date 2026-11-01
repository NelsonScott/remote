use adw::prelude::*;
use gtk::{gdk, gio, glib};

use remote::ui;

fn main() -> glib::ExitCode {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("remote=info"))
        .init();

    if std::env::var_os("ROKU_MOCK").is_some_and(|v| v != "0" && !v.is_empty()) {
        start_mock();
    }

    let app = adw::Application::builder()
        .application_id(ui::APP_ID)
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.connect_startup(|_| {
        let provider = gtk::CssProvider::new();
        provider.load_from_string(ui::CSS);
        if let Some(display) = gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
    });
    app.connect_activate(ui::build);
    // GTK must not see our own env flags as CLI args
    app.run_with_args::<String>(&[])
}

/// `ROKU_MOCK=1`: run against an in-process fake Roku and keep config and
/// icon cache in a throwaway directory so the real ones are untouched.
fn start_mock() {
    let fake: &'static remote::fake_roku::FakeRoku =
        Box::leak(Box::new(remote::fake_roku::FakeRoku::start()));
    std::env::set_var("ROKU_HOST", fake.host());
    let scratch = std::env::temp_dir().join(format!("roku-mock-{}", std::process::id()));
    if std::env::var_os("ROKU_CONFIG_DIR").is_none() {
        std::env::set_var("ROKU_CONFIG_DIR", scratch.join("config"));
    }
    if std::env::var_os("ROKU_CACHE_DIR").is_none() {
        std::env::set_var("ROKU_CACHE_DIR", scratch.join("cache"));
    }
    eprintln!("ROKU_MOCK: fake Roku at http://{}", fake.host());
    // QA knobs that mimic the real TV misbehaving
    let ms = |name: &str| {
        std::env::var(name)
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .map(std::time::Duration::from_millis)
    };
    if let Some(busy) = ms("ROKU_MOCK_BUSY_MS") {
        // a launch answers late and device-info stalls past the probe timeout
        let launch_delay = ms("ROKU_MOCK_LAUNCH_DELAY_MS").unwrap_or_default();
        fake.busy_on_launch(launch_delay, busy, std::time::Duration::from_secs(4));
        eprintln!(
            "ROKU_MOCK: busy {busy:?} after each launch (launch answers after {launch_delay:?})"
        );
    }
    if let Some(after) = ms("ROKU_MOCK_DOWN_AFTER_MS") {
        // the TV disappears (stops answering anything) after this long
        std::thread::spawn(move || {
            std::thread::sleep(after);
            eprintln!("ROKU_MOCK: going down");
            fake.set_down(Some(std::time::Duration::from_secs(30)));
        });
    }
    if let Some(log_path) = std::env::var_os("ROKU_MOCK_LOG") {
        // tail the recorded requests to a file for headless QA
        let path = std::path::PathBuf::from(log_path);
        std::thread::spawn(move || {
            use std::io::Write;
            let mut seen = 0;
            loop {
                std::thread::sleep(std::time::Duration::from_millis(100));
                let reqs = fake.requests();
                if reqs.len() > seen {
                    if let Ok(mut f) = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&path)
                    {
                        for r in &reqs[seen..] {
                            let ms =
                                r.at.duration_since(std::time::UNIX_EPOCH)
                                    .map(|d| d.as_millis())
                                    .unwrap_or(0);
                            let _ = writeln!(f, "{ms} {} {}", r.method, r.path);
                        }
                    }
                    seen = reqs.len();
                }
            }
        });
    }
}

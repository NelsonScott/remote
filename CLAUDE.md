# Remote: agent notes

A Roku remote for the GNOME desktop. Rust + GTK4 + libadwaita, plain-HTTP ECP
on the LAN. The layout is modelled on the Mac app Roam (its own code and
assets are not used; Roam is PolyForm Noncommercial).

## Stack

- Rust 2021, gtk4-rs 0.11.5 (`v4_18`), libadwaita-rs 0.9.2 (`v1_8`), ureq 3
  (sync, run on `gio::spawn_blocking`), roxmltree, toml/serde, tiny_http for
  the fake server. All versions pinned with `=` in Cargo.toml.
- Name "Remote", app id `xyz.scottnelson.Remote`, binary and crate `remote`.
  Window about 320x720. `ROKU_*` env vars and `fake-roku` refer to the
  device, not the app, and keep their names.
- Plain HTTP ECP only (port 8060). No ecp-session websocket, no private
  listening, no `set-audio-output`: nothing that touches the TV's audio
  routing.

## Commands

No host devel packages needed: `scripts/dev.sh` runs cargo inside the
`org.gnome.Sdk//50` Flatpak with the rust-stable extension.

```sh
scripts/dev.sh test                                # unit + tests/fake_server.rs + tests/liveness.rs
scripts/dev.sh clippy --all-targets -- -D warnings && scripts/dev.sh fmt --check
ROKU_MOCK=1 ROKU_MOCK_LOG=target/reqs.log scripts/dev.sh run   # fake Roku (sandbox /tmp is private: log under target/)
ROKU_HOST=192.168.1.50 scripts/dev.sh run                      # real TV, config not written
# mock QA knobs: ROKU_POLL_MS=2000 (probe interval), ROKU_MOCK_BUSY_MS=6000 +
# ROKU_MOCK_LAUNCH_DELAY_MS=3500 (busy TV after a launch), ROKU_MOCK_DOWN_AFTER_MS=10000
# (TV vanishes); ROKU_MOCK_LOG lines are "<epoch ms> METHOD path"
scripts/install.sh                                 # Flatpak build + flatpak install --user --reinstall
scripts/update-cargo-sources.sh                    # after any Cargo.lock change
packaging/rpm/make-srpm.sh                         # SRPM for COPR
```

Installed app: Flatpak `xyz.scottnelson.Remote` (user), config in
`~/.var/app/xyz.scottnelson.Remote/config/remote/`. Build state lives in
`~/.cache/remote-dev/` (flatpak-builder state, local repo, cargo home).
Running the Flatpak headless needs `env -u WAYLAND_DISPLAY flatpak run
--nosocket=wayland --socket=x11 ...`: with a Wayland socket present,
`fallback-x11` hides X11 and the app cannot open the Xvfb display.

## QA rules ("it compiles" is not "it works")

1. `scripts/dev.sh test`, clippy `-D warnings`, `fmt --check` all clean
   before any commit. CI runs the same in a Fedora container plus a Flatpak
   bundle build (`.github/workflows/ci.yml`).
2. Drive the real UI headless: Xvfb, `GDK_BACKEND=x11 GSK_RENDERER=cairo
   ROKU_MOCK=1 ROKU_MOCK_LOG=<file>`, screenshot with ImageMagick `import
   -window root`, LOOK at the PNG, light and dark
   (`ADW_DEBUG_COLOR_SCHEME=prefer-dark`). Use `xdotool key`, `keydown/keyup`,
   `mousedown/mouseup` to exercise the keyboard map, press-and-hold and the
   text field, then diff the request log against what you expected.
3. Against a real TV only read-only calls (`/query/device-info`,
   `/query/apps`, `/query/icon/<id>`) plus at most ONE harmless keypress
   (`Info`). Never volume, power, inputs, launch or anything audio-related on
   someone's TV during QA.
4. Do not install (`scripts/install.sh`) until the above passes.

## Design notes worth knowing

- All TV commands go through one ordered queue on the main loop
  (`ui::enqueue` -> `pump`), one `spawn_blocking` at a time, so typed
  characters and keydown/keyup never reorder. A transport error drops the
  rest of the queue (silently unless the TV is long gone, see below).
- Liveness (`src/liveness.rs`, pure + unit tested): a busy Roku (launching
  a channel) sits on device-info and even on `/launch` for seconds, so one
  miss is NOT offline. Unreachable after 3 consecutive network failures (or
  2+ spanning 15 s); any HTTP answer, incl. a 2xx command, resets at once.
  Probes are skipped while a command is in flight and for 5 s after one, and
  a probe overlapping a command does not count. "TV not reachable / Search
  Again" toast only when a user action fails after 15 s+ of Unreachable.
  One toast at a time (`show_toast` replaces, never queues). Timeouts: probe
  3 s, keypress 3 s, launch 10 s. The original false "Unreachable" bug: a
  `/launch` POST outlived a 3 s timeout while the TV launched the channel,
  and one error flipped everything offline.
- Press-and-hold uses `gtk::EventControllerLegacy` in the capture phase on
  the button (raw ButtonPress/Release). A second `GestureClick` would be
  denied by GtkButton's own gesture. Keyboard hold ignores auto-repeat
  (tracked in `held`) and sends one keydown then keyup on release; losing
  window focus releases everything.
- The window-level key controller yields to the text entry when it has
  focus, except Escape, which returns focus to the remote. Focus is cleared
  on map so the keyboard drives the remote immediately.
- `Key::Info` is the Roku `*` (Options) button; ECP has no separate Info key.
- Layout: every control is a 72x46 rounded tile (10 px radius) on ONE
  3-column grid with 10 px gaps: [Back Power Home], the D-pad plus with
  "Ok", then Replay/Options/Search, Rew/Play/Fwd, Vol-/Mute/Vol+.
  Constants `TILE_*`, `SECTION_GAP`, `CHANNEL_*` in `ui/mod.rs`.
- Glyphs are drawn with cairo in `ui/glyph.rs` in the widget's CSS `color`
  (purple tiles, white on the coral Power tile), not icon-theme names, so
  Volume -/+ carry a big bold sign next to a small speaker.
- Palette: dark by default regardless of the system theme
  (`ColorScheme::ForceDark`); header menu "Follow System Theme"
  (`win.follow-system-theme`, `follow_system_theme` in config.toml, default
  off) switches to `ColorScheme::Default`. `.dark` is toggled on the window
  from `AdwStyleManager` (charcoal #1f2124, tiles #1a1c1f + hairline edge,
  purple #9b6dff) and also redefines libadwaita's named colours
  (`--window-bg-color`, `--popover-bg-color`, `--dialog-bg-color`, ...) so
  the menu, shortcuts and about sheets are charcoal too. Our widgets set
  `background-image: none` so even a forced `GTK_THEME=Adwaita` cannot paint
  stock button gradients over them. Light uses `@window_bg_color` with
  #f0f0f3 tiles and #6a35d4 glyphs (both pass 4.5:1). Header title = "● name"
  (wraps to 2 lines) plus a device card (TV glyph, name, dot + IP). Device
  chooser = "TVs" radio section in the header menu (`win.device`, target =
  host), fed by every TV that answered device-info during discovery.
- Channel tiles: 62x46 (Roku icons are 4:3) rounded tiles with the name
  under them, 4 per row; `gtk::Stack` (symbolic placeholder -> picture),
  icons fetched sequentially after the grid is shown; the cache key is the
  device-id. The mock's "Live TV" 404s its icon to keep the placeholder
  covered.
- App icon: a generated papercut remote, shipped as a hicolor PNG set
  `data/icons/hicolor/{16..512}/apps/` (Lanczos from `data/icons/source.png`)
  by the Flatpak manifest and the RPM spec. Deliberately NO scalable app SVG
  (it would win at large sizes). The 16 px `-symbolic.svg` (fill-only so GTK
  recolours it) is kept. Keep `<svg` within the first ~200 bytes of any SVG
  icon: flatpak's icon validator sniffs the format and a long leading comment
  fails the export with "Format not recognized".
- Config dir is `~/.config/remote`; a pre-0.2.0 `~/.config/roku/config.toml`
  is copied over once on first start.

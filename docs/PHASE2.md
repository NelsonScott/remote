# Phase 2 notes

Scope (2026-10-31): tray icon and global media keys.
Private listening is out entirely (never touch the TV's audio routing: it
feeds external speakers), and the `ecp-session` websocket is not needed for
anything else, so plain HTTP ECP stays the only transport.

## Tray icon / background mode

GNOME Shell has no tray; the practical options are:

1. **StatusNotifierItem (SNI) over D-Bus** (`org.kde.StatusNotifierItem`),
   which shows up with the AppIndicator / "Tray Icons: Reloaded" extensions.
   Rust crates: `ksni` (sync, simple) or `zbus` + a hand-written interface.
   The menu would carry Power, Mute, Volume +/- and "Show remote". Without
   an extension nothing is displayed, so keep this optional
   (`--tray` flag or a setting) and never require it.
2. **Background portal** (`org.freedesktop.portal.Background`,
   `ashpd` crate) to legitimately keep running after the window closes
   (needed if we ever ship as a Flatpak). Hide on close instead of quitting
   (`window.set_hide_on_close(true)`), present again on activation: the app
   is already `adw::Application`, so a second `roku` launch would just call
   `activate` if we drop `NON_UNIQUE` (currently set so headless QA can run
   next to a desktop instance; switch to default flags for the tray build).
3. A GNOME Shell extension is the "native" route but is a separate
   JavaScript project; not worth it for a one-user app.

Recommendation: SNI via `ksni` behind a `--tray` flag plus hide-on-close.

## Global media keys

GNOME grabs `XF86AudioPlay`/`Pause`/`Next`/`Prev`/`Raise`/`Lower`/`Mute`
itself, so a GTK key controller never sees them while unfocused. Options:

1. **MPRIS** (`org.mpris.MediaPlayer2.roku` on the session bus). GNOME routes
   the media keys (and the lock-screen / quick-settings media controls) to
   the most recent MPRIS player. Implement `Play/Pause/Next/Previous` ->
   Roku `Play`/`Fwd`/`Rev`, `Volume` property -> VolumeUp/Down steps (Roku
   has no absolute volume; scripts need the same workaround). Crate:
   `mpris-server` (zbus based). Caveat: Roku would compete with real players
   (Firefox, Spotify) for "most recent"; a toggle in the menu ("Take media
   keys") that registers/unregisters the MPRIS object solves it.
2. **GlobalShortcuts portal** (`org.freedesktop.portal.GlobalShortcuts`,
   GNOME 48+, `ashpd` crate): the user binds shortcuts in Settings once, the
   portal delivers `Activated`/`Deactivated` signals (so press-and-hold works
   via keydown/keyup). Good for Volume +/- on, say, Super+F11/F12 without
   stealing the media keys from real players.
3. `gsettings` custom keybindings calling `roku --press VolumeUp`: trivial but
   spawns a process per press (fast enough: the binary starts in ~30 ms and
   `ROKU_HOST` skips discovery). Would need a tiny CLI mode in `main.rs`
   (`roku --press <Key>` -> cached host -> one keypress -> exit). Cheapest
   first step and useful for launchers and scripts anyway.

Recommendation: add the `--press <Key>` CLI first (also handy for scripts),
then MPRIS with an opt-in toggle, then the GlobalShortcuts portal for
volume if hold-to-repeat from the keyboard is wanted.

## Smaller follow-ups spotted during milestone 1

- ~~Window title truncates long device names~~: done 2026-10-31 (wrapping
  title, status dot moved into the status line).
- `/query/active-app` is parsed but not shown; a one-line "Now: YouTube"
  under the header would be cheap (poll alongside device-info).
- Settings: pick a device when several Rokus answer SSDP (today: the
  previously used USN wins, else the first that answers device-info).
- Flatpak manifest, metainfo and COPR RPM exist (2026-10-31, see README
  Packaging). Flathub submission still needs: a public repo URL (manifest
  `dir` source -> `git` source with a tag + commit), screenshots in the
  metainfo, the Background portal if tray mode lands, and a final name
  (trademark: "Roku" is Roku, Inc.'s).
- Discovery waits the full SSDP timeout even after the sweep found a TV
  (about 4 s on a first run with a strict firewall). Could return early
  when exactly one device answered.
- A tray build under Flatpak needs `--talk-name=org.kde.StatusNotifierWatcher`;
  MPRIS needs `--own-name=org.mpris.MediaPlayer2.roku`.

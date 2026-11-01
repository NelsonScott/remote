//! The remote window: header, button grid, text entry, app grid, keyboard
//! handling and the connection state machine.

pub mod glyph;
pub mod keys;

use std::cell::{Cell, RefCell};
use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use adw::prelude::*;
use gtk::{gdk, gio, glib};

use crate::config::{Config, SavedDevice};
use crate::discovery;
use crate::ecp::{App, Client, DeviceInfo, Key};
use crate::icons;
use crate::liveness::{Liveness, Status};

use glyph::Glyph;

/// Remote tile geometry (Roam proportions): every control is a rounded
/// rectangle on one 3-column grid with equal gaps.
const TILE_W: i32 = 72;
const TILE_H: i32 = 46;
const TILE_GAP: i32 = 10;
/// Space between the remote's sections (top row, D-pad, 3x3 block).
const SECTION_GAP: i32 = 18;
/// Channel tiles keep the Roku icon aspect (290x218, about 4:3).
const CHANNEL_W: i32 = 62;
const CHANNEL_H: i32 = 46;

pub const APP_ID: &str = "xyz.scottnelson.Remote";
const STATUS_POLL: Duration = Duration::from_secs(10);

/// Liveness probe interval; `ROKU_POLL_MS` shortens it for headless QA.
fn status_poll() -> Duration {
    std::env::var("ROKU_POLL_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|ms| *ms >= 200)
        .map(Duration::from_millis)
        .unwrap_or(STATUS_POLL)
}

/// One unit of work for the TV, executed strictly in order.
#[derive(Debug, Clone)]
enum Cmd {
    Press(Key),
    Down(Key),
    Up(Key),
    Lit(char),
    Launch(String),
}

struct State {
    window: adw::ApplicationWindow,
    device: DeviceView,
    /// TVs offered in the header menu's device chooser
    devices: RefCell<Vec<KnownDevice>>,
    devices_menu: gio::Menu,
    device_action: gio::SimpleAction,
    toasts: adw::ToastOverlay,
    entry: gtk::Entry,
    apps_box: gtk::FlowBox,
    apps_status: gtk::Label,

    client: RefCell<Option<Client>>,
    device_key: RefCell<String>,
    /// what the dot and card show (Online / Unreachable or not connected)
    online: Cell<bool>,
    connecting: Cell<bool>,
    /// failure counting / busy debounce for the connected TV
    liveness: RefCell<Liveness>,
    /// a status probe is in flight
    probing: Cell<bool>,
    /// card status line while online ("192.168.1.221", "... · Standby")
    status_text: RefCell<String>,
    /// keys currently held (keyboard or mouse) and sent as keydown
    held: RefCell<HashSet<Key>>,
    queue: RefCell<VecDeque<Cmd>>,
    pumping: Cell<bool>,
    last_text: RefCell<String>,
    /// the one toast on screen, if any (toasts never queue up)
    toast: RefCell<Option<adw::Toast>>,
    /// bumped on every (re)connect so stale app loads are dropped
    generation: Cell<u32>,
}

type Shared = Rc<State>;

pub fn build(app: &adw::Application) {
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Remote")
        .default_width(320)
        .default_height(720)
        .width_request(300)
        .height_request(420)
        .build();
    window.add_css_class("remote-window");
    // Roam is dark: force it unless the user opted into the system scheme.
    // Our own palette (charcoal) is keyed off `.dark` on the window.
    let follow_system = Config::load().follow_system_theme;
    apply_color_scheme(follow_system);
    let style = adw::StyleManager::default();
    sync_dark(&window, style.is_dark());
    {
        let weak = window.downgrade();
        style.connect_dark_notify(move |sm| {
            if let Some(w) = weak.upgrade() {
                sync_dark(&w, sm.is_dark());
            }
        });
    }

    // --- header -----------------------------------------------------------
    let device = DeviceView::new();
    let header = adw::HeaderBar::builder()
        .title_widget(&device.title)
        .build();

    let devices_menu = gio::Menu::new();
    let menu = gio::Menu::new();
    menu.append_section(Some("TVs"), &devices_menu);
    let search_section = gio::Menu::new();
    search_section.append(Some("Search for TVs Again"), Some("win.rediscover"));
    menu.append_section(None, &search_section);
    let appearance = gio::Menu::new();
    appearance.append(
        Some("_Follow System Theme"),
        Some("win.follow-system-theme"),
    );
    menu.append_section(None, &appearance);
    let misc = gio::Menu::new();
    misc.append(Some("_Keyboard Shortcuts"), Some("win.shortcuts"));
    misc.append(Some("_About Remote"), Some("win.about"));
    menu.append_section(None, &misc);
    let menu_button = gtk::MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .menu_model(&menu)
        .tooltip_text("Menu")
        .build();
    header.pack_end(&menu_button);
    let device_action =
        gio::SimpleAction::new_stateful("device", Some(glib::VariantTy::STRING), &"".to_variant());

    // --- body -------------------------------------------------------------
    let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    body.set_margin_top(2);
    body.set_margin_bottom(16);
    body.set_margin_start(16);
    body.set_margin_end(16);

    let entry = gtk::Entry::builder()
        .placeholder_text("Type on the TV")
        .primary_icon_name("input-keyboard-symbolic")
        .tooltip_text(
            "Characters are sent as you type. Enter submits, Escape returns focus to the remote.",
        )
        .css_classes(["tv-entry"])
        .margin_top(SECTION_GAP + 4)
        .build();

    let apps_box = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .homogeneous(true)
        .min_children_per_line(4)
        .max_children_per_line(4)
        .column_spacing(6)
        .row_spacing(12)
        .can_focus(false)
        .margin_top(16)
        .css_classes(["channels"])
        .build();
    let apps_status = gtk::Label::builder()
        .label("")
        .css_classes(["dim-label", "caption"])
        .wrap(true)
        .justify(gtk::Justification::Center)
        .build();

    let toasts = adw::ToastOverlay::new();
    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .propagate_natural_height(true)
        .child(&body)
        .vexpand(true)
        .build();
    toasts.set_child(Some(&scroller));

    let toolbar = adw::ToolbarView::new();
    toolbar.set_top_bar_style(adw::ToolbarStyle::Flat);
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&toasts));
    window.set_content(Some(&toolbar));

    let state: Shared = Rc::new(State {
        window: window.clone(),
        device,
        devices: RefCell::new(Vec::new()),
        devices_menu,
        device_action,
        toasts,
        entry: entry.clone(),
        apps_box: apps_box.clone(),
        apps_status: apps_status.clone(),
        client: RefCell::new(None),
        device_key: RefCell::new(String::new()),
        online: Cell::new(false),
        connecting: Cell::new(false),
        liveness: RefCell::new(Liveness::new()),
        probing: Cell::new(false),
        status_text: RefCell::new(String::new()),
        held: RefCell::new(HashSet::new()),
        queue: RefCell::new(VecDeque::new()),
        pumping: Cell::new(false),
        last_text: RefCell::new(String::new()),
        toast: RefCell::new(None),
        generation: Cell::new(0),
    });

    body.append(&state.device.card);

    // Roam's remote: [Back Power Home], the D-pad plus, then a 3x3 block.
    let remote = gtk::Box::new(gtk::Orientation::Vertical, SECTION_GAP);
    remote.set_halign(gtk::Align::Center);
    remote.set_margin_top(SECTION_GAP + 4);
    remote.append(&build_grid(
        &state,
        &[[
            tile(
                Key::Back,
                Face::Glyph(Glyph::Back),
                "Back (Backspace / Esc)",
            ),
            tile(Key::Power, Face::Glyph(Glyph::Power), "Power"),
            tile(Key::Home, Face::Glyph(Glyph::Home), "Home (h)"),
        ]],
    ));
    remote.append(&build_dpad(&state));
    remote.append(&build_grid(
        &state,
        &[
            [
                tile(
                    Key::InstantReplay,
                    Face::Glyph(Glyph::Replay),
                    "Instant Replay (r)",
                ),
                tile(Key::Info, Face::Glyph(Glyph::Options), "Options * (i or *)"),
                tile(Key::Search, Face::Glyph(Glyph::Search), "Search (/)"),
            ],
            [
                tile(Key::Rev, Face::Glyph(Glyph::Rewind), "Rewind (,)"),
                tile(
                    Key::Play,
                    Face::Glyph(Glyph::PlayPause),
                    "Play / Pause (space)",
                ),
                tile(Key::Fwd, Face::Glyph(Glyph::Forward), "Fast Forward (.)"),
            ],
            [
                tile(
                    Key::VolumeDown,
                    Face::Glyph(Glyph::VolumeDown),
                    "Volume Down (-)",
                ),
                tile(Key::VolumeMute, Face::Glyph(Glyph::Mute), "Mute (m)"),
                tile(Key::VolumeUp, Face::Glyph(Glyph::VolumeUp), "Volume Up (+)"),
            ],
        ],
    ));
    body.append(&remote);
    body.append(&entry);
    body.append(&apps_box);
    body.append(&apps_status);

    setup_entry(&state);
    setup_actions(&state, follow_system);
    setup_keyboard(&state);

    // GTK focuses the first focusable child (the entry) on map; we want the
    // keyboard to drive the remote until the user clicks into the field.
    window.connect_map(|w| {
        let w = w.clone();
        glib::idle_add_local_once(move || GtkWindowExt::set_focus(&w, None::<&gtk::Widget>));
    });
    window.present();

    // periodic liveness probe
    {
        let s = state.clone();
        glib::timeout_add_local(status_poll(), move || {
            poll_status(&s);
            glib::ControlFlow::Continue
        });
    }
    connect(&state, Target::Auto);
}

/// Off: always dark (Roam). On: follow the GNOME light/dark preference.
fn apply_color_scheme(follow_system: bool) {
    adw::StyleManager::default().set_color_scheme(if follow_system {
        adw::ColorScheme::Default
    } else {
        adw::ColorScheme::ForceDark
    });
}

fn sync_dark(window: &adw::ApplicationWindow, dark: bool) {
    if dark {
        window.add_css_class("dark");
    } else {
        window.remove_css_class("dark");
    }
}

// --- device title + card ---------------------------------------------------

fn status_dot() -> gtk::Box {
    let dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    dot.add_css_class("status-dot");
    dot.add_css_class("offline");
    dot.set_valign(gtk::Align::Center);
    dot.set_halign(gtk::Align::Center);
    dot.set_tooltip_text(Some("Offline"));
    dot
}

/// Roam shows the TV twice: "● name" centred above the controls (our header
/// title, which wraps instead of ellipsizing like adw::WindowTitle) and a
/// device card (TV glyph, name, dot + address) in its sidebar, which in our
/// narrow window sits on top of the remote.
struct DeviceView {
    title: gtk::Box,
    title_name: gtk::Label,
    card: gtk::Box,
    card_name: gtk::Label,
    status: gtk::Label,
    dots: [gtk::Box; 2],
}

impl DeviceView {
    fn new() -> Self {
        let title_dot = status_dot();
        let title_name = gtk::Label::builder()
            .label("Roku")
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .lines(2)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .justify(gtk::Justification::Center)
            .max_width_chars(22)
            .css_classes(["device-name"])
            .build();
        let title = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        title.set_valign(gtk::Align::Center);
        title.add_css_class("device-title");
        title.append(&title_dot);
        title.append(&title_name);

        let icon = gtk::Image::builder()
            .icon_name("tv-symbolic")
            .pixel_size(30)
            .css_classes(["card-icon"])
            .build();
        let card_name = gtk::Label::builder()
            .label("Roku")
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes(["card-name"])
            .build();
        let card_dot = status_dot();
        let status = gtk::Label::builder()
            .label("Connecting…")
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes(["card-status"])
            .build();
        let status_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        status_row.append(&card_dot);
        status_row.append(&status);
        let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
        text.set_valign(gtk::Align::Center);
        text.set_hexpand(true);
        text.append(&card_name);
        text.append(&status_row);
        let card = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        card.add_css_class("device-card");
        card.append(&icon);
        card.append(&text);

        DeviceView {
            title,
            title_name,
            card,
            card_name,
            status,
            dots: [title_dot, card_dot],
        }
    }

    fn name(&self) -> glib::GString {
        self.title_name.label()
    }

    fn status(&self) -> glib::GString {
        self.status.label()
    }

    fn set_name(&self, text: &str) {
        for l in [&self.title_name, &self.card_name] {
            l.set_label(text);
            l.set_tooltip_text(Some(text));
        }
    }

    fn set_status(&self, text: &str) {
        self.status.set_label(text);
    }

    fn set_online(&self, online: bool) {
        for dot in &self.dots {
            if online {
                dot.remove_css_class("offline");
                dot.add_css_class("online");
                dot.set_tooltip_text(Some("Online"));
            } else {
                dot.remove_css_class("online");
                dot.add_css_class("offline");
                dot.set_tooltip_text(Some("Offline"));
            }
        }
    }
}

// --- remote tiles ----------------------------------------------------------

/// What a tile shows: a drawn line glyph or a short text ("Ok").
#[derive(Clone, Copy)]
enum Face {
    Glyph(Glyph),
    Text(&'static str),
}

#[derive(Clone, Copy)]
struct TileSpec {
    key: Key,
    face: Face,
    tooltip: &'static str,
}

const fn tile(key: Key, face: Face, tooltip: &'static str) -> TileSpec {
    TileSpec { key, face, tooltip }
}

/// One rounded-rectangle remote control, wired to its key.
fn remote_tile(state: &Shared, spec: TileSpec) -> gtk::Button {
    let button = gtk::Button::builder()
        .tooltip_text(spec.tooltip)
        .focusable(false)
        .focus_on_click(false)
        .width_request(TILE_W)
        .height_request(TILE_H)
        .css_classes(["tile"])
        .build();
    match spec.face {
        Face::Glyph(g) => button.set_child(Some(&glyph::widget(g))),
        Face::Text(t) => button.set_child(Some(
            &gtk::Label::builder()
                .label(t)
                .css_classes(["tile-text"])
                .build(),
        )),
    }
    if spec.key == Key::Power {
        button.add_css_class("power");
    }
    bind_key(state, &button, spec.key);
    button
}

/// Wires a button to a key: repeatable keys use press-and-hold
/// (keydown on press, keyup on release), the rest a single keypress.
fn bind_key(state: &Shared, button: &gtk::Button, key: Key) {
    if key.is_repeatable() {
        let legacy = gtk::EventControllerLegacy::new();
        legacy.set_propagation_phase(gtk::PropagationPhase::Capture);
        let s = state.clone();
        legacy.connect_event(move |_, event| {
            use gdk::EventType as T;
            let is_primary = event
                .downcast_ref::<gdk::ButtonEvent>()
                .map(|b| b.button() == gdk::BUTTON_PRIMARY)
                .unwrap_or(false);
            let kind = event.event_type();
            if (kind == T::ButtonPress && is_primary) || kind == T::TouchBegin {
                key_down(&s, key);
            } else if (kind == T::ButtonRelease && is_primary)
                || matches!(kind, T::TouchEnd | T::TouchCancel | T::GrabBroken)
            {
                key_up(&s, key);
            }
            glib::Propagation::Proceed
        });
        button.add_controller(legacy);
    } else {
        let s = state.clone();
        button.connect_clicked(move |_| enqueue(&s, Cmd::Press(key)));
    }
}

fn remote_grid() -> gtk::Grid {
    let grid = gtk::Grid::builder()
        .row_spacing(TILE_GAP)
        .column_spacing(TILE_GAP)
        .column_homogeneous(true)
        .halign(gtk::Align::Center)
        .build();
    grid.add_css_class("remote-grid");
    grid
}

/// Full rows of three tiles on the shared column grid.
fn build_grid(state: &Shared, rows: &[[TileSpec; 3]]) -> gtk::Grid {
    let grid = remote_grid();
    for (r, row) in rows.iter().enumerate() {
        for (c, spec) in row.iter().enumerate() {
            grid.attach(&remote_tile(state, *spec), c as i32, r as i32, 1, 1);
        }
    }
    grid
}

/// The D-pad: five tiles in a plus on the same three columns, "Ok" centre.
fn build_dpad(state: &Shared) -> gtk::Grid {
    let grid = remote_grid();
    grid.add_css_class("dpad");
    let cells = [
        (tile(Key::Up, Face::Glyph(Glyph::Up), "Up"), 1, 0),
        (tile(Key::Left, Face::Glyph(Glyph::Left), "Left"), 0, 1),
        (tile(Key::Select, Face::Text("Ok"), "Select (Enter)"), 1, 1),
        (tile(Key::Right, Face::Glyph(Glyph::Right), "Right"), 2, 1),
        (tile(Key::Down, Face::Glyph(Glyph::Down), "Down"), 1, 2),
    ];
    for (spec, c, r) in cells {
        grid.attach(&remote_tile(state, spec), c, r, 1, 1);
    }
    grid
}

// --- text entry ------------------------------------------------------------

fn setup_entry(state: &Shared) {
    let s = state.clone();
    state.entry.connect_changed(move |e| {
        let new_text = e.text().to_string();
        let old_text = s.last_text.replace(new_text.clone());
        if old_text == new_text {
            return;
        }
        let old: Vec<char> = old_text.chars().collect();
        let new: Vec<char> = new_text.chars().collect();
        let common = old
            .iter()
            .zip(new.iter())
            .take_while(|(a, b)| a == b)
            .count();
        for _ in common..old.len() {
            enqueue(&s, Cmd::Press(Key::Backspace));
        }
        for c in &new[common..] {
            enqueue(&s, Cmd::Lit(*c));
        }
    });
    let s = state.clone();
    state.entry.connect_activate(move |e| {
        enqueue(&s, Cmd::Press(Key::Enter));
        // the TV field has the text now; clear ours without echoing deletes
        s.last_text.replace(String::new());
        e.set_text("");
    });
}

// --- actions, menu, dialogs ------------------------------------------------

fn setup_actions(state: &Shared, follow_system: bool) {
    // header menu check item; persisted in config.toml
    let theme =
        gio::SimpleAction::new_stateful("follow-system-theme", None, &follow_system.to_variant());
    theme.connect_activate(|action, _| {
        let on = !action
            .state()
            .and_then(|v| v.get::<bool>())
            .unwrap_or(false);
        action.set_state(&on.to_variant());
        apply_color_scheme(on);
        if let Err(e) = Config::update(|c| c.follow_system_theme = on) {
            log::warn!("could not save theme setting: {e}");
        }
    });
    state.window.add_action(&theme);

    let s = state.clone();
    let rediscover = gio::SimpleAction::new("rediscover", None);
    rediscover.connect_activate(move |_, _| connect(&s, Target::Search));
    state.window.add_action(&rediscover);

    // device chooser: radio items in the header menu, target = host
    let s = state.clone();
    state.device_action.connect_activate(move |action, param| {
        let Some(host) = param.and_then(|p| p.get::<String>()) else {
            return;
        };
        let current = action.state().and_then(|v| v.get::<String>());
        if current.as_deref() != Some(host.as_str()) || !s.online.get() {
            connect(&s, Target::Host(host));
        }
    });
    state.window.add_action(&state.device_action);

    let s = state.clone();
    let shortcuts = gio::SimpleAction::new("shortcuts", None);
    shortcuts.connect_activate(move |_, _| show_shortcuts(&s));
    state.window.add_action(&shortcuts);

    let s = state.clone();
    let about = gio::SimpleAction::new("about", None);
    about.connect_activate(move |_, _| show_about(&s));
    state.window.add_action(&about);

    let s = state.clone();
    let press = gio::SimpleAction::new("press", Some(glib::VariantTy::STRING));
    press.connect_activate(move |_, param| {
        if let Some(name) = param.and_then(|p| p.get::<String>()) {
            match Key::parse(&name) {
                Some(k) => enqueue(&s, Cmd::Press(k)),
                None => log::warn!("win.press: unknown key {name}"),
            }
        }
    });
    state.window.add_action(&press);

    if let Some(app) = state.window.application() {
        app.set_accels_for_action("win.shortcuts", &["<Control>question"]);
        app.set_accels_for_action("window.close", &["<Control>w", "<Control>q"]);
    }
}

fn show_shortcuts(state: &Shared) {
    let dialog = adw::ShortcutsDialog::new();
    let section = adw::ShortcutsSection::new(Some("Remote (while the text field is not focused)"));
    for (title, accel) in keys::SHORTCUT_ROWS {
        section.add(adw::ShortcutsItem::new(title, accel));
    }
    dialog.add(section);
    dialog.present(Some(&state.window));
}

fn show_about(state: &Shared) {
    let about = adw::AboutDialog::builder()
        .application_name("Remote")
        .application_icon(APP_ID)
        .developer_name("Scott Nelson")
        .version(env!("CARGO_PKG_VERSION"))
        .comments("A native GNOME remote for Roku TVs. Talks plain HTTP ECP on your LAN; nothing leaves the network.")
        .license_type(gtk::License::MitX11)
        .build();
    let name = state.device.name();
    if !name.is_empty() && name != "Roku" {
        let host = state
            .client
            .borrow()
            .as_ref()
            .map(|c| c.host().to_string())
            .unwrap_or_default();
        about.set_debug_info(&format!(
            "Device: {name}\nECP: {host}\nStatus: {}",
            state.device.status()
        ));
    }
    about.present(Some(&state.window));
}

// --- keyboard --------------------------------------------------------------

fn entry_has_focus(state: &State) -> bool {
    match GtkWindowExt::focus(&state.window) {
        Some(w) => w == state.entry || w.is_ancestor(&state.entry),
        None => false,
    }
}

fn setup_keyboard(state: &Shared) {
    let controller = gtk::EventControllerKey::new();
    controller.set_propagation_phase(gtk::PropagationPhase::Capture);

    let s = state.clone();
    controller.connect_key_pressed(move |_, keyval, _code, modifiers| {
        let blocked = gdk::ModifierType::CONTROL_MASK
            | gdk::ModifierType::ALT_MASK
            | gdk::ModifierType::SUPER_MASK
            | gdk::ModifierType::META_MASK;
        if modifiers.intersects(blocked) {
            return glib::Propagation::Proceed;
        }
        if entry_has_focus(&s) {
            if keyval == gdk::Key::Escape {
                GtkWindowExt::set_focus(&s.window, None::<&gtk::Widget>);
                return glib::Propagation::Stop;
            }
            return glib::Propagation::Proceed;
        }
        let Some(key) = keys::map_keyval(keyval) else {
            return glib::Propagation::Proceed;
        };
        if s.held.borrow().contains(&key) {
            // X11/Wayland key auto-repeat; the TV repeats on its own
            return glib::Propagation::Stop;
        }
        if key.is_repeatable() {
            key_down(&s, key);
        } else {
            s.held.borrow_mut().insert(key);
            enqueue(&s, Cmd::Press(key));
        }
        glib::Propagation::Stop
    });

    let s = state.clone();
    controller.connect_key_released(move |_, keyval, _code, _modifiers| {
        if let Some(key) = keys::map_keyval(keyval) {
            if key.is_repeatable() {
                key_up(&s, key);
            } else {
                s.held.borrow_mut().remove(&key);
            }
        }
    });
    state.window.add_controller(controller);

    // if the window loses focus mid-hold, release everything
    let s = state.clone();
    state.window.connect_is_active_notify(move |w| {
        if !w.is_active() {
            release_all(&s);
        }
    });
}

fn key_down(state: &Shared, key: Key) {
    if state.held.borrow_mut().insert(key) {
        enqueue(state, Cmd::Down(key));
    }
}

fn key_up(state: &Shared, key: Key) {
    if state.held.borrow_mut().remove(&key) {
        enqueue(state, Cmd::Up(key));
    }
}

fn release_all(state: &Shared) {
    let held: Vec<Key> = state.held.borrow_mut().drain().collect();
    for k in held {
        if k.is_repeatable() {
            enqueue(state, Cmd::Up(k));
        }
    }
}

// --- command queue ---------------------------------------------------------

fn enqueue(state: &Shared, cmd: Cmd) {
    state.queue.borrow_mut().push_back(cmd);
    if !state.pumping.get() {
        state.pumping.set(true);
        let s = state.clone();
        glib::spawn_future_local(async move { pump(s).await });
    }
}

async fn pump(state: Shared) {
    loop {
        let next = state.queue.borrow_mut().pop_front();
        let Some(cmd) = next else { break };
        let Some(client) = state.client.borrow().clone() else {
            state.queue.borrow_mut().clear();
            show_toast(&state, "Not connected to a TV", true);
            break;
        };
        log::debug!("-> {cmd:?}");
        let label = format!("{cmd:?}");
        state.liveness.borrow_mut().command_sent(Instant::now());
        let result = gio::spawn_blocking(move || match cmd {
            Cmd::Press(k) => client.keypress(k),
            Cmd::Down(k) => client.keydown(k),
            Cmd::Up(k) => client.keyup(k),
            Cmd::Lit(c) => client.type_char(c),
            Cmd::Launch(id) => client.launch(&id),
        })
        .await;
        let Ok(result) = result else {
            log::error!("{label}: worker panicked");
            continue;
        };
        let now = Instant::now();
        // a 2xx (or any HTTP answer) is proof of life; a network error is
        // one strike, not "offline"
        let status = state.liveness.borrow_mut().command_result(&result, now);
        apply_status(&state, status);
        match result {
            Ok(()) => {}
            Err(e @ crate::ecp::Error::Transport(_)) => {
                log::warn!(
                    "{label} failed ({} in a row): {e}",
                    state.liveness.borrow().failures()
                );
                // drop the rest; the user can retry
                state.queue.borrow_mut().clear();
                // a transient miss shows nothing; only a TV that has been
                // gone for a while gets the Search Again offer
                if state.liveness.borrow().offer_search(now) {
                    show_toast(&state, "TV not reachable", true);
                }
            }
            Err(e) => {
                log::warn!("{label} failed: {e}");
                show_toast(&state, &format!("TV rejected {label}: {e}"), false);
            }
        }
    }
    state.pumping.set(false);
}

fn toast_text(toast: &adw::Toast) -> String {
    toast
        .custom_title()
        .and_downcast::<gtk::Label>()
        .map(|l| l.label().to_string())
        .or_else(|| toast.title().map(|t| t.to_string()))
        .unwrap_or_default()
}

/// Shows `msg`, replacing (never queueing behind) a toast already on screen.
fn show_toast(state: &Shared, msg: &str, offer_search: bool) {
    let old = state.toast.borrow_mut().take();
    if let Some(old) = old {
        if toast_text(&old) == msg {
            state.toast.replace(Some(old));
            return;
        }
        old.dismiss();
    }
    let toast = adw::Toast::new(msg);
    toast.set_timeout(3);
    // the window is ~320 px wide: let the text wrap instead of ellipsizing
    toast.set_custom_title(Some(
        &gtk::Label::builder()
            .label(msg)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .max_width_chars(14)
            .xalign(0.0)
            .css_classes(["toast-title"])
            .build(),
    ));
    if offer_search {
        toast.set_button_label(Some("Search Again"));
        toast.set_action_name(Some("win.rediscover"));
    }
    let weak = Rc::downgrade(state);
    toast.connect_dismissed(move |t| {
        if let Some(s) = weak.upgrade() {
            let mine = s.toast.borrow().as_ref() == Some(t);
            if mine {
                s.toast.replace(None);
            }
        }
    });
    state.toast.replace(Some(toast.clone()));
    state.toasts.add_toast(toast);
}

// --- connection state ------------------------------------------------------

/// Reflect the liveness verdict in the dot and the card.
fn apply_status(state: &State, status: Status) {
    match status {
        Status::Online => {
            let text = state.status_text.borrow().clone();
            if !state.online.get() || state.device.status() != text {
                if !state.online.get() {
                    log::info!("TV online again");
                }
                set_online(state, true, (!text.is_empty()).then_some(text.as_str()));
            }
        }
        Status::Unreachable => {
            if state.online.get() {
                log::warn!(
                    "TV unreachable after {} failed requests",
                    state.liveness.borrow().failures()
                );
                set_online(state, false, Some("Unreachable"));
            }
        }
    }
}

fn set_online(state: &State, online: bool, status: Option<&str>) {
    state.online.set(online);
    state.device.set_online(online);
    if let Some(text) = status {
        state.device.set_status(text);
    }
}

/// Card status line: Roam shows just the address next to the dot.
fn status_for(info: &DeviceInfo, host: &str) -> String {
    let ip = host.split(':').next().unwrap_or(host);
    if info.is_awake() {
        ip.to_string()
    } else {
        format!("{ip} · Standby")
    }
}

/// A TV the device chooser can switch to.
#[derive(Debug, Clone, PartialEq, Eq)]
struct KnownDevice {
    host: String,
    name: String,
    usn: String,
}

/// What `connect` should look for.
#[derive(Debug, Clone)]
enum Target {
    /// cached host first, then SSDP
    Auto,
    /// SSDP only (menu: Search for TVs Again)
    Search,
    /// a device picked in the chooser
    Host(String),
}

enum Found {
    Device(Box<FoundDevice>),
    Nothing(String),
}

struct FoundDevice {
    client: Client,
    info: DeviceInfo,
    usn: String,
    /// every TV that answered device-info (feeds the chooser); empty keeps
    /// the current list
    all: Vec<KnownDevice>,
}

fn probe(host: &str, usn: &str) -> Result<FoundDevice, String> {
    let client = Client::new(host);
    match client.device_info() {
        Ok(info) => Ok(FoundDevice {
            all: vec![KnownDevice {
                host: client.host().to_string(),
                name: info.display_name(),
                usn: usn.to_string(),
            }],
            client,
            info,
            usn: usn.to_string(),
        }),
        Err(e) => Err(format!("{host}: {e}")),
    }
}

/// Blocking: resolve `target` to a reachable TV.
fn locate(target: Target, known: Vec<KnownDevice>) -> Found {
    let found = |r: Result<FoundDevice, String>| match r {
        Ok(d) => Found::Device(Box::new(d)),
        Err(e) => Found::Nothing(e),
    };
    if let Ok(host) = std::env::var("ROKU_HOST") {
        return found(probe(&host, ""));
    }
    let cfg = Config::load();
    match target {
        Target::Host(host) => {
            let usn = known
                .iter()
                .find(|d| d.host == host)
                .map(|d| d.usn.clone())
                .unwrap_or_default();
            return found(probe(&host, &usn).map(|mut d| {
                d.all = Vec::new(); // keep the chooser list as it is
                d
            }));
        }
        Target::Auto => {
            if let Some(saved) = &cfg.device {
                match probe(&saved.host, &saved.usn) {
                    Ok(d) => return Found::Device(Box::new(d)),
                    Err(e) => log::info!("cached device not answering ({e}); searching"),
                }
            }
        }
        Target::Search => {}
    }
    let devices = match discovery::search(discovery::DEFAULT_TIMEOUT) {
        Ok(d) => d,
        Err(e) => return Found::Nothing(format!("SSDP failed: {e}")),
    };
    if devices.is_empty() {
        return Found::Nothing("No Roku answered on the network".into());
    }
    // prefer the device we knew, else the first that answers device-info;
    // ask every one so the chooser can list them all
    let preferred = cfg
        .device
        .as_ref()
        .map(|d| d.usn.clone())
        .unwrap_or_default();
    let mut ordered: Vec<&discovery::SsdpDevice> = devices.iter().collect();
    ordered.sort_by_key(|d| !(!preferred.is_empty() && d.usn == preferred));
    let mut chosen: Option<FoundDevice> = None;
    let mut all = Vec::new();
    let mut last_err = String::new();
    for dev in ordered {
        match probe(&dev.host, &dev.usn) {
            Ok(mut d) => {
                all.append(&mut d.all);
                if chosen.is_none() {
                    chosen = Some(d);
                }
            }
            Err(e) => last_err = e,
        }
    }
    match chosen {
        Some(mut d) => {
            d.all = all;
            Found::Device(Box::new(d))
        }
        None => Found::Nothing(format!("Found a Roku but device-info failed ({last_err})")),
    }
}

/// Menu labels treat `_` as a mnemonic marker.
fn menu_label(name: &str) -> String {
    name.replace('_', "__")
}

fn rebuild_devices_menu(state: &State) {
    state.devices_menu.remove_all();
    for d in state.devices.borrow().iter() {
        let item = gio::MenuItem::new(Some(&menu_label(&d.name)), None);
        item.set_action_and_target_value(Some("win.device"), Some(&d.host.to_variant()));
        state.devices_menu.append_item(&item);
    }
}

fn connect(state: &Shared, target: Target) {
    if state.connecting.get() {
        return;
    }
    state.connecting.set(true);
    state.device.set_status(match target {
        Target::Search => "Searching…",
        _ => "Connecting…",
    });
    let gen = state.generation.get().wrapping_add(1);
    state.generation.set(gen);
    let known = state.devices.borrow().clone();
    let s = state.clone();
    glib::spawn_future_local(async move {
        let found = gio::spawn_blocking(move || locate(target, known)).await;
        s.connecting.set(false);
        match found {
            Ok(Found::Device(found)) => {
                let FoundDevice {
                    client,
                    info,
                    usn,
                    all,
                } = *found;
                let host = client.host().to_string();
                log::info!(
                    "connected to {} ({} {}) at {host}",
                    info.display_name(),
                    info.model_name,
                    info.software_version
                );
                s.device.set_name(&info.display_name());
                s.liveness.borrow_mut().reset();
                s.status_text.replace(status_for(&info, &host));
                set_online(&s, true, Some(&status_for(&info, &host)));
                s.device_key.replace(info.cache_key());
                s.client.replace(Some(client.clone()));
                {
                    let mut devices = s.devices.borrow_mut();
                    if !all.is_empty() {
                        *devices = all;
                    }
                    if !devices.iter().any(|d| d.host == host) {
                        devices.push(KnownDevice {
                            host: host.clone(),
                            name: info.display_name(),
                            usn: usn.clone(),
                        });
                    }
                }
                rebuild_devices_menu(&s);
                s.device_action.set_state(&host.to_variant());
                if std::env::var_os("ROKU_HOST").is_none() {
                    let saved = SavedDevice {
                        host,
                        name: info.display_name(),
                        usn,
                    };
                    if let Err(e) = Config::update(|c| c.device = Some(saved)) {
                        log::warn!("could not save config: {e}");
                    }
                }
                load_apps(&s, client, gen);
            }
            Ok(Found::Nothing(reason)) => {
                log::warn!("no device: {reason}");
                if s.client.borrow().is_none() {
                    s.device.set_name("No TV found");
                }
                set_online(&s, false, Some("Offline"));
                s.apps_status.set_text(&reason);
                show_toast(&s, "No Roku found on the network", true);
            }
            Err(_) => log::error!("locate panicked"),
        }
    });
}

fn poll_status(state: &Shared) {
    if state.connecting.get() || state.probing.get() {
        return;
    }
    let Some(client) = state.client.borrow().clone() else {
        return;
    };
    // the TV is busy with a command in flight or just sent: it answering
    // slowly now proves nothing, and the command result says it is alive
    if state.pumping.get() || !state.liveness.borrow().should_probe(Instant::now()) {
        log::debug!("status probe skipped: TV busy with a command");
        return;
    }
    state.probing.set(true);
    let gen = state.generation.get();
    let s = state.clone();
    glib::spawn_future_local(async move {
        let host = client.host().to_string();
        let started = Instant::now();
        let result = gio::spawn_blocking(move || client.device_info()).await;
        s.probing.set(false);
        if s.generation.get() != gen {
            return; // reconnected meanwhile
        }
        let Ok(result) = result else { return };
        let before = s.liveness.borrow().failures();
        let status = s
            .liveness
            .borrow_mut()
            .probe_result(&result, started, Instant::now());
        let after = s.liveness.borrow().failures();
        match &result {
            Ok(info) => {
                s.device.set_name(&info.display_name());
                s.status_text.replace(status_for(info, &host));
            }
            Err(e) if after > before => {
                log::info!("status probe failed ({after} in a row): {e}")
            }
            Err(e) => {
                log::info!("status probe failed, ignored (a command went out meanwhile): {e}")
            }
        }
        apply_status(&s, status);
    });
}

// --- app grid --------------------------------------------------------------

fn load_apps(state: &Shared, client: Client, gen: u32) {
    let s = state.clone();
    glib::spawn_future_local(async move {
        s.apps_status.set_text("Loading channels…");
        let c = client.clone();
        let apps = match gio::spawn_blocking(move || c.apps()).await {
            Ok(Ok(apps)) => apps,
            Ok(Err(e)) => {
                s.apps_status
                    .set_text(&format!("Could not load channels: {e}"));
                return;
            }
            Err(_) => return,
        };
        if s.generation.get() != gen {
            return;
        }
        s.apps_box.remove_all();
        let mut tiles = Vec::with_capacity(apps.len());
        for app in &apps {
            let (button, tile) = app_tile(&s, app);
            s.apps_box.insert(&button, -1);
            tiles.push(tile);
        }
        s.apps_status.set_text("");
        let device_key = s.device_key.borrow().clone();
        for (app, tile) in apps.iter().zip(tiles) {
            if s.generation.get() != gen {
                return;
            }
            let c = client.clone();
            let id = app.id.clone();
            let key = device_key.clone();
            let fetched: Result<PathBuf, String> =
                match gio::spawn_blocking(move || icons::fetch_icon(&c, &key, &id)).await {
                    Ok(Ok(p)) => Ok(p),
                    Ok(Err(e)) => Err(e.to_string()),
                    Err(_) => Err("worker panicked".into()),
                };
            match fetched {
                Ok(path) => match gdk::Texture::from_filename(&path) {
                    Ok(texture) => tile.show_icon(&texture),
                    Err(e) => {
                        log::warn!("bad icon {}: {e}", path.display());
                        let _ = std::fs::remove_file(&path);
                    }
                },
                Err(e) => log::info!("icon {}: {e}", app.id),
            }
        }
    });
}

/// Symbolic stand-in shown on the dark tile until (or unless) the TV
/// serves an icon.
fn placeholder_icon(app: &App) -> &'static str {
    let name = app.name.to_ascii_lowercase();
    if ["playstation", "xbox", "nintendo", "switch", "game"]
        .iter()
        .any(|g| name.contains(g))
    {
        "input-gaming-symbolic"
    } else if app.kind == "tvin" {
        "tv-symbolic"
    } else {
        "applications-multimedia-symbolic"
    }
}

/// Roam channel tile: rounded icon with the channel name underneath.
fn app_tile(state: &Shared, app: &App) -> (gtk::Button, AppTile) {
    let picture = gtk::Picture::builder()
        .content_fit(gtk::ContentFit::Cover)
        .can_shrink(true)
        .build();
    let placeholder = gtk::Image::builder()
        .icon_name(placeholder_icon(app))
        .pixel_size(22)
        .css_classes(["channel-placeholder"])
        .build();
    let stack = gtk::Stack::builder()
        .transition_type(gtk::StackTransitionType::Crossfade)
        .transition_duration(150)
        .width_request(CHANNEL_W)
        .height_request(CHANNEL_H)
        .halign(gtk::Align::Center)
        .overflow(gtk::Overflow::Hidden)
        .css_classes(["channel-frame"])
        .build();
    stack.add_named(&placeholder, Some("placeholder"));
    stack.add_named(&picture, Some("icon"));
    stack.set_visible_child_name("placeholder");

    let name = gtk::Label::builder()
        .label(&app.name)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .max_width_chars(8)
        .width_chars(1)
        .xalign(0.5)
        .css_classes(["channel-name"])
        .build();
    let content = gtk::Box::new(gtk::Orientation::Vertical, 5);
    content.append(&stack);
    content.append(&name);

    let button = gtk::Button::builder()
        .child(&content)
        .tooltip_text(&app.name)
        .focusable(false)
        .focus_on_click(false)
        .css_classes(["channel"])
        .build();
    let s = state.clone();
    let id = app.id.clone();
    button.connect_clicked(move |_| enqueue(&s, Cmd::Launch(id.clone())));
    (button, AppTile { stack, picture })
}

struct AppTile {
    stack: gtk::Stack,
    picture: gtk::Picture,
}

impl AppTile {
    fn show_icon(&self, texture: &gdk::Texture) {
        self.picture.set_paintable(Some(texture));
        self.stack.set_visible_child_name("icon");
    }
}

/// Roam palette. Light follows libadwaita surfaces; `.dark` (toggled from
/// AdwStyleManager) switches to Roam's charcoal. Tiles sit just off the
/// ground with a hairline edge, glyphs in Roku purple, Power filled coral.
pub const CSS: &str = r#"
.remote-window { background-color: @window_bg_color; }
.remote-window.dark { background-color: #1f2124; color: #e4e5e7; }
/* Dark = Roam charcoal everywhere, not Adwaita's #222226 family: the named
   colours cascade to the header menu popover, the shortcuts / about dialogs
   (bottom sheets inside this window) and any stock widget. */
.remote-window.dark {
  --window-bg-color: #1f2124;
  --window-fg-color: #e4e5e7;
  --view-bg-color: #1a1c1f;
  --view-fg-color: #e4e5e7;
  --headerbar-bg-color: #1f2124;
  --headerbar-fg-color: #e4e5e7;
  --headerbar-backdrop-color: #1f2124;
  --popover-bg-color: #2a2c30;
  --popover-fg-color: #e4e5e7;
  --dialog-bg-color: #26282c;
  --dialog-fg-color: #e4e5e7;
  --sheet-bg-color: #26282c;
  --sheet-fg-color: #e4e5e7;
  --card-bg-color: #2c2e32;
  --card-fg-color: #e4e5e7;
  --thumbnail-bg-color: #2c2e32;
  --accent-bg-color: #7c4dff;
  --accent-color: #b594ff;
}
.remote-window headerbar { background: none; box-shadow: none; }

.status-dot {
  min-width: 9px;
  min-height: 9px;
  border-radius: 999px;
  background-color: @error_color;
  transition: background-color 200ms;
}
.status-dot.online { background-color: #2fb555; }
.dark .status-dot.online { background-color: #35cd54; }
.device-name { font-weight: 700; font-size: 13px; }

.device-card {
  padding: 10px 14px;
  border-radius: 12px;
  background-color: #ebebef;
}
.dark .device-card { background-color: #2c2e32; }
.card-icon { color: alpha(currentColor, 0.85); }
.card-name { font-weight: 700; font-size: 14px; }
.card-status { font-size: 12px; opacity: 0.75; }

/* background-image/border: none keeps a forced GTK_THEME's button
   gradients from painting over the palette */
.tile, entry.tv-entry, button.channel, .channel-frame, .device-card {
  background-image: none;
  border: none;
  text-shadow: none;
}
.tile {
  padding: 0;
  border-radius: 10px;
  outline: none;
  background-color: #f0f0f3;
  box-shadow: inset 0 0 0 1px #d8d8de;
  color: #6a35d4;
}
.tile:hover { background-color: #e7e7ec; }
.tile:active { background-color: #dcdce3; }
.dark .tile {
  background-color: #1a1c1f;
  box-shadow: inset 0 0 0 1px #33373c;
  color: #9b6dff;
}
.dark .tile:hover { background-color: #24272b; }
.dark .tile:active { background-color: #2d3136; }
.remote-window .tile.power {
  background-color: #f2403f;
  box-shadow: none;
  color: #ffffff;
}
.remote-window .tile.power:hover { background-color: #ff5452; }
.remote-window .tile.power:active { background-color: #d63232; }
.tile-text { font-size: 14px; font-weight: 500; }
.toast-title { font-weight: 700; }

entry.tv-entry {
  min-height: 34px;
  border-radius: 10px;
  background-color: #f0f0f3;
  box-shadow: inset 0 0 0 1px #d8d8de;
}
.dark entry.tv-entry {
  background-color: #1a1c1f;
  box-shadow: inset 0 0 0 1px #33373c;
}

flowbox.channels > flowboxchild { padding: 0; }
button.channel {
  padding: 0;
  background: none;
  box-shadow: none;
  border-radius: 10px;
}
.channel-frame {
  border-radius: 10px;
  background-color: #f0f0f3;
  box-shadow: inset 0 0 0 1px #d8d8de;
}
.dark .channel-frame {
  background-color: #1a1c1f;
  box-shadow: inset 0 0 0 1px #33373c;
}
.channel-placeholder { color: alpha(currentColor, 0.7); }
button.channel:hover .channel-frame { filter: brightness(1.08); }
button.channel:active .channel-frame { filter: brightness(0.9); }
.channel-name { font-size: 12px; font-weight: 400; opacity: 0.85; }
"#;

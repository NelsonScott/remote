//! Keyboard -> remote key mapping (used while the window has focus and the
//! text entry does not).

use gtk::gdk;

use crate::ecp::Key;

/// Maps a GDK keyval to a Roku key. Letters are matched case-insensitively.
pub fn map_keyval(keyval: gdk::Key) -> Option<Key> {
    use gdk::Key as K;
    let k = keyval.to_lower();
    Some(match k {
        K::Up | K::KP_Up => Key::Up,
        K::Down | K::KP_Down => Key::Down,
        K::Left | K::KP_Left => Key::Left,
        K::Right | K::KP_Right => Key::Right,
        K::Return | K::KP_Enter => Key::Select,
        K::BackSpace | K::Escape => Key::Back,
        K::h => Key::Home,
        K::space => Key::Play,
        K::i | K::asterisk | K::KP_Multiply => Key::Info,
        K::plus | K::equal | K::KP_Add => Key::VolumeUp,
        K::minus | K::underscore | K::KP_Subtract => Key::VolumeDown,
        K::m => Key::VolumeMute,
        K::r => Key::InstantReplay,
        K::comma => Key::Rev,
        K::period => Key::Fwd,
        K::slash => Key::Search,
        _ => return None,
    })
}

/// (title, accelerator string in GtkShortcutLabel syntax) rows for the
/// shortcuts dialog. Kept next to `map_keyval` so they cannot drift apart.
pub const SHORTCUT_ROWS: &[(&str, &str)] = &[
    ("Navigate", "Up Down Left Right"),
    ("OK (Select)", "Return"),
    ("Back", "BackSpace Escape"),
    ("Home", "h"),
    ("Play / Pause", "space"),
    ("Options (*)", "i asterisk"),
    ("Instant Replay", "r"),
    ("Rewind / Fast Forward", "comma period"),
    ("Search", "slash"),
    ("Volume Up / Down", "plus minus"),
    ("Mute", "m"),
    ("Leave the text field", "Escape"),
    ("Keyboard Shortcuts", "<Control>question"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_mapping() {
        assert_eq!(map_keyval(gdk::Key::Up), Some(Key::Up));
        assert_eq!(map_keyval(gdk::Key::Return), Some(Key::Select));
        assert_eq!(map_keyval(gdk::Key::BackSpace), Some(Key::Back));
        assert_eq!(map_keyval(gdk::Key::Escape), Some(Key::Back));
        assert_eq!(map_keyval(gdk::Key::h), Some(Key::Home));
        assert_eq!(map_keyval(gdk::Key::H), Some(Key::Home));
        assert_eq!(map_keyval(gdk::Key::space), Some(Key::Play));
        assert_eq!(map_keyval(gdk::Key::i), Some(Key::Info));
        assert_eq!(map_keyval(gdk::Key::asterisk), Some(Key::Info));
        assert_eq!(map_keyval(gdk::Key::plus), Some(Key::VolumeUp));
        assert_eq!(map_keyval(gdk::Key::minus), Some(Key::VolumeDown));
        assert_eq!(map_keyval(gdk::Key::m), Some(Key::VolumeMute));
        assert_eq!(map_keyval(gdk::Key::r), Some(Key::InstantReplay));
        assert_eq!(map_keyval(gdk::Key::q), None);
        assert_eq!(map_keyval(gdk::Key::F5), None);
    }
}

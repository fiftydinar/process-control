//! Finding the windows that belong to processes.
//!
//! A task manager is only useful when its rows read as applications rather
//! than as a list of executables, so every process that owns a window is
//! matched to it. The display server knows which process owns a window two
//! ways: the `_NET_WM_PID` property, which most toolkits set, and `WM_CLASS`,
//! which is the name a desktop entry's `StartupWMClass` refers to. This reads
//! both, so a program is recognised whichever of the two it publishes and
//! however it is packaged: nothing here consults Flatpak, Snap or systemd.
//!
//! Wayland has no equivalent of either property. On a Wayland session this
//! therefore sees the Xwayland windows, which is every X11 program and covers
//! most of what a user has open.

use x11rb::connection::Connection as _;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ConnectionExt as _, GetPropertyReply, Window as Xid,
};
use x11rb::rust_connection::RustConnection;

pub use crate::sample::Window;

/// `_NET_CLIENT_LIST`: the top-level windows the window manager tracks.
const CLIENT_LIST: &str = "_NET_CLIENT_LIST";

/// `_NET_WM_PID`: the process id a window sets on itself.
const WM_PID: &str = "_NET_WM_PID";

/// `_NET_WM_NAME`: the title in UTF-8.
const WM_NAME_UTF8: &str = "_NET_WM_NAME";

/// Whether this is a Wayland session.
///
/// Wayland publishes only the Xwayland windows to other programs, so the
/// list this module reads is partial: a native Wayland window, or a program
/// sitting in the tray, is missing from it.
pub fn on_wayland() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some()
        || std::env::var("XDG_SESSION_TYPE").is_ok_and(|kind| kind.eq_ignore_ascii_case("wayland"))
}

/// Every managed window on the display.
///
/// An empty list is what a session with no X display gives: a Wayland session
/// with no Xwayland, or a machine with no display at all. Neither is an error
/// worth showing, so both are answered the same way.
pub fn windows() -> Vec<Window> {
    let Ok((connection, screen)) = x11rb::connect(None) else {
        return Vec::new();
    };
    read(&connection, screen).unwrap_or_default()
}

/// Reads every window the window manager tracks.
fn read(connection: &RustConnection, screen: usize) -> Option<Vec<Window>> {
    let root = connection.setup().roots.get(screen)?.root;
    let list = intern(connection, CLIENT_LIST)?;
    if list == x11rb::NONE {
        return Some(Vec::new());
    }
    let reply = connection
        .get_property(false, root, list, AtomEnum::WINDOW, 0, 1 << 14)
        .ok()?
        .reply()
        .ok()?;
    Some(
        reply
            .value32()
            .map(|ids| ids.filter_map(|id| window(connection, id)).collect())
            .unwrap_or_default(),
    )
}

/// Reads one window's process, class and title.
fn window(connection: &RustConnection, id: Xid) -> Option<Window> {
    let pid = intern(connection, WM_PID)
        .and_then(|atom| property(connection, id, atom, AtomEnum::CARDINAL))
        .as_deref()
        .and_then(card)
        .unwrap_or(0) as i32;
    let (instance, class) = property(connection, id, AtomEnum::WM_CLASS, AtomEnum::STRING)
        .as_deref()
        .map(split_class)
        .unwrap_or_default();
    Some(Window {
        id,
        pid,
        class,
        instance,
        title: title(connection, id),
    })
}

/// A window's title, preferring the UTF-8 property to the legacy one.
fn title(connection: &RustConnection, id: Xid) -> String {
    // `_NET_WM_NAME` is of type `UTF8_STRING`, not `STRING`: asking for the
    // wrong type makes a server refuse to hand the title over.
    let utf8 = intern(connection, WM_NAME_UTF8);
    let utf8_type = intern(connection, "UTF8_STRING").unwrap_or(AtomEnum::STRING.into());
    if let Some(atom) = utf8
        && atom != x11rb::NONE
        && let Some(value) = property(connection, id, atom, utf8_type)
        && let Ok(text) = String::from_utf8(value)
    {
        return text.trim_end_matches('\0').to_string();
    }
    property(connection, id, AtomEnum::WM_NAME, AtomEnum::ANY)
        .and_then(|value| String::from_utf8(value).ok())
        .map(|text| text.trim_end_matches('\0').to_string())
        .unwrap_or_default()
}

/// Reads one property of `window`, or `None` when it has none.
fn property(
    connection: &RustConnection,
    window: Xid,
    property: impl Into<Atom>,
    kind: impl Into<Atom>,
) -> Option<Vec<u8>> {
    let reply: GetPropertyReply = connection
        .get_property(false, window, property, kind, 0, u32::MAX / 4)
        .ok()?
        .reply()
        .ok()?;
    // A property that does not exist comes back as `None`, not as empty.
    if reply.format == 0 {
        return None;
    }
    Some(reply.value)
}

/// The atom with this name, or `x11rb::NONE` when the server has none.
fn intern(connection: &RustConnection, name: impl AsRef<[u8]>) -> Option<Atom> {
    let atom = connection
        .intern_atom(false, name.as_ref())
        .ok()?
        .reply()
        .ok()?
        .atom;
    Some(atom)
}

/// The first 32-bit value in a property.
fn card(value: &[u8]) -> Option<u32> {
    Some(u32::from_ne_bytes(value.get(..4)?.try_into().ok()?))
}

/// `WM_CLASS` is two NUL-terminated strings: the instance, then the class.
fn split_class(value: &[u8]) -> (String, String) {
    let mut parts = value.split(|byte| *byte == 0);
    let mut next = || {
        parts
            .next()
            .map(|part| String::from_utf8_lossy(part).to_string())
            .unwrap_or_default()
    };
    (next(), next())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_class_property_names_the_instance_and_the_class() {
        let (instance, class) = split_class(b"firefox\0Firefox\0");
        assert_eq!(instance, "firefox");
        assert_eq!(class, "Firefox");
    }

    #[test]
    fn a_class_property_with_only_one_string_still_reads() {
        let (instance, class) = split_class(b"solo\0");
        assert_eq!(instance, "solo");
        assert_eq!(class, "");
    }

    #[test]
    fn the_keys_a_desktop_entry_is_matched_against_cover_both_names() {
        let window = Window {
            id: 1,
            pid: 42,
            class: "Firefox".to_string(),
            instance: "Navigator".to_string(),
            title: "A page".to_string(),
        };
        assert_eq!(window.keys(), ["Firefox", "Navigator"]);
    }

    #[test]
    fn a_repeated_name_is_offered_once() {
        let window = Window {
            class: "kitty".to_string(),
            instance: "kitty".to_string(),
            ..Window::default()
        };
        assert_eq!(window.keys(), ["kitty"]);
    }

    #[test]
    fn an_empty_class_gives_no_keys() {
        assert!(Window::default().keys().is_empty());
    }

    #[test]
    fn a_cardinal_property_is_a_32_bit_number() {
        assert_eq!(card(&[42, 0, 0, 0]), Some(42));
        assert_eq!(card(&[1, 2]), None);
    }

    #[test]
    fn reading_the_display_gives_windows_or_an_empty_list() {
        // CI has no display server, which must not be an error.
        for window in windows() {
            assert!(window.id > 0);
        }
    }
}

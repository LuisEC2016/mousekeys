//! Requests to the niri IPC socket ($NIRI_SOCKET), without spawning
//! `niri msg`: the logical width of the focused monitor, and whether there is
//! a window under the pointer.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(1);
const MAX_REPLY: u64 = 1 << 20;

fn request(req: &[u8]) -> Option<UnixStream> {
    let mut sock = UnixStream::connect(std::env::var_os("NIRI_SOCKET")?).ok()?;
    sock.set_read_timeout(Some(TIMEOUT)).ok()?;
    sock.set_write_timeout(Some(TIMEOUT)).ok()?;
    sock.write_all(req).ok()?;
    Some(sock)
}

fn read_reply(reader: &mut impl BufRead, reply: &mut Vec<u8>) -> io::Result<()> {
    reader.take(MAX_REPLY).read_until(b'\n', reply)?;
    Ok(())
}

pub fn focused_output_width() -> Option<f64> {
    let mut sock = BufReader::new(request(b"\"FocusedOutput\"\n")?);
    let mut reply = Vec::new();
    read_reply(&mut sock, &mut reply).ok()?;
    logical_width(std::str::from_utf8(&reply).ok()?)
}

/// `niri msg pick-window` without a person clicking: niri answers with the
/// window under the pointer when the next button goes down, and swallows that
/// press and its release. It looks with the same `window_under` that decides
/// whether Mod + left or right button becomes "move/resize window", so it says
/// whether such a press would reach the application: over a layer-shell
/// surface (the DMS bar, control center or notifications) or empty space there
/// is no window, and niri lets the press through even with Super held.
pub struct Pick {
    reader: BufReader<UnixStream>,
    reply: Vec<u8>,
}

#[derive(Debug, PartialEq)]
pub enum Picked {
    Window,
    Nothing,
    /// Error reply (a niri without pick-window) or a closed connection.
    Unknown,
}

impl Pick {
    /// Starts the pick. niri sets its grab on an idle callback after reading
    /// the request; a second request on another connection, answered after
    /// the first one was read, keeps the probe button from usually arriving
    /// before the grab. If it still does, `answer` times out and the caller
    /// sends the button again.
    pub fn start() -> Option<Pick> {
        let sock = request(b"\"PickWindow\"\n")?;
        let mut barrier = BufReader::new(request(b"\"Version\"\n")?);
        read_reply(&mut barrier, &mut Vec::new()).ok()?;
        Some(Pick { reader: BufReader::new(sock), reply: Vec::new() })
    }

    /// `None` if niri has not answered within `timeout`.
    pub fn answer(&mut self, timeout: Duration) -> Option<Picked> {
        self.reader.get_ref().set_read_timeout(Some(timeout)).ok()?;
        match read_reply(&mut self.reader, &mut self.reply) {
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => None,
            Err(_) => Some(Picked::Unknown),
            Ok(()) => Some(picked(&self.reply)),
        }
    }
}

/// niri's compact reply: `{"Ok":{"PickedWindow":null}}` or the window object.
pub fn picked(reply: &[u8]) -> Picked {
    const PREFIX: &[u8] = b"{\"Ok\":{\"PickedWindow\":";
    match reply.strip_prefix(PREFIX) {
        Some(rest) if rest.starts_with(b"null}") => Picked::Nothing,
        Some(rest) if rest.starts_with(b"{") => Picked::Window,
        _ => Picked::Unknown,
    }
}

/// Extracts `logical.width` from niri's compact JSON reply. A quote inside a
/// string (make, model) is escaped, so `"logical":{` cannot appear there.
pub fn logical_width(json: &str) -> Option<f64> {
    const LOGICAL: &str = "\"logical\":{";
    const WIDTH: &str = "\"width\":";
    let rest = &json[json.find(LOGICAL)? + LOGICAL.len()..];
    let obj = &rest[..rest.find('}')?];
    let num = &obj[obj.find(WIDTH)? + WIDTH.len()..];
    let end = num
        .find(|c: char| !matches!(c, '0'..='9' | '.' | '-' | '+' | 'e' | 'E'))
        .unwrap_or(num.len());
    num[..end].parse::<f64>().ok().filter(|w| *w > 0.0 && w.is_finite())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_niri_26_04_reply() {
        let r = r#"{"Ok":{"FocusedOutput":{"name":"DP-1","make":"LG Electronics","model":"LG FULL HD","serial":"0x01010101","physical_size":[480,270],"modes":[{"width":1920,"height":1080,"refresh_rate":74973,"is_preferred":true},{"width":720,"height":400,"refresh_rate":70082,"is_preferred":false}],"current_mode":0,"is_custom_mode":false,"vrr_supported":false,"vrr_enabled":false,"logical":{"x":1920,"y":0,"width":1920,"height":1080,"scale":1.0,"transform":"Normal"}}}}"#;
        assert_eq!(logical_width(r), Some(1920.0));
    }

    #[test]
    fn scaled_and_no_monitor_cases() {
        let r = r#"{"Ok":{"FocusedOutput":{"modes":[{"width":3840}],"logical":{"x":0,"y":0,"width":2194.2857,"height":1234}}}}"#;
        assert_eq!(logical_width(r), Some(2194.2857));
        assert_eq!(logical_width(r#"{"Ok":{"FocusedOutput":null}}"#), None);
        assert_eq!(logical_width(r#"{"Err":"x"}"#), None);
        assert_eq!(logical_width(r#"{"model":"a\"logical\":{\"width\":5}","logical":{"width":0}}"#), None);
    }

    #[test]
    fn pick_window_replies() {
        assert_eq!(picked(b"{\"Ok\":{\"PickedWindow\":null}}\n"), Picked::Nothing);
        let w = br#"{"Ok":{"PickedWindow":{"id":12,"title":"x","app_id":"y","pid":1,"workspace_id":1,"is_focused":true}}}"#;
        assert_eq!(picked(w), Picked::Window);
        assert_eq!(picked(br#"{"Err":"error parsing request"}"#), Picked::Unknown);
        assert_eq!(picked(b"{\"Ok\":{\"PickedWindow\":nu"), Picked::Unknown);
        assert_eq!(picked(b""), Picked::Unknown);
    }
}

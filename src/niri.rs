//! Logical width of the focused monitor, over the niri IPC socket
//! ($NIRI_SOCKET), without spawning `niri msg`.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(1);
const MAX_REPLY: u64 = 1 << 20;

pub fn focused_output_width() -> Option<f64> {
    let mut sock = UnixStream::connect(std::env::var_os("NIRI_SOCKET")?).ok()?;
    sock.set_read_timeout(Some(TIMEOUT)).ok()?;
    sock.set_write_timeout(Some(TIMEOUT)).ok()?;
    sock.write_all(b"\"FocusedOutput\"\n").ok()?;
    let mut reply = Vec::new();
    BufReader::new(sock.take(MAX_REPLY)).read_until(b'\n', &mut reply).ok()?;
    logical_width(std::str::from_utf8(&reply).ok()?)
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
}

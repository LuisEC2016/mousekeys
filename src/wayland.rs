//! Minimal Wayland client: speaks the wire protocol directly, without
//! libwayland, and only to create a `zwlr_virtual_pointer_v1`.

use std::fmt;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

const IFACE: &str = "zwlr_virtual_pointer_manager_v1";
const DISPLAY: u32 = 1;
const IO_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug)]
pub enum WlError {
    Io(io::Error),
    Closed,
    Protocol(String),
    Unsupported,
}

impl fmt::Display for WlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WlError::Io(e) => write!(f, "{e}"),
            WlError::Closed => f.write_str(&tr!("the compositor closed the connection", "el compositor cerró la conexión")),
            WlError::Protocol(m) => f.write_str(&tr!("protocol error: {m}", "error de protocolo: {m}")),
            WlError::Unsupported => f.write_str(&tr!("the compositor does not expose {IFACE}", "el compositor no expone {IFACE}")),
        }
    }
}

impl From<io::Error> for WlError {
    fn from(e: io::Error) -> Self {
        WlError::Io(e)
    }
}

pub fn fixed(v: f64) -> i32 {
    (v * 256.0).round() as i32
}

fn now_ms() -> u32 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: CLOCK_MONOTONIC always exists on Linux and `ts` is valid.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    (ts.tv_sec as u64 * 1000 + ts.tv_nsec as u64 / 1_000_000) as u32
}

fn u32_at(b: &[u8], off: usize) -> Option<u32> {
    Some(u32::from_ne_bytes(b.get(off..off + 4)?.try_into().ok()?))
}

/// Wayland string: length including the NUL, padded to 4 bytes.
fn string_at(b: &[u8], off: usize) -> Option<(&str, usize)> {
    let n = u32_at(b, off)? as usize;
    let bytes = b.get(off + 4..off + 4 + n.checked_sub(1)?)?;
    Some((std::str::from_utf8(bytes).ok()?, off + 4 + n.div_ceil(4) * 4))
}

#[derive(Default)]
struct Msg(Vec<u8>);

impl Msg {
    fn begin(&mut self, obj: u32, opcode: u16) -> usize {
        let start = self.0.len();
        self.0.extend_from_slice(&obj.to_ne_bytes());
        self.0.extend_from_slice(&(opcode as u32).to_ne_bytes());
        start
    }

    fn u32(&mut self, v: u32) -> &mut Self {
        self.0.extend_from_slice(&v.to_ne_bytes());
        self
    }

    fn i32(&mut self, v: i32) -> &mut Self {
        self.0.extend_from_slice(&v.to_ne_bytes());
        self
    }

    fn string(&mut self, s: &str) -> &mut Self {
        let n = s.len() + 1;
        self.u32(n as u32);
        self.0.extend_from_slice(s.as_bytes());
        self.0.resize(self.0.len() + 1 + (4 - n % 4) % 4, 0);
        self
    }

    fn end(&mut self, start: usize) {
        let size = (self.0.len() - start) as u32;
        let word = &mut self.0[start + 4..start + 8];
        let opcode = u32::from_ne_bytes(word.try_into().unwrap());
        word.copy_from_slice(&((size << 16) | opcode).to_ne_bytes());
    }
}

pub struct VirtualPointer {
    sock: UnixStream,
    next_id: u32,
    inbuf: Vec<u8>,
    out: Msg,
    registry: u32,
    manager_global: Option<(u32, u32)>,
    pending_sync: Option<u32>,
    ptr: u32,
}

impl VirtualPointer {
    pub fn connect() -> Result<Self, WlError> {
        let display = std::env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "wayland-0".into());
        let path = if display.starts_with('/') {
            PathBuf::from(display)
        } else {
            let runtime = std::env::var_os("XDG_RUNTIME_DIR")
                .ok_or_else(|| WlError::Protocol(tr!("XDG_RUNTIME_DIR is not set", "XDG_RUNTIME_DIR no definido")))?;
            PathBuf::from(runtime).join(display)
        };
        let sock = UnixStream::connect(&path)?;
        sock.set_read_timeout(Some(IO_TIMEOUT))?;
        sock.set_write_timeout(Some(IO_TIMEOUT))?;

        let mut vp = Self {
            sock,
            next_id: 2,
            inbuf: Vec::with_capacity(4096),
            out: Msg::default(),
            registry: 0,
            manager_global: None,
            pending_sync: None,
            ptr: 0,
        };
        vp.registry = vp.new_id();
        let m = vp.out.begin(DISPLAY, 1); // wl_display.get_registry
        vp.out.u32(vp.registry);
        vp.out.end(m);
        vp.roundtrip()?;

        let (name, version) = vp.manager_global.ok_or(WlError::Unsupported)?;
        let manager = vp.new_id();
        let m = vp.out.begin(vp.registry, 0); // wl_registry.bind
        vp.out.u32(name).string(IFACE).u32(version.min(2)).u32(manager);
        vp.out.end(m);
        vp.ptr = vp.new_id();
        let m = vp.out.begin(manager, 0); // create_virtual_pointer(seat = null)
        vp.out.u32(0).u32(vp.ptr);
        vp.out.end(m);
        vp.roundtrip()?;
        Ok(vp)
    }

    pub fn fd(&self) -> RawFd {
        self.sock.as_raw_fd()
    }

    fn new_id(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    fn flush(&mut self) -> Result<(), WlError> {
        let r = self.sock.write_all(&self.out.0);
        self.out.0.clear();
        Ok(r?)
    }

    fn parse(&mut self) -> Result<(), WlError> {
        let mut off = 0;
        while let (Some(obj), Some(word)) = (u32_at(&self.inbuf, off), u32_at(&self.inbuf, off + 4)) {
            let (size, opcode) = ((word >> 16) as usize, word & 0xffff);
            if size < 8 {
                return Err(WlError::Protocol(tr!("{size}-byte message", "mensaje de {size} bytes")));
            }
            let Some(body) = self.inbuf.get(off + 8..off + size) else {
                break;
            };
            if obj == DISPLAY && opcode == 0 {
                // wl_display.error(object, code, message)
                let code = u32_at(body, 4).unwrap_or(0);
                let msg = string_at(body, 8).map_or("", |(s, _)| s);
                return Err(WlError::Protocol(format!("{code}: {msg}")));
            }
            if obj == self.registry && opcode == 0 {
                // wl_registry.global(name, interface, version)
                if let (Some(name), Some((iface, next))) = (u32_at(body, 0), string_at(body, 4)) {
                    if iface == IFACE {
                        self.manager_global = u32_at(body, next).map(|v| (name, v));
                    }
                }
            } else if Some(obj) == self.pending_sync && opcode == 0 {
                self.pending_sync = None; // wl_callback.done
            }
            off += size;
        }
        self.inbuf.drain(..off);
        Ok(())
    }

    fn read_some(&mut self) -> Result<(), WlError> {
        let mut buf = [0u8; 4096];
        let n = self.sock.read(&mut buf)?;
        if n == 0 {
            return Err(WlError::Closed);
        }
        self.inbuf.extend_from_slice(&buf[..n]);
        self.parse()
    }

    /// Processes whatever has arrived. Call only when poll reports it readable.
    pub fn dispatch(&mut self) -> Result<(), WlError> {
        self.read_some()
    }

    pub fn roundtrip(&mut self) -> Result<(), WlError> {
        let cb = self.new_id();
        self.pending_sync = Some(cb);
        let m = self.out.begin(DISPLAY, 0); // wl_display.sync
        self.out.u32(cb);
        self.out.end(m);
        self.flush()?;
        while self.pending_sync.is_some() {
            self.read_some()?;
        }
        Ok(())
    }

    fn frame(&mut self) {
        let m = self.out.begin(self.ptr, 4);
        self.out.end(m);
    }

    pub fn motion(&mut self, dx: f64, dy: f64) -> Result<(), WlError> {
        let m = self.out.begin(self.ptr, 0);
        self.out.u32(now_ms()).i32(fixed(dx)).i32(fixed(dy));
        self.out.end(m);
        self.frame();
        self.flush()
    }

    pub fn button(&mut self, code: u32, pressed: bool) -> Result<(), WlError> {
        let m = self.out.begin(self.ptr, 2);
        self.out.u32(now_ms()).u32(code).u32(pressed as u32);
        self.out.end(m);
        self.frame();
        self.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_point_24_8() {
        assert_eq!(fixed(1.0), 256);
        assert_eq!(fixed(-1.5), -384);
        assert_eq!(fixed(0.72), 184);
    }

    #[test]
    fn message_with_padded_string() {
        let mut m = Msg::default();
        let s = m.begin(2, 0);
        m.u32(7).string(IFACE).u32(2).u32(3);
        m.end(s);
        let b = &m.0;
        // IFACE: 31 bytes + NUL = 32, no extra padding
        assert_eq!(b.len(), 8 + 4 + 4 + 32 + 4 + 4);
        assert_eq!(u32_at(b, 4).unwrap(), ((b.len() as u32) << 16));
        let (s, next) = string_at(b, 12).unwrap();
        assert_eq!(s, IFACE);
        assert_eq!(u32_at(b, next).unwrap(), 2);

        let mut m = Msg::default();
        let s = m.begin(1, 1);
        m.string("ab");
        m.end(s);
        assert_eq!(m.0.len(), 8 + 4 + 4);
        assert_eq!(string_at(&m.0, 8).unwrap(), ("ab", 16));
    }
}

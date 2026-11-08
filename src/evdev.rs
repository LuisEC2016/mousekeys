//! Key state read from /dev/input/event* with `EVIOCGKEY`.
//!
//! No grab and no event reading: it only asks which keys are held, and the
//! devices are open only for the duration of a movement, a Space gesture or
//! the wait for a click.
//!
//! Opening an evdev node takes microseconds, but closing it waits for an RCU
//! grace period (evdev_detach_client → synchronize_rcu): about 9 ms measured,
//! up to 33 ms. That is why non-keyboards are skipped by reading sysfs,
//! without opening them, and keyboards are closed on a separate thread.

use std::fs::{self, File, OpenOptions};
use std::io::Read as _;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;

pub const KEY_SPACE: u16 = 57;
pub const KEY_UP: u16 = 103;
pub const KEY_LEFT: u16 = 105;
pub const KEY_RIGHT: u16 = 106;
pub const KEY_DOWN: u16 = 108;
pub const ARROWS: [u16; 4] = [KEY_UP, KEY_LEFT, KEY_RIGHT, KEY_DOWN];

const KEY_MAX: usize = 0x2ff;
const KEY_NBYTES: usize = KEY_MAX / 8 + 1;
const NAME_LEN: usize = 256;
/// Bits per bitmap word: the kernel handles it as `unsigned long`.
const W: usize = usize::BITS as usize;
const KEY_WORDS: usize = KEY_NBYTES / size_of::<usize>();

const fn ioc_read(nr: u32, size: usize) -> u32 {
    (2 << 30) | ((size as u32) << 16) | ((b'E' as u32) << 8) | nr
}

const EVIOCGKEY: u32 = ioc_read(0x18, KEY_NBYTES);
const EVIOCGBIT_KEY: u32 = ioc_read(0x20 + 1, KEY_NBYTES);
const EVIOCGNAME: u32 = ioc_read(0x06, NAME_LEN);

/// Key bitmap in native words, as the kernel copies it: correct on any
/// endianness, and merging several keyboards takes 12 ORs, not 96.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct KeyState([usize; KEY_WORDS]);

impl KeyState {
    pub fn is_down(&self, code: u16) -> bool {
        let code = code as usize;
        code <= KEY_MAX && self.0[code / W] >> (code % W) & 1 == 1
    }

    pub fn any(&self) -> bool {
        self.0.iter().any(|&w| w != 0)
    }

    pub fn codes(&self) -> Vec<u16> {
        (0..=KEY_MAX as u16).filter(|&c| self.is_down(c)).collect()
    }
}

/// Read ioctl into `buf`; the size encoded in `request` must be that of `T`.
fn ioctl_read<T>(file: &File, request: u32, buf: &mut T) -> bool {
    debug_assert_eq!((request >> 16) & 0x3fff, size_of::<T>() as u32);
    // SAFETY: the kernel writes at most size_of::<T>() bytes into `buf`.
    unsafe { libc::ioctl(file.as_raw_fd(), request as _, (buf as *mut T).cast::<libc::c_void>()) >= 0 }
}

/// Does the sysfs `capabilities/key` bitmap declare the four arrows?
///
/// It holds kernel `long` words in hex, most significant first and without
/// the leading zero words. They are parsed with the width of `usize`; if a
/// word does not fit (32-bit binary on a 64-bit kernel), `None`, and the
/// caller checks by opening the device.
fn arrows_in_caps(text: &str) -> Option<bool> {
    let mut found = 0u8;
    let mut words = 0;
    for (i, tok) in text.split_ascii_whitespace().rev().enumerate() {
        if tok.len() > W / 4 {
            return None;
        }
        let word = usize::from_str_radix(tok, 16).ok()?;
        for (j, &k) in ARROWS.iter().enumerate() {
            let k = k as usize;
            if k / W == i && word >> (k % W) & 1 == 1 {
                found |= 1 << j;
            }
        }
        words += 1;
    }
    (words > 0).then_some(found == (1 << ARROWS.len()) - 1)
}

fn sysfs_arrows(event: u32) -> Option<bool> {
    let mut f = File::open(format!("/sys/class/input/event{event}/device/capabilities/key")).ok()?;
    let mut buf = [0u8; 512];
    let n = f.read(&mut buf).ok()?;
    if n == buf.len() {
        return None;
    }
    arrows_in_caps(std::str::from_utf8(&buf[..n]).ok()?)
}

pub struct Keyboard {
    pub path: PathBuf,
    file: File,
}

impl Keyboard {
    pub fn name(&self) -> String {
        let mut buf = [0u8; NAME_LEN];
        if !ioctl_read(&self.file, EVIOCGNAME, &mut buf) {
            return String::new();
        }
        let end = buf.iter().position(|&b| b == 0).unwrap_or(NAME_LEN);
        String::from_utf8_lossy(&buf[..end]).into_owned()
    }
}

/// Every readable device that has the four arrows.
#[derive(Default)]
pub struct Keyboards(Vec<Keyboard>);

impl Keyboards {
    pub fn open() -> Self {
        let Ok(dir) = fs::read_dir("/dev/input") else {
            return Self::default();
        };
        let mut paths: Vec<(u32, PathBuf)> = dir
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let n = e.file_name().to_str()?.strip_prefix("event")?.parse().ok()?;
                Some((n, e.path()))
            })
            .collect();
        paths.sort_unstable_by_key(|(n, _)| *n);

        let mut kbds = Vec::new();
        for (n, path) in paths {
            if sysfs_arrows(n) == Some(false) {
                continue;
            }
            let Ok(file) = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(&path)
            else {
                continue;
            };
            let mut caps = KeyState::default();
            if ioctl_read(&file, EVIOCGBIT_KEY, &mut caps.0) && ARROWS.iter().all(|&k| caps.is_down(k)) {
                kbds.push(Keyboard { path, file });
            }
        }
        Self(kbds)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Keyboard> {
        self.0.iter()
    }

    /// Keys held now on any keyboard. A disconnected one counts as released.
    pub fn state(&self) -> KeyState {
        let mut all = KeyState::default();
        let mut buf = KeyState::default();
        for k in &self.0 {
            if ioctl_read(&k.file, EVIOCGKEY, &mut buf.0) {
                for (a, b) in all.0.iter_mut().zip(buf.0) {
                    *a |= b;
                }
            }
        }
        all
    }
}

extern "C" fn close_all(arg: *mut libc::c_void) -> *mut libc::c_void {
    // SAFETY: `arg` is the Box that `Drop for Keyboards` handed to this thread.
    drop(unsafe { Box::from_raw(arg.cast::<Vec<Keyboard>>()) });
    std::ptr::null_mut()
}

impl Drop for Keyboards {
    /// Closes the descriptors on a throwaway thread so the daemon loop does
    /// not wait ~9 ms per keyboard for RCU. Raw pthread rather than
    /// `std::thread`: the thread only calls close(), and this way the binary
    /// is ~15 KB smaller. If the thread cannot be created, they are closed here.
    fn drop(&mut self) {
        if self.0.is_empty() {
            return;
        }
        let arg = Box::into_raw(Box::new(std::mem::take(&mut self.0)));
        // SAFETY: local attributes initialized before use; if pthread_create
        // fails, the Box never reached another thread and is reclaimed here.
        // The thread inherits the mask with the signals blocked: the loop's
        // signalfd keeps receiving them.
        unsafe {
            let mut attr: libc::pthread_attr_t = std::mem::zeroed();
            let mut tid: libc::pthread_t = 0;
            let mut ok = false;
            if libc::pthread_attr_init(&mut attr) == 0 {
                if libc::pthread_attr_setdetachstate(&mut attr, libc::PTHREAD_CREATE_DETACHED) == 0 {
                    // Minimal stack; if the system requires more, the default one stays.
                    libc::pthread_attr_setstacksize(&mut attr, libc::PTHREAD_STACK_MIN.max(16 * 1024));
                    ok = libc::pthread_create(&mut tid, &attr, close_all, arg.cast()) == 0;
                }
                libc::pthread_attr_destroy(&mut attr);
            }
            if !ok {
                drop(Box::from_raw(arg));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ioctls_match_linux_input_h() {
        // _IOR('E', 0x18, len) etc. with len = KEY_MAX/8+1 = 96
        assert_eq!(KEY_NBYTES, 96);
        assert_eq!(EVIOCGKEY, 0x8060_4518);
        assert_eq!(EVIOCGBIT_KEY, 0x8060_4521);
        assert_eq!(EVIOCGNAME, 0x8100_4506);
    }

    #[test]
    fn key_bits() {
        let mut s = KeyState::default();
        assert!(!s.any());
        for k in [KEY_SPACE, KEY_DOWN] {
            s.0[k as usize / W] |= 1 << (k as usize % W);
        }
        assert!(s.any());
        assert!(s.is_down(KEY_SPACE) && s.is_down(KEY_DOWN) && !s.is_down(KEY_UP));
        assert_eq!(s.codes(), vec![KEY_SPACE, KEY_DOWN]);
        assert!(!s.is_down(0x300));
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn sysfs_capabilities() {
        // Copied from /sys/class/input/event*/device/capabilities/key (kernel 7.0).
        let at_kbd = "40000 20000 0 0 0 101600702900c03 ff80207af250d001 feffffdfffcfffff fffffffffffffffe\n";
        let consumer = "733eff 0 0 483ffff17aff32d bfd4444600000000 1 130c730b17c000 267bfad9415fed 9e168000004400 10000002\n";
        let power = "8000 10000000000000 0\n";
        assert_eq!(arrows_in_caps(at_kbd), Some(true));
        assert_eq!(arrows_in_caps(consumer), Some(true));
        assert_eq!(arrows_in_caps(power), Some(false));
        assert_eq!(arrows_in_caps("0\n"), Some(false));
        // Only three arrows (KEY_DOWN missing, bit 44 of word 1).
        assert_eq!(arrows_in_caps("068000000000 0"), Some(false));
        assert_eq!(arrows_in_caps("168000000000 0"), Some(true));
        assert_eq!(arrows_in_caps(""), None);
        assert_eq!(arrows_in_caps("xyz 0"), None);
        assert_eq!(arrows_in_caps("10000000000000000 0"), None);
    }
}

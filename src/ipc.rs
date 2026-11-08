//! Control socket: datagrams to one abstract socket per Wayland session.
//!
//! Abstract so that no stale files are left behind and only one daemon can
//! exist per session. Since the abstract namespace has no file permissions,
//! the daemon checks with SO_PASSCRED that every command comes from its own uid.

use std::io;
use std::mem::{size_of, zeroed};
use std::os::fd::AsRawFd;
use std::os::linux::net::SocketAddrExt;
use std::os::unix::net::{SocketAddr, UnixDatagram};

pub const MAX_MSG: usize = 512;

pub fn address() -> io::Result<SocketAddr> {
    let display = std::env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "wayland-0".into());
    // SAFETY: getuid cannot fail.
    let uid = unsafe { libc::getuid() };
    SocketAddr::from_abstract_name(format!("niri-mousekeys-rs-{uid}-{display}"))
}

pub fn enable_passcred(sock: &UnixDatagram) -> io::Result<()> {
    let on: libc::c_int = 1;
    // SAFETY: correct option and length for SO_PASSCRED.
    let r = unsafe {
        libc::setsockopt(
            sock.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PASSCRED,
            (&on as *const libc::c_int).cast(),
            size_of::<libc::c_int>() as libc::socklen_t,
        )
    };
    if r < 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
}

pub enum Recv {
    Empty,
    Rejected,
    Msg(usize),
}

/// Reads one datagram without blocking. `Rejected` if it is not from this uid or does not fit.
pub fn recv(sock: &UnixDatagram, buf: &mut [u8]) -> io::Result<Recv> {
    let mut iov = libc::iovec { iov_base: buf.as_mut_ptr().cast(), iov_len: buf.len() };
    let mut cbuf = [0u64; 8];
    // SAFETY: msghdr points to buffers that live for the whole call, and the
    // CMSG_* macros only walk the control area the kernel has just filled.
    unsafe {
        let mut msg: libc::msghdr = zeroed();
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = cbuf.as_mut_ptr().cast();
        msg.msg_controllen = size_of::<[u64; 8]>() as _;
        let n = libc::recvmsg(sock.as_raw_fd(), &mut msg, libc::MSG_DONTWAIT | libc::MSG_CMSG_CLOEXEC);
        if n < 0 {
            let e = io::Error::last_os_error();
            return match e.kind() {
                io::ErrorKind::WouldBlock => Ok(Recv::Empty),
                io::ErrorKind::Interrupted => Ok(Recv::Empty),
                _ => Err(e),
            };
        }
        let mut uid = None;
        let mut c = libc::CMSG_FIRSTHDR(&msg);
        while !c.is_null() {
            if (*c).cmsg_level == libc::SOL_SOCKET && (*c).cmsg_type == libc::SCM_CREDENTIALS {
                let cred: libc::ucred = std::ptr::read_unaligned(libc::CMSG_DATA(c).cast());
                uid = Some(cred.uid);
            }
            c = libc::CMSG_NXTHDR(&msg, c);
        }
        let trunc = msg.msg_flags & libc::MSG_TRUNC != 0;
        if trunc || uid != Some(libc::getuid()) {
            return Ok(Recv::Rejected);
        }
        Ok(Recv::Msg(n as usize))
    }
}

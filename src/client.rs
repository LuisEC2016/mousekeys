//! The niri bind side: each press sends a datagram to the daemon and exits.
//! If the daemon is not running (DMS down, plugin disabled), it starts one.

use std::fs::OpenOptions;
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::net::UnixDatagram;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};

use crate::ipc;

fn not_running(e: &io::Error) -> bool {
    matches!(e.raw_os_error(), Some(libc::ECONNREFUSED | libc::ENOENT))
}

pub fn log_path() -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").unwrap_or_else(|| "/tmp".into());
    PathBuf::from(dir).join("niri-mousekeys-rs.log")
}

fn spawn_daemon() -> io::Result<()> {
    let exe = std::env::current_exe()?;
    let log = OpenOptions::new().create(true).append(true).mode(0o600).open(log_path())?;
    let mut cmd = Command::new(exe);
    cmd.arg("daemon").stdin(Stdio::null()).stdout(log.try_clone()?).stderr(log);
    // SAFETY: setsid is async-signal-safe.
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    cmd.spawn().map(drop)
}

/// Sends without starting the daemon. `Ok(false)` if there is no daemon.
pub fn try_send(msg: &str) -> io::Result<bool> {
    let sock = UnixDatagram::unbound()?;
    match sock.send_to_addr(msg.as_bytes(), &ipc::address()?) {
        Ok(_) => Ok(true),
        Err(e) if not_running(&e) => Ok(false),
        Err(e) => Err(e),
    }
}

pub fn send(msg: &str) -> i32 {
    match try_send(msg) {
        Ok(true) => return 0,
        Ok(false) => {}
        Err(e) => {
            log!("{}", tr!("could not send {msg:?}: {e}", "no se pudo enviar {msg:?}: {e}"));
            return 1;
        }
    }
    if let Err(e) = spawn_daemon() {
        log!("{}", tr!("could not start the daemon: {e}", "no se pudo arrancar el daemon: {e}"));
        return 1;
    }
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        sleep(Duration::from_millis(20));
        match try_send(msg) {
            Ok(true) => return 0,
            Ok(false) => continue,
            Err(e) => {
                log!("{}", tr!("could not send {msg:?}: {e}", "no se pudo enviar {msg:?}: {e}"));
                return 1;
            }
        }
    }
    let path = log_path();
    log!("{}", tr!("the daemon did not start; see {}", "el daemon no arrancó; ver {}", path.display()));
    1
}

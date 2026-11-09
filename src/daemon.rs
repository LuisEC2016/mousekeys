//! Resident daemon: keeps the Wayland connection and the virtual pointer, and
//! takes commands from the control socket and from stdin (the DMS plugin
//! writes the settings there when they change).

use std::f64::consts::SQRT_2;
use std::io;
use std::mem::{size_of, zeroed};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixDatagram;
use std::thread::sleep;
use std::time::{Duration, Instant};

use crate::config::Config;
use crate::evdev::{Candidates, KEY_DOWN, KEY_LEFT, KEY_RIGHT, KEY_SPACE, KEY_UP, KeyState, Keyboards, MODIFIERS};
use crate::ipc::{self, Recv};
use crate::niri;
use crate::wayland::{VirtualPointer, WlError};

pub const BTN_LEFT: u32 = 0x110;
pub const BTN_RIGHT: u32 = 0x111;
pub const BTN_MIDDLE: u32 = 0x112;
/// Button for `over_window`, which niri's pick grab swallows. It is outside
/// the mouse range (0x110..0x120): if it ever reached a client, Qt, GTK and
/// browsers would ignore it, and niri does not treat it as left, right or
/// middle, so it cannot start a Mod gesture either.
const PROBE_BUTTON: u32 = 0x2e7; // BTN_TRIGGER_HAPPY40
const PROBE_TIMEOUT: Duration = Duration::from_millis(50);

pub fn button_code(name: &str) -> Option<u32> {
    match name {
        "left" => Some(BTN_LEFT),
        "right" => Some(BTN_RIGHT),
        "middle" => Some(BTN_MIDDLE),
        _ => None,
    }
}

pub fn direction(name: &str) -> Option<(f64, f64)> {
    match name {
        "left" => Some((-1.0, 0.0)),
        "right" => Some((1.0, 0.0)),
        "up" => Some((0.0, -1.0)),
        "down" => Some((0.0, 1.0)),
        _ => None,
    }
}

fn number(s: &str) -> Option<f64> {
    s.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// At most `MAX_LOGGED` characters of `s`, for log lines quoting input.
fn clip(s: &str) -> String {
    const MAX_LOGGED: usize = 80;
    match s.char_indices().nth(MAX_LOGGED) {
        Some((end, _)) => format!("{}…", &s[..end]),
        None => s.to_owned(),
    }
}

/// Enter pressed up to three times before releasing: single, double or
/// triple click (word, paragraph). More presses are ignored instead of
/// queueing an unbounded burst.
const MAX_CLICKS: usize = 3;

#[derive(Debug, PartialEq)]
pub enum Cmd<'a> {
    Move((f64, f64)),
    Click(u32),
    Space,
    Jump(f64),
    Nudge(f64, f64),
    Config(Vec<&'a str>),
    Quit,
    Ping,
}

pub fn parse(line: &str) -> Result<Option<Cmd<'_>>, String> {
    let mut words = line.split_ascii_whitespace();
    let Some(cmd) = words.next() else {
        return Ok(None);
    };
    let args: Vec<&str> = words.collect();
    let line = clip(line.trim());
    let bad = || tr!("unknown command: {line:?}", "orden desconocida: {line:?}");
    let cmd = match (cmd, args.as_slice()) {
        ("move", [d]) => Cmd::Move(direction(d).ok_or_else(bad)?),
        ("click", [b]) => Cmd::Click(button_code(b).ok_or_else(bad)?),
        ("space", []) => Cmd::Space,
        ("jump", ["left"]) => Cmd::Jump(-1.0),
        ("jump", ["right"]) => Cmd::Jump(1.0),
        ("nudge", [dx, dy]) => Cmd::Nudge(number(dx).ok_or_else(bad)?, number(dy).ok_or_else(bad)?),
        ("config", pairs) => Cmd::Config(pairs.to_vec()),
        ("quit", []) => Cmd::Quit,
        ("ping", []) => Cmd::Ping,
        _ => return Err(bad()),
    };
    Ok(Some(cmd))
}

struct Move {
    kbd: Keyboards,
    t0: f64,
    last: f64,
    hint: (f64, f64),
    moved: bool,
}

/// Space gesture: `Down` (held, not yet known whether it is a tap or a drag),
/// then `Wait` (released, waiting for a possible second tap) or `Drag`.
#[derive(Clone, Copy, PartialEq)]
enum Phase {
    Down,
    Wait,
    Drag,
}

struct Space {
    phase: Phase,
    kbd: Keyboards,
    t0: f64,
    t_up: f64,
    /// The pointer moved while Space was down: the button would go down
    /// wherever the pointer ended up, not where Space was pressed.
    moved: bool,
    /// Whether niri would take Mod + left button at the press point, asked
    /// once, the first time the drag could start with modifiers held.
    over_window: Option<bool>,
}

pub struct Daemon {
    vp: VirtualPointer,
    cfg: Config,
    epoch: Instant,
    mv: Option<Move>,
    candidates: Candidates,
    clicks: Vec<u32>,
    click_kbd: Keyboards,
    click_deadline: f64,
    /// Every key was released: the queued clicks go out one per `click_gap`,
    /// scheduled through poll instead of sleeping in the loop.
    click_ready: bool,
    click_next: f64,
    space: Option<Space>,
    held: Option<u32>,
    warned_no_kbd: bool,
    warned_foreign: bool,
    exit: Option<i32>,
}

impl Daemon {
    pub fn new(vp: VirtualPointer, cfg: Config) -> Self {
        Self {
            vp,
            cfg,
            epoch: Instant::now(),
            mv: None,
            candidates: Candidates::default(),
            clicks: Vec::new(),
            click_kbd: Keyboards::default(),
            click_deadline: 0.0,
            click_ready: false,
            click_next: 0.0,
            space: None,
            held: None,
            warned_no_kbd: false,
            warned_foreign: false,
            exit: None,
        }
    }

    fn now(&self) -> f64 {
        self.epoch.elapsed().as_secs_f64()
    }

    fn keyboards(&mut self) -> Keyboards {
        let kbd = Keyboards::open_cached(&mut self.candidates);
        if kbd.is_empty() && !self.warned_no_kbd {
            log!(
                "{}",
                tr!(
                    "cannot read any keyboard in /dev/input (user not in the input group?). \
                     Degraded mode: fixed steps.",
                    "no se puede leer ningún teclado en /dev/input (¿usuario fuera del grupo input?). \
                     Modo degradado: pasos fijos."
                )
            );
        }
        self.warned_no_kbd = kbd.is_empty();
        kbd
    }

    pub fn handle(&mut self, line: &str) -> Result<(), WlError> {
        let cmd = match parse(line) {
            Ok(Some(c)) => c,
            Ok(None) => return Ok(()),
            Err(e) => {
                log!("{e}");
                return Ok(());
            }
        };
        if matches!(cmd, Cmd::Move(_) | Cmd::Click(_) | Cmd::Space) {
            // Undoes the hide-when-typing of the key that just triggered the bind.
            self.vp.motion(0.0, 0.0)?;
        }
        match cmd {
            Cmd::Move(hint) => {
                self.drag_before_moving()?;
                self.start_move(hint)?;
            }
            Cmd::Click(code) => self.queue_click(code)?,
            Cmd::Space => self.space_press(self.now())?,
            Cmd::Jump(sign) => {
                // niri clamps the motion at the outer edge: jumping "out" does nothing.
                let w = niri::focused_output_width().unwrap_or(self.cfg.jump_fallback);
                self.motion(sign * w, 0.0)?;
            }
            Cmd::Nudge(dx, dy) => {
                self.motion(dx, dy)?;
                self.vp.roundtrip()?;
            }
            Cmd::Config(pairs) => {
                let old = self.cfg.clone();
                for e in self.cfg.apply(pairs) {
                    log!("{e}");
                }
                if let Some(changes) = self.cfg.changes(&old) {
                    log!("{changes}");
                }
            }
            Cmd::Quit => self.exit = Some(0),
            Cmd::Ping => {}
        }
        Ok(())
    }

    fn start_move(&mut self, hint: (f64, f64)) -> Result<(), WlError> {
        if self.mv.is_some() {
            return Ok(()); // already moving: polling picks up the new arrow
        }
        let now = self.now(); // before opening: acceleration counts from the press
        let kbd = self.keyboards();
        if kbd.is_empty() {
            let step = self.cfg.fallback_step;
            return self.motion(hint.0 * step, hint.1 * step);
        }
        self.mv = Some(Move { kbd, t0: now, last: now, hint, moved: false });
        Ok(())
    }

    fn step_move(&mut self, now: f64) -> Result<(), WlError> {
        let Some(m) = self.mv.as_mut() else {
            return Ok(());
        };
        let s = m.kbd.state();
        let held = |k| if s.is_down(k) { 1.0 } else { 0.0 };
        let (l, r, u, d) = (held(KEY_LEFT), held(KEY_RIGHT), held(KEY_UP), held(KEY_DOWN));
        if l + r + u + d == 0.0 {
            let (hint, moved) = (m.hint, m.moved);
            let done = self.mv.take(); // dropped after the last motion
            if !moved {
                // Press shorter than the first poll.
                let step = self.cfg.min_step;
                self.motion(hint.0 * step, hint.1 * step)?;
            }
            drop(done);
            return Ok(());
        }
        let (dx, dy) = (r - l, d - u);
        let dt = (now - m.last).min(0.05);
        m.last = now;
        if (dx != 0.0 || dy != 0.0) && dt > 0.0 {
            let diag = if dx != 0.0 && dy != 0.0 { SQRT_2 } else { 1.0 };
            let dist = self.cfg.speed(now - m.t0) * dt / diag;
            m.moved = true;
            self.motion(dx * dist, dy * dist)?;
        }
        Ok(())
    }

    /// An arrow while Space is still held means a drag, not a tap: if the
    /// button may go down now (see `drag_blocked`), it goes down before the
    /// pointer moves, so the drag starts where Space was pressed instead of
    /// after `holdMs`.
    fn drag_before_moving(&mut self) -> Result<(), WlError> {
        let Some(s) = self.space.as_ref().filter(|s| s.phase == Phase::Down) else {
            return Ok(());
        };
        let state = s.kbd.state();
        if !state.is_down(KEY_SPACE) || self.drag_blocked(&state)? {
            return Ok(());
        }
        self.start_drag()
    }

    fn start_drag(&mut self) -> Result<(), WlError> {
        if let Some(s) = self.space.as_mut() {
            s.phase = Phase::Drag;
        }
        self.held = Some(BTN_LEFT);
        self.vp.button(BTN_LEFT, true)
    }

    /// With `dragWaitMods`, the drag button waits for Super, Ctrl, Alt and
    /// Shift to be released only where stock niri would take Mod + left
    /// button as "move window": over a window. Over a DMS panel it goes down
    /// with the modifiers held. The answer is asked once per press, at the
    /// press point; if the pointer already left it, the old rule applies.
    fn drag_blocked(&mut self, state: &KeyState) -> Result<bool, WlError> {
        if !self.cfg.waits_for_mods() || !state.any_of(&MODIFIERS) {
            return Ok(false);
        }
        let Some(s) = self.space.as_ref() else {
            return Ok(true);
        };
        if let Some(known) = s.over_window {
            return Ok(known);
        }
        if s.moved {
            return Ok(true);
        }
        let over = self.over_window()?;
        if let Some(s) = self.space.as_mut() {
            s.over_window = Some(over);
        }
        Ok(over)
    }

    /// Whether niri would take Mod + left/right button at the pointer as
    /// "move/resize window" (see `niri::Pick`). When in doubt (no niri socket,
    /// no answer), `true`: the button waits for the modifiers, as it always did.
    ///
    /// The pick grab sends the surface under the pointer a leave and an enter,
    /// so it is only asked when modifiers are held, and never while a button
    /// is down (it would break the application's implicit grab).
    fn over_window(&mut self) -> Result<bool, WlError> {
        if self.held.is_some() {
            return Ok(true);
        }
        // Nothing of ours left unread in niri: the probe button goes after the grab.
        self.vp.roundtrip()?;
        let Some(mut pick) = niri::Pick::start() else {
            return Ok(true);
        };
        for _ in 0..2 {
            self.vp.button(PROBE_BUTTON, true)?;
            self.vp.button(PROBE_BUTTON, false)?;
            if let Some(picked) = pick.answer(PROBE_TIMEOUT) {
                return Ok(picked != niri::Picked::Nothing);
            }
            // The button beat the grab: one more press ends the pick.
        }
        log!(
            "{}",
            tr!(
                "niri did not answer pick-window; the button waits for the modifiers",
                "niri no respondió a pick-window; el botón espera a los modificadores"
            )
        );
        Ok(true)
    }

    /// Moves the pointer, noting it in a Space press whose button has not
    /// gone down yet.
    fn motion(&mut self, dx: f64, dy: f64) -> Result<(), WlError> {
        self.vp.motion(dx, dy)?;
        if dx != 0.0 || dy != 0.0 {
            if let Some(s) = self.space.as_mut().filter(|s| s.phase == Phase::Down) {
                s.moved = true;
            }
        }
        Ok(())
    }

    /// Clicks wait until every key is released: stock niri takes Super +
    /// button over a window, and the application would see Ctrl. With
    /// modifiers held the click goes out right away, so Super+Ctrl can stay
    /// down as a mode, where niri lets it through: over a DMS panel (no window
    /// under the pointer), or anywhere with `dragWaitMods` off (patch 0004),
    /// where the application does see the modifiers.
    fn queue_click(&mut self, code: u32) -> Result<(), WlError> {
        if self.clicks.len() >= MAX_CLICKS {
            return Ok(());
        }
        if self.clicks.is_empty() && !self.click_ready {
            self.click_kbd = self.keyboards();
            self.click_deadline = self.now() + self.cfg.click_timeout;
            if self.click_kbd.state().any_of(&MODIFIERS) && !(self.cfg.waits_for_mods() && self.over_window()?) {
                self.click_kbd = Keyboards::default();
                self.click_ready = true;
                self.click_next = self.now();
            }
        }
        self.clicks.push(code);
        Ok(())
    }

    fn space_press(&mut self, now: f64) -> Result<(), WlError> {
        if let Some(s) = &self.space {
            if s.phase != Phase::Wait {
                return Ok(()); // Space already down: niri does not repeat (repeat=false)
            }
            let t_up = s.t_up;
            self.space = None;
            if now - t_up <= self.cfg.double_t {
                return self.queue_click(BTN_RIGHT); // double tap
            }
            self.queue_click(BTN_LEFT)?; // the wait expired before it was polled
        }
        let kbd = self.keyboards();
        if kbd.is_empty() {
            // Without /dev/input there is no way to measure duration or a double tap.
            return self.queue_click(BTN_LEFT);
        }
        self.space = Some(Space { phase: Phase::Down, kbd, t0: now, t_up: 0.0, moved: false, over_window: None });
        Ok(())
    }

    fn step_space(&mut self, now: f64) -> Result<(), WlError> {
        let Some(s) = self.space.as_mut() else {
            return Ok(());
        };
        if s.phase == Phase::Wait {
            // >= and not >: poll wakes up exactly at the deadline (see `timeout_ms`).
            if now - s.t_up >= self.cfg.double_t {
                self.space = None;
                self.queue_click(BTN_LEFT)?; // single tap
            }
            return Ok(());
        }
        // If the keyboards can no longer be read, the state comes back empty =
        // released: the drag ends instead of leaving the button down.
        let state = s.kbd.state();
        let down = state.is_down(KEY_SPACE);
        let long = now - s.t0 >= self.cfg.hold_t;
        // With dragWaitMods, over a window the button waits until Super,
        // Ctrl, Alt and Shift are released (Space still held): on stock niri,
        // Mod + left button there is "move window" and the application would
        // never see the press.
        let mods_block = s.phase == Phase::Down && down && long && self.drag_blocked(&state)?;
        let Some(s) = self.space.as_mut() else {
            return Ok(());
        };
        match s.phase {
            Phase::Down if !down && long => {
                // A long press released before the drag could start is not a tap.
                self.space = None;
                log!(
                    "{}",
                    tr!(
                        "drag not started: release Super and Ctrl while keeping Space down (dragWaitMods)",
                        "arrastre no iniciado: suelta Super y Ctrl sin soltar Space (dragWaitMods)"
                    )
                );
            }
            Phase::Down if !down => {
                s.phase = Phase::Wait;
                s.t_up = now;
            }
            Phase::Down if long && !mods_block && s.moved => {
                // Pressing now would grab wherever the arrows left the pointer.
                self.space = None;
                log!(
                    "{}",
                    tr!(
                        "drag not started: the pointer moved before the button went down. \
                         Move only after releasing Super and Ctrl, with Space still down",
                        "arrastre no iniciado: el puntero se movió antes de bajar el botón. \
                         Mueve solo después de soltar Super y Ctrl, con Space aún pulsado"
                    )
                );
            }
            Phase::Down if long && !mods_block => self.start_drag()?,
            Phase::Drag if !down => {
                self.release_held()?; // the button first, then close the keyboards
                self.space = None;
            }
            _ => {}
        }
        Ok(())
    }

    pub fn release_held(&mut self) -> Result<(), WlError> {
        if let Some(code) = self.held.take() {
            self.vp.button(code, false)?;
        }
        Ok(())
    }

    fn step_click(&mut self, now: f64) -> Result<(), WlError> {
        let mut kbd = None;
        if !self.click_ready {
            let state = self.click_kbd.state();
            if state.any() && now < self.click_deadline {
                return Ok(());
            }
            kbd = Some(std::mem::take(&mut self.click_kbd)); // closed after the first click
            if state.any() {
                self.clicks.clear();
                let (timeout, codes) = (self.cfg.click_timeout, state.codes());
                log!(
                    "{}",
                    tr!(
                        "click dropped: keys still held after {timeout}s (evdev codes {codes:?})",
                        "clic cancelado: teclas aún pulsadas tras {timeout}s (códigos evdev {codes:?})"
                    )
                );
                return Ok(());
            }
            self.click_ready = true;
            self.click_next = now;
        }
        if now >= self.click_next && !self.clicks.is_empty() {
            let code = self.clicks.remove(0);
            self.vp.button(code, true)?;
            self.vp.button(code, false)?;
            self.click_next = now + self.cfg.click_gap;
        }
        if self.clicks.is_empty() {
            self.click_ready = false;
        }
        drop(kbd);
        Ok(())
    }

    fn read_stdin(&mut self, buf: &mut Vec<u8>) -> Result<bool, WlError> {
        let mut chunk = [0u8; 1024];
        // SAFETY: read into a local buffer of known size.
        let n = unsafe { libc::read(0, chunk.as_mut_ptr().cast(), chunk.len()) };
        if n < 0 {
            let kind = io::Error::last_os_error().kind();
            return Ok(matches!(kind, io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock));
        }
        if n == 0 {
            return Ok(false);
        }
        buf.extend_from_slice(&chunk[..n as usize]);
        while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = buf.drain(..=pos).collect();
            self.handle(&String::from_utf8_lossy(&line))?;
        }
        if buf.len() > 4 * ipc::MAX_MSG {
            buf.clear();
        }
        Ok(true)
    }

    fn read_control(&mut self, ctl: &UnixDatagram) -> Result<(), WlError> {
        let mut msg = [0u8; ipc::MAX_MSG];
        loop {
            match ipc::recv(ctl, &mut msg) {
                Ok(Recv::Empty) => return Ok(()),
                Ok(Recv::Rejected) => {
                    if !self.warned_foreign {
                        log!(
                            "{}",
                            tr!(
                                "command ignored: not from this user or too long",
                                "orden ignorada: no viene de este usuario o es demasiado larga"
                            )
                        );
                        self.warned_foreign = true;
                    }
                }
                Ok(Recv::Msg(n)) => self.handle(&String::from_utf8_lossy(&msg[..n]))?,
                Err(e) => {
                    log!("{}", tr!("control socket: {e}", "socket de control: {e}"));
                    return Ok(());
                }
            }
        }
    }

    /// ms until the next pending step, or -1 with nothing in progress.
    ///
    /// Movement wakes up at `last + tick`, not a whole tick after any other
    /// wakeup. The double tap wait does not poll keys: it wakes up once when
    /// the deadline expires instead of at 125 Hz.
    fn timeout_ms(&self, now: f64) -> i32 {
        let tick = self.cfg.tick;
        let mut next = f64::INFINITY;
        if let Some(m) = &self.mv {
            next = next.min(m.last + tick);
        }
        if !self.clicks.is_empty() {
            next = next.min(if self.click_ready { self.click_next } else { now + tick });
        }
        if let Some(s) = &self.space {
            next = next.min(if s.phase == Phase::Wait { s.t_up + self.cfg.double_t } else { now + tick });
        }
        if next == f64::INFINITY {
            return -1;
        }
        ((next - now).max(0.0) * 1000.0).ceil().min(i32::MAX as f64) as i32
    }

    /// Main loop. With nothing in progress, poll waits without a timeout: 0% CPU.
    pub fn run(&mut self, ctl: &UnixDatagram, sigfd: RawFd, lifeline: Option<RawFd>) -> Result<Exit, WlError> {
        let pollfd = |fd| libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
        let mut stdin_open = true;
        let mut stdin_buf = Vec::new();
        loop {
            if self.exit.is_some() {
                return Ok(Exit::Quit);
            }
            let timeout = self.timeout_ms(self.now());
            let mut fds = [
                pollfd(self.vp.fd()),
                pollfd(ctl.as_raw_fd()),
                pollfd(sigfd),
                pollfd(if stdin_open { 0 } else { -1 }),
                pollfd(lifeline.unwrap_or(-1)),
            ];
            // SAFETY: `fds` lives for the whole call; poll ignores negative fds.
            let n = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, timeout) };
            if n < 0 {
                let e = io::Error::last_os_error();
                if e.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(e.into());
            }
            if fds[2].revents != 0 {
                return Ok(Exit::Signal(read_signal(sigfd)));
            }
            if fds[4].revents != 0 {
                return Ok(Exit::Supervisor);
            }
            if fds[0].revents != 0 {
                self.vp.dispatch()?;
            }
            if fds[1].revents != 0 {
                self.read_control(ctl)?;
            }
            if fds[3].revents != 0 {
                stdin_open = self.read_stdin(&mut stdin_buf)?;
            }
            let now = self.now();
            if self.mv.as_ref().is_some_and(|m| now - m.last >= self.cfg.tick * 0.9) {
                self.step_move(now)?;
            }
            if self.space.is_some() {
                self.step_space(now)?;
            }
            if !self.clicks.is_empty() {
                self.step_click(now)?;
            }
        }
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        // niri does not release the buttons of a virtual pointer that is
        // destroyed (destroy_virtual_pointer does nothing): without this,
        // exiting in the middle of a drag leaves the left button stuck.
        if let Err(e) = self.release_held() {
            log!("{}", tr!("could not release the button: {e}", "no se pudo soltar el botón: {e}"));
        }
    }
}

pub enum Exit {
    Quit,
    Signal(i32),
    Supervisor,
}

fn read_signal(fd: RawFd) -> i32 {
    // SAFETY: signalfd delivers whole signalfd_siginfo structures.
    unsafe {
        let mut info: libc::signalfd_siginfo = zeroed();
        let n = libc::read(fd, (&mut info as *mut libc::signalfd_siginfo).cast(), size_of::<libc::signalfd_siginfo>());
        if n == size_of::<libc::signalfd_siginfo>() as isize { info.ssi_signo as i32 } else { libc::SIGTERM }
    }
}

const STOP_SIGNALS: [libc::c_int; 3] = [libc::SIGTERM, libc::SIGINT, libc::SIGHUP];

/// Blocks `signals` and delivers them through a signalfd, so they are handled
/// from a poll loop instead of an asynchronous handler.
fn block_signals(signals: &[libc::c_int]) -> io::Result<OwnedFd> {
    // SAFETY: POSIX calls on a local sigset_t; there are no other threads yet.
    unsafe {
        let mut set: libc::sigset_t = zeroed();
        libc::sigemptyset(&mut set);
        for &sig in signals {
            libc::sigaddset(&mut set, sig);
        }
        if libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut()) != 0 {
            return Err(io::Error::last_os_error());
        }
        let fd = libc::signalfd(-1, &set, libc::SFD_CLOEXEC | libc::SFD_NONBLOCK);
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(OwnedFd::from_raw_fd(fd))
    }
}

/// `Ok(None)` if a daemon is already running and no takeover was requested.
/// With `takeover`, asks it to exit (it releases its button if it holds one)
/// and takes its place: this is how the daemon DMS supervises replaces the
/// one a bind started.
fn bind_control(takeover: bool) -> io::Result<Option<UnixDatagram>> {
    let addr = ipc::address()?;
    let in_use = |e: &io::Error| e.kind() == io::ErrorKind::AddrInUse;
    match UnixDatagram::bind_addr(&addr) {
        Ok(s) => return Ok(Some(s)),
        Err(e) if in_use(&e) && takeover => {}
        Err(e) if in_use(&e) => return Ok(None),
        Err(e) => return Err(e),
    }
    log!("{}", tr!("taking over from the daemon that was running", "relevando al daemon que ya corría"));
    let _ = UnixDatagram::unbound()?.send_to_addr(b"quit", &addr);
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        sleep(Duration::from_millis(20));
        match UnixDatagram::bind_addr(&addr) {
            Ok(s) => return Ok(Some(s)),
            Err(e) if in_use(&e) && Instant::now() < deadline => continue,
            Err(e) => return Err(e),
        }
    }
}

/// `--supervised` mode: this process stays as the guardian (the direct child
/// of DMS) and the daemon goes on in a child with its own session. Quickshell
/// sends SIGKILL when it destroys a Process; when the guardian dies, the kernel
/// closes the write end of the pipe and the daemon, which watches the read
/// end, releases the button and exits. Returns that end in the child; the
/// guardian never returns.
fn fork_guardian() -> io::Result<OwnedFd> {
    let mut fds = [0; 2];
    // SAFETY: pipe2 fills in two new descriptors that become OwnedFd.
    let (lifeline, keepalive) = unsafe {
        if libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) < 0 {
            return Err(io::Error::last_os_error());
        }
        (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1]))
    };
    // SIGCHLD is blocked before the fork so an early exit of the child is not lost.
    let sigfd = block_signals(&[libc::SIGTERM, libc::SIGINT, libc::SIGHUP, libc::SIGCHLD])?;
    // SAFETY: there are no other threads yet, so the child can keep running normal Rust.
    match unsafe { libc::fork() } {
        -1 => Err(io::Error::last_os_error()),
        0 => {
            drop(keepalive);
            drop(sigfd);
            // SAFETY: a forked child is never a group leader, so setsid does not fail.
            unsafe { libc::setsid() };
            Ok(lifeline)
        }
        child => {
            drop(lifeline);
            std::process::exit(guard(child, &sigfd));
        }
    }
}

/// Waits for the daemon to exit (and returns its code, so DMS sees crashes) or
/// for the guardian itself to be stopped; then it exits and the daemon learns
/// about it through the pipe.
fn guard(child: libc::pid_t, sigfd: &OwnedFd) -> i32 {
    let mut pfd = libc::pollfd { fd: sigfd.as_raw_fd(), events: libc::POLLIN, revents: 0 };
    loop {
        // SAFETY: a single valid pollfd.
        if unsafe { libc::poll(&mut pfd, 1, -1) } < 0 {
            continue;
        }
        let sig = read_signal(sigfd.as_raw_fd());
        if sig != libc::SIGCHLD {
            return 128 + sig;
        }
        let mut status = 0;
        // SAFETY: non-blocking wait for our only child.
        if unsafe { libc::waitpid(child, &mut status, libc::WNOHANG) } == child {
            return if libc::WIFEXITED(status) {
                libc::WEXITSTATUS(status)
            } else {
                128 + libc::WTERMSIG(status)
            };
        }
    }
}

pub fn main(args: &[&str]) -> i32 {
    let mut cfg = Config::default();
    let supervised = args.contains(&"--supervised");
    let takeover = supervised || args.contains(&"--takeover");
    for e in cfg.apply(args.iter().copied().filter(|a| !a.starts_with("--"))) {
        log!("{e}");
    }
    for flag in args.iter().filter(|a| a.starts_with("--") && !matches!(**a, "--supervised" | "--takeover")) {
        log!("{}", tr!("unknown option: {flag}", "opción desconocida: {flag}"));
    }
    let lifeline = if supervised {
        match fork_guardian() {
            Ok(fd) => Some(fd),
            Err(e) => {
                log!("{}", tr!("could not create the guardian: {e}", "no se pudo crear el guardián: {e}"));
                return 1;
            }
        }
    } else {
        None
    };
    let sigfd = match block_signals(&STOP_SIGNALS) {
        Ok(fd) => fd,
        Err(e) => {
            log!("{}", tr!("could not set up the signals: {e}", "no se pudieron preparar las señales: {e}"));
            return 1;
        }
    };
    let ctl = match bind_control(takeover) {
        Ok(Some(s)) => s,
        Ok(None) => {
            log!("{}", tr!("a daemon is already running in this session", "ya hay un daemon en esta sesión"));
            return 0;
        }
        Err(e) => {
            log!("{}", tr!("control socket: {e}", "socket de control: {e}"));
            return 1;
        }
    };
    if let Err(e) = ctl.set_nonblocking(true).and_then(|_| ipc::enable_passcred(&ctl)) {
        log!("{}", tr!("control socket: {e}", "socket de control: {e}"));
        return 1;
    }
    let vp = match VirtualPointer::connect() {
        Ok(vp) => vp,
        Err(e) => {
            log!("{}", tr!("could not create the virtual pointer: {e}", "no se pudo crear el puntero virtual: {e}"));
            return 1;
        }
    };
    let (pid, summary) = (std::process::id(), cfg.summary());
    log!("{}", tr!("ready (pid {pid}). {summary}", "listo (pid {pid}). {summary}"));
    let mut daemon = Daemon::new(vp, cfg);
    let code = match daemon.run(&ctl, sigfd.as_raw_fd(), lifeline.as_ref().map(AsRawFd::as_raw_fd)) {
        Ok(Exit::Quit) => {
            log!("{}", tr!("exiting", "saliendo"));
            0
        }
        Ok(Exit::Supervisor) => {
            log!("{}", tr!("the DMS plugin stopped; exiting", "el plugin de DMS se detuvo; saliendo"));
            0
        }
        Ok(Exit::Signal(sig)) => {
            log!("{}", tr!("signal {sig}; exiting", "señal {sig}; saliendo"));
            128 + sig
        }
        Err(WlError::Closed) => {
            log!("{}", tr!("the compositor closed the connection; exiting", "el compositor cerró la conexión; saliendo"));
            0
        }
        Err(e) => {
            log!("{}", tr!("Wayland connection lost: {e}", "conexión Wayland perdida: {e}"));
            1
        }
    };
    drop(daemon);
    code
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_commands() {
        assert_eq!(parse("move left").unwrap(), Some(Cmd::Move((-1.0, 0.0))));
        assert_eq!(parse(" click   middle \n").unwrap(), Some(Cmd::Click(BTN_MIDDLE)));
        assert_eq!(parse("space").unwrap(), Some(Cmd::Space));
        assert_eq!(parse("jump right").unwrap(), Some(Cmd::Jump(1.0)));
        assert_eq!(parse("nudge -150 0.5").unwrap(), Some(Cmd::Nudge(-150.0, 0.5)));
        assert_eq!(parse("config vMax=2600 holdMs=400").unwrap(), Some(Cmd::Config(vec!["vMax=2600", "holdMs=400"])));
        assert_eq!(parse("").unwrap(), None);
        assert_eq!(parse("quit").unwrap(), Some(Cmd::Quit));
    }

    #[test]
    fn invalid_commands() {
        for bad in ["move", "move sideways", "click left right", "jump up", "nudge 1", "nudge nan 0", "space 1", "rm -rf"] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn long_input_is_clipped_in_the_log() {
        let e = parse(&"ñ".repeat(3000)).unwrap_err();
        assert!(e.chars().count() < 130 && e.contains('…'), "{e}");
        assert_eq!(clip("move left"), "move left");
    }
}

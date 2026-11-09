//! niri-mousekeys: move the pointer from the keyboard on niri (Wayland).
//!
//! A single binary: niri binds launch it as a client (it sends a datagram
//! and exits) and the DMS plugin supervises it as the daemon.

// No eprintln!: it panics if stderr is closed (DMS closes the pipe when it
// destroys the Process) and the daemon must still be able to release the button.
macro_rules! log {
    ($($arg:tt)*) => {{
        use std::io::Write as _;
        let _ = writeln!(std::io::stderr(), "niri-mousekeys: {}", format_args!($($arg)*));
    }};
}

/// Formats the English text, or the Spanish one when the locale is Spanish
/// (see `i18n`). Both are format strings that take the same arguments.
macro_rules! tr {
    ($en:literal, $es:literal $(, $arg:expr)* $(,)?) => {
        if crate::i18n::spanish() { format!($es $(, $arg)*) } else { format!($en $(, $arg)*) }
    };
}

mod client;
mod config;
mod daemon;
mod evdev;
mod i18n;
mod ipc;
mod niri;
mod wayland;

use config::Config;
use evdev::Keyboards;

const USAGE: &str = "\
Usage:
  niri-mousekeys daemon [--takeover|--supervised] [key=value...]
  niri-mousekeys move left|right|up|down               arrow binds
  niri-mousekeys click left|right|middle               click when the keys are released
  niri-mousekeys space                                 tap, double tap or hold
  niri-mousekeys jump left|right                       jump to the adjacent monitor
  niri-mousekeys nudge DX DY                           test: move by DX,DY px
  niri-mousekeys config key=value...                   change settings live
  niri-mousekeys quit                                  stop the daemon
  niri-mousekeys check                                 diagnostics
  niri-mousekeys binds                                 niri binds for this binary
  niri-mousekeys version
";

const USAGE_ES: &str = "\
Uso:
  niri-mousekeys daemon [--takeover|--supervised] [clave=valor...]
  niri-mousekeys move left|right|up|down               atajos de las flechas
  niri-mousekeys click left|right|middle               clic al soltar las teclas
  niri-mousekeys space                                 toque, doble toque o mantener
  niri-mousekeys jump left|right                       salta al monitor contiguo
  niri-mousekeys nudge DX DY                           prueba: desplaza DX,DY px
  niri-mousekeys config clave=valor...                 cambia ajustes en caliente
  niri-mousekeys quit                                  para el daemon
  niri-mousekeys check                                 diagnóstico
  niri-mousekeys binds                                 binds de niri para este binario
  niri-mousekeys version
";

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = argv.iter().map(String::as_str).collect();
    std::process::exit(run(&args));
}

fn run(args: &[&str]) -> i32 {
    match args {
        ["daemon", rest @ ..] => daemon::main(rest),
        ["move", d] if daemon::direction(d).is_some() => client::send(&format!("move {d}")),
        ["click", b] if daemon::button_code(b).is_some() => client::send(&format!("click {b}")),
        ["space"] => client::send("space"),
        ["jump", d @ ("left" | "right")] => client::send(&format!("jump {d}")),
        ["nudge", dx, dy] if dx.parse::<f64>().is_ok() && dy.parse::<f64>().is_ok() => {
            client::send(&format!("nudge {dx} {dy}"))
        }
        ["config", pairs @ ..] if !pairs.is_empty() => config(pairs),
        ["quit"] => match client::try_send("quit") {
            Ok(true) => 0,
            Ok(false) => {
                log!("{}", tr!("the daemon is not running", "el daemon no está corriendo"));
                0
            }
            Err(e) => {
                log!("{e}");
                1
            }
        },
        ["check"] => check(),
        ["binds"] => {
            print!("{}", binds());
            0
        }
        ["version" | "--version" | "-V"] => {
            println!("{}", env!("CARGO_PKG_VERSION"));
            0
        }
        _ => {
            eprint!("{}", if i18n::spanish() { USAGE_ES } else { USAGE });
            2
        }
    }
}

fn config(pairs: &[&str]) -> i32 {
    let errors = Config::default().apply(pairs.iter().copied());
    for e in &errors {
        log!("{e}");
    }
    let msg = format!("config {}", pairs.join(" "));
    if !errors.is_empty() || msg.len() > ipc::MAX_MSG {
        return 2;
    }
    client::send(&msg)
}

fn check() -> i32 {
    let mut ok = true;
    match wayland::VirtualPointer::connect() {
        Ok(_) => println!("{}", tr!("[ok] zwlr_virtual_pointer_manager_v1 available", "[ok] zwlr_virtual_pointer_manager_v1 disponible")),
        Err(e) => {
            println!("{}", tr!("[FAIL] virtual pointer: {e}", "[FALLO] puntero virtual: {e}"));
            ok = false;
        }
    }
    let kbd = Keyboards::open();
    if kbd.is_empty() {
        println!(
            "{}",
            tr!(
                "[FAIL] no readable keyboard in /dev/input (input group + log in again?)",
                "[FALLO] ningún teclado legible en /dev/input (¿grupo input + volver a iniciar sesión?)"
            )
        );
        ok = false;
    } else {
        for k in kbd.iter() {
            println!(
                "{}",
                tr!("[ok] readable keyboard: {} ({})", "[ok] teclado legible: {} ({})", k.path.display(), k.name())
            );
        }
        let held = kbd.state().codes();
        if held.is_empty() {
            println!("{}", tr!("     keys held now: none", "     teclas pulsadas ahora: ninguna"));
        } else {
            println!("{}", tr!("     keys held now: {held:?}", "     teclas pulsadas ahora: {held:?}"));
        }
    }
    match niri::focused_output_width() {
        Some(w) => println!(
            "{}",
            tr!("[ok] niri IPC: focused monitor is {w} logical px wide", "[ok] IPC de niri: monitor enfocado de {w} px lógicos")
        ),
        None => println!(
            "{}",
            tr!(
                "[--] niri IPC did not answer: the monitor jump will use jumpFallback",
                "[--] IPC de niri sin respuesta: el salto de monitor usará jumpFallback"
            )
        ),
    }
    match client::try_send("ping") {
        Ok(true) => println!("{}", tr!("[ok] daemon running", "[ok] daemon corriendo")),
        _ => println!(
            "{}",
            tr!(
                "[--] daemon: not running (the plugin or the first bind starts it)",
                "[--] daemon: no está corriendo (lo arranca el plugin o el primer atajo)"
            )
        ),
    }
    if ok { 0 } else { 1 }
}

fn kdl_string(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Key, command, then the hotkey-overlay title in English and in Spanish.
const BINDS: [(&str, &str, &str, &str); 9] = [
    ("Mod+Ctrl+Left", "move left", "Pointer: left", "Puntero: izquierda"),
    ("Mod+Ctrl+Down", "move down", "Pointer: down", "Puntero: abajo"),
    ("Mod+Ctrl+Up", "move up", "Pointer: up", "Puntero: arriba"),
    ("Mod+Ctrl+Right", "move right", "Pointer: right", "Puntero: derecha"),
    ("Mod+Ctrl+Return", "click left", "Pointer: left click (on release)", "Puntero: clic izquierdo (al soltar)"),
    ("Mod+Ctrl+Shift+Return", "click right", "Pointer: right click (on release)", "Puntero: clic derecho (al soltar)"),
    (
        "Mod+Ctrl+Space",
        "space",
        "Pointer: Space tap=left, double=right, hold=drag",
        "Puntero: Espacio toque=izq, doble=der, mantener=arrastrar",
    ),
    ("Mod+Shift+Ctrl+Left", "jump left", "Pointer: jump to left monitor", "Puntero: saltar al monitor izquierdo"),
    ("Mod+Shift+Ctrl+Right", "jump right", "Pointer: jump to right monitor", "Puntero: saltar al monitor derecho"),
];

fn binds() -> String {
    let exe = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "niri-mousekeys".into());
    let exe = kdl_string(exe.trim_end_matches(" (deleted)"));
    let es = i18n::spanish();
    let mut out = tr!(
        "// Inside binds {{ }} in ~/.config/niri/config.kdl. Comment out the default\n\
         // Mod+Ctrl+arrows and Mod+Shift+Ctrl+Left/Right binds first.\n",
        "// Dentro de binds {{ }} en ~/.config/niri/config.kdl. Comenta antes los\n\
         // Mod+Ctrl+flechas y Mod+Shift+Ctrl+Left/Right originales.\n"
    );
    let width = BINDS.iter().map(|b| b.0.len()).max().unwrap_or(0);
    for (key, cmd, title_en, title_es) in BINDS {
        let args: Vec<String> = cmd.split(' ').map(kdl_string).collect();
        let title = kdl_string(if es { title_es } else { title_en });
        out.push_str(&format!(
            "{key:<width$} repeat=false hotkey-overlay-title={title} {{ spawn {exe} {}; }}\n",
            args.join(" ")
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binds_are_valid_kdl_without_a_shell() {
        let b = binds();
        assert_eq!(b.lines().filter(|l| l.contains("repeat=false")).count(), 9);
        assert!(!b.contains("sh\" \"-c"));
        assert!(b.contains("\"move\" \"left\"; }"));
        assert!(b.starts_with("// ") && b.contains(" binds { } "), "{b}");
        assert_eq!(kdl_string(r#"/a "b"\c"#), r#""/a \"b\"\\c""#);
    }

    #[test]
    fn cli_rejects_invalid_arguments() {
        assert_eq!(run(&["move", "sideways"]), 2);
        assert_eq!(run(&["nudge", "x", "1"]), 2);
        assert_eq!(run(&["config", "vMax=abc"]), 2);
        assert_eq!(run(&[]), 2);
    }
}

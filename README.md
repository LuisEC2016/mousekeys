# Niri Mouse Keys

Dux Shell (DMS) plugin to move the pointer from the keyboard on niri: `Super+Ctrl+arrows` with acceleration, clicks when you release the keys, and Space as a button (tap, double tap and hold to drag).

The plugin is a thin QML layer (daemon supervision and settings page). All the work is done by a Rust binary, `niri-mousekeys`, which speaks the Wayland wire protocol directly (no libwayland) and depends only on `libc`.

## Languages

English is the default. Spanish is also included:

- **Plugin UI** (settings page, toasts, startup check): follows the DMS language. Translations live in [`translations/`](translations); `es.json` covers every string.
- **`niri-mousekeys` binary** (`check`, `binds`, logs, usage): Spanish when the language code is `es` (`es`, `es_MX.UTF-8`, ...), English otherwise. It takes the first entry of `LANGUAGE`, unless the locale (`LC_ALL`, then `LC_MESSAGES`, then `LANG`) is unset, `C` or `POSIX`; otherwise that locale. This is the gettext order, and the same one `Qt.locale()` follows for the DMS UI language, so the binary and the plugin page agree. A language picked in the DMS settings does not reach the binary.

## Shortcuts

| Shortcut | Action |
|---|---|
| `Super+Ctrl+←/→/↑/↓` (hold) | Moves the pointer. Starts slow (fine positioning) and accelerates while you hold the key. |
| `Super+Ctrl+` two arrows | Diagonal movement. |
| `Super+Ctrl+Enter` | Left click: over a window when you **release** all the keys, over a DMS panel at once. Enter twice before releasing = double click (selects a word), three times = triple click (selects a paragraph). More presses are ignored. |
| `Super+Ctrl+Shift+Enter` | Right click, also on release. |
| `Super+Ctrl+Shift+←/→` | The pointer jumps to the adjacent monitor and lands on the same relative spot. |
| `Super+Ctrl+Space` (one tap) | **Left** click, 0.3 s after releasing Space (it waits for a possible second tap). |
| `Super+Ctrl+Space` (double tap) | **Right** click (context menu). |
| `Super+Ctrl+Space` (hold) | **Drag**. Over a DMS panel (a volume slider in the control center): keep `Super+Ctrl+Space` down and move with the arrows. Over a window on stock niri: release Super and Ctrl and keep Space down; after 0.3 s the left button goes down. Move with `Super+Ctrl+`arrows without releasing Space; releasing Space drops the object. |

A short tap moves 2–5 px. Holding the key covers about 70 px in 0.5 s, about 540 px in 1 s, and crosses a 1920 px monitor in about 1.8 s (checked by the `default_curve_matches_the_readme` test).

**Why the click waits for you to release:** while Super is held, stock niri reserves the left click over a window for moving it and the right click for resizing it, and the application would see Ctrl. Over a window the click is queued and goes out once no key is held; if keys are still held after 3 s, it is dropped. Over a DMS panel (bar, control center, notifications) niri lets the click through, so it goes out at once and Super+Ctrl can stay down as a mode. The daemon tells them apart with niri's `pick-window` (the same `window_under` niri uses for those gestures), asked only when a modifier is held. With [patch 0004](#niri-patches) and `dragWaitMods` off, clicks go out at once over windows too; the application then sees Super and Ctrl held, so a Ctrl+click action applies (a link opens in a new tab).

**Why the drag waits for Super and Ctrl:** stock niri takes Super + left button over a window as "move window", so a button pressed there with Super held never reaches the application. Over a DMS panel the drag starts with Super and Ctrl held (with the first arrow, or after `holdMs`). Over a window, with `dragWaitMods` on (the default), the button goes down only once Space has been held for `holdMs` **and** no modifier is held; Space alone keeps the drag going, and `Super+Ctrl+`arrows move it. If you release Space before Super and Ctrl, the drag does not start and nothing is clicked (the log says so). Move only after the button is down: if the pointer moves earlier, the drag does not start either, instead of grabbing whatever the pointer ended up on. Release Super and Ctrl before Space at the end too: an application that sees Ctrl on drop copies instead of moving (file managers). With [patch 0004](#niri-patches) you can turn `dragWaitMods` off: hold `Super+Ctrl+Space` and move with the arrows at the same time; the button goes down with the first arrow (or after 0.3 s), and releasing Space drops.

## How it works

```
niri (bind Super+Ctrl+←, repeat=false)
  └─ spawn: niri-mousekeys move left ──datagram──▶ niri-mousekeys daemon ◀── stdin: settings (DMS)
                                                     │        ▲
             EVIOCGKEY on /dev/input/event* ◀────────┤        └─ supervised by qml/MouseKeysDaemon.qml
             (no grab, 125 Hz only while an arrow,   │
              a click or Space is in progress)       ▼
                                   zwlr_virtual_pointer_v1.motion ──▶ niri moves the cursor
```

- **Client:** each shortcut launches `niri-mousekeys` directly (no `sh -c`), which sends a datagram to an abstract socket (`\0niri-mousekeys-rs-<uid>-<WAYLAND_DISPLAY>`, with a hash of the display instead when an absolute `WAYLAND_DISPLAY` would make the name longer than 107 bytes) and exits.
- **Daemon:** the plugin starts it (`daemon --supervised` with the settings as arguments) and restarts it with growing backoff if it fails. Changes on the settings page reach it over stdin instantly, without restarting it or writing files.
- **Guardian:** Quickshell kills the processes of a `Process` that is destroyed (disabling the plugin, reloading DMS) with SIGKILL, and with SIGKILL there is no way to release a button. That is why `--supervised` splits in two: a minimal guardian, which is the direct child of DMS, and the real daemon in its own session, joined by a pipe. When the guardian dies, even by SIGKILL, the daemon sees the pipe closed, releases the button and exits. The guardian returns the daemon's exit code, so DMS still sees its failures.
- **Without DMS:** if the daemon is not running (DMS stopped or plugin disabled), the first shortcut starts one with the default values. When the plugin starts, it takes over: the previous daemon releases the button if it held one and exits.
- **Socket security:** the abstract namespace has no file permissions, so the daemon checks with `SO_PASSCRED` that every command comes from its own uid. Another user on the machine cannot inject clicks.
- **Stuck button:** niri does not release the buttons of a virtual pointer that goes away. The daemon releases the button on SIGTERM, SIGINT or SIGHUP (via `signalfd`), when the guardian pipe closes, or when it loses the connection. Only a `kill -9` of the daemon itself prevents it.
- **Monitor jump:** the width of the focused monitor is requested from the niri IPC socket (`$NIRI_SOCKET`) without launching `niri msg`.

### Measured footprint

On this machine (niri 26.04, DMS 1.6.0), idle and with both running at the same time:

| | Original Python script | `niri-mousekeys` supervised by DMS (guardian + daemon) |
|---|---|---|
| Processes | 1 | 2 |
| RSS | 14,768 kB | 2,416 + 1,052 kB |
| PSS (real own memory) | 7,716 kB | 246 + 278 = 524 kB |
| `Private_Dirty` | 6,852 kB | 44 + 48 = 92 kB |
| Idle CPU (5 s) | — | 0 ticks |
| Binary | interpreter + script | 511 kB, no dependencies besides libc |

Per gesture (100 `move` with the daemon running, `strace -c`; 1,500 for CPU): about 45 system calls and 0.29 ms of CPU. The paths of the candidate keyboards are kept between gestures and rescanned only when `/dev/input` changes (its mtime), so a gesture opens only the keyboards instead of reading the sysfs capabilities of every input node; the descriptors are still opened and closed per gesture. After the first few hundred gestures the daemon's RSS settles about 300 kB higher (glibc keeps the stacks of the threads that close the keyboards) and stays there.

Measured with `ps -o rss` and `/proc/<pid>/smaps_rollup`; the binary size is from the release build of 2026-11-08 with both languages. Each key press launches a process that takes under 10 ms with the daemon running (`/usr/bin/time` reports 0.00 s) and about 20 ms the first time, when it also starts the daemon.

## Requirements

1. **niri with `zwlr_virtual_pointer_manager_v1`** (niri 26.04 exposes it). Check it with `niri-mousekeys check`.
2. **DMS 1.6 or later.** Tested on 1.6.0 (loading, supervised daemon and settings page); on 2.x it works through the `DankButton` shim in `qs.Widgets`, which leaves a deprecation warning in the log.
3. **Rust/cargo** to build the binary once (the *Build and install* button does it for you).
4. **Read access to `/dev/input/event*`**, usually through the `input` group.

> **Security notice:** being in the `input` group lets **any process of your user** read every key press on every keyboard, as a keylogger would. It is the same permission ydotool, keyd or kanata ask for. `niri-mousekeys` does not grab devices or read events: it only asks which keys are held, and only while it moves the pointer or waits to click. Even so, the permission is granted to the whole user. Weigh it before applying it on a shared machine.
>
> Without that permission it runs in **degraded mode**: each key press moves a fixed 40 px, there is no continuous movement, the click cannot wait for you to release, and Space only left-clicks.

## Installation

### 1. Read access to the keyboards

```sh
groups | grep -qw input || sudo usermod -aG input "$USER"
```

Log out and back in; until then, niri and its children do not see the new group.

### 2. Install and enable the plugin

```sh
dms plugins install niriMouseKeys   # once it is published in the registry
```

Or by hand: clone this repository into `~/.config/DankMaterialShell/plugins/niriMouseKeys`. Then, in **Settings → Plugins**, enable *Niri Mouse Keys*.

### 3. Build the binary

In the plugin settings, press **Build and install**. It is equivalent to:

```sh
cargo install --force --locked --path ~/.config/DankMaterialShell/plugins/niriMouseKeys \
    --root ~/.local --target-dir "${XDG_CACHE_HOME:-$HOME/.cache}/niri-mousekeys"
```

It leaves the binary in `~/.local/bin/niri-mousekeys` and the build artifacts in the cache, outside the plugin folder. After updating the plugin, press the button again: the daemon warns if the binary version does not match the plugin version. `--force` replaces whatever is already at that path (a copy made by hand, a symlink to a development build); without it cargo refuses with `binary niri-mousekeys already exists in destination`.

### 4. niri binds

niri does not allow two binds with the same combination. Inside `binds { }` in `~/.config/niri/config.kdl`, comment out the defaults:

```kdl
    // Mod+Ctrl+Left  { move-column-left; }
    // Mod+Ctrl+Down  { move-window-down; }
    // Mod+Ctrl+Up    { move-window-up; }
    // Mod+Ctrl+Right { move-column-right; }
    // Mod+Shift+Ctrl+Left  { move-column-to-monitor-left; }
    // Mod+Shift+Ctrl+Right { move-column-to-monitor-right; }
```

Those actions remain on `Super+Ctrl+H/J/K/L` and `Super+Ctrl+Shift+H/L`. Check that `Mod+Ctrl+Return`, `Mod+Ctrl+Shift+Return` and `Mod+Ctrl+Space` are free:

```sh
grep -nE 'Mod\+(Ctrl\+(Shift\+)?|Shift\+Ctrl\+)(Left|Right|Up|Down|Return|Space)\b' ~/.config/niri/config.kdl
```

Paste the binds shown on the settings page (**Copy binds**) or printed by `niri-mousekeys binds`. They carry the absolute path of the binary because `spawn` does not expand `~`:

```kdl
    Mod+Ctrl+Left         repeat=false hotkey-overlay-title="Pointer: left" { spawn "/home/USER/.local/bin/niri-mousekeys" "move" "left"; }
    // ... 9 binds in total
```

The `hotkey-overlay-title` texts follow the binary's language (see [Languages](#languages)). `repeat=false` matters: without it, niri would relaunch the binary at the keyboard repeat rate. No `spawn-at-startup` is needed: the plugin starts the daemon with the session.

### 5. Validate and test

```sh
niri validate
niri-mousekeys check
niri-mousekeys nudge 150 0     # the cursor jumps 150 px to the right
niri-mousekeys nudge -150 0    # and back
```

Acceptance test with the physical keyboard:

1. Hold `Super+Ctrl+→`: the cursor glides and accelerates.
2. Over a link, `Super+Ctrl+Enter` and **release everything**: the click arrives.
3. `Super+Ctrl+Shift+Enter` over an app: context menu.
4. With two monitors, `Super+Ctrl+Shift+→` and then `←`.
5. One tap of `Super+Ctrl+Space` over a link and release: left click after 0.3 s.
6. Double tap of `Super+Ctrl+Space` over an app: context menu.
7. Over a file: `Super+Ctrl+Space`, release Super and Ctrl keeping Space down, move it with `Super+Ctrl+`arrows, release Super and Ctrl and then Space. The file moves, not the window. (With `grp:win_space_toggle`, the keyboard layout only stays put with patch 0005.)

## Migrating from the Python script

1. In `config.kdl`, replace the 9 binds `spawn "sh" "-c" "exec python3 ... niri-mousekeys.py ..."` with those from `niri-mousekeys binds`, and delete the line `spawn-at-startup ... niri-mousekeys.py daemon`.
2. `pkill -f 'niri-mousekeys[.]py daemon'` (SIGTERM: it releases the button if it was in the middle of a drag).
3. The values from `~/.config/niri/niri-mousekeys.conf` move to the settings page (times now in ms). The file is no longer read.

The two daemons use different sockets, so they can coexist while you test.

## Settings

All of them are changed on the plugin page and apply instantly. From the command line (for example without DMS): `niri-mousekeys config vMax=2600 rampMs=600`.

| Key | Default | Effect |
|---|---|---|
| `vMin` | 90 px/s | Starting speed. Lower it for more precision. |
| `vMax` | 1800 px/s | Top speed. Raise it with several monitors. |
| `delayMs` | 200 ms | Time at `vMin` before accelerating. |
| `rampMs` | 800 ms | Time from `vMin` to `vMax` (quadratic curve). |
| `minStep` | 2 px | Guaranteed step for each tap. |
| `holdMs` | 300 ms | Time Space must be held to start dragging. |
| `dragWaitMods` | on (`1`) | Over a window, clicks and the drag button wait until Super, Ctrl, Alt and Shift are released (over DMS panels they never wait). Turn it off (`0`) only with patch 0004: they go out at once everywhere, and the application sees the modifiers. |
| `doubleMs` | 300 ms | Maximum gap between two Space taps; also the delay of the single-tap left click. |
| `clickTimeoutMs` | 3000 ms | Maximum wait for you to release the keys before the click is dropped. |
| `fallbackStep` | 40 px | Step per key press in degraded mode. |
| `jumpFallback` | 1920 px | Monitor jump when niri does not answer over IPC. |
| `tickMs`, `clickGapMs` | 8 ms, 40 ms | CLI only: polling period and pause between queued clicks. |

Distances are in niri logical pixels.

## Command line

```
niri-mousekeys check      diagnostics: protocol, readable keyboards, niri IPC, daemon
niri-mousekeys binds      niri binds with the path of this binary
niri-mousekeys quit       stops the daemon (the plugin relaunches it; to stop it, disable the plugin)
niri-mousekeys version
```

Logs: the plugin's daemon writes to the DMS log (lines starting with `niriMouseKeys:`); the one a shortcut starts without DMS writes to `$XDG_RUNTIME_DIR/niri-mousekeys-rs.log`.

## niri patches

Three small patches on top of niri v26.04, all in `src/input/mod.rs`, in [`patches/`](patches). None of them is needed to move the pointer or to click:

| Patch | Needed if... | Without it |
|---|---|---|
| `0003-hide-when-typing-only-typing` | you use `cursor { hide-when-typing }` | the cursor blinks when it starts moving |
| `0004-virtual-pointer-no-mod-gestures` | you want clicks and drags over windows without releasing Super and Ctrl (`dragWaitMods=0`) | with `dragWaitMods=0`, holding Space moves the window instead of dragging the object, and a click with Super held moves or resizes it |
| `0005-bind-keys-keep-layout` | you use `grp:win_space_toggle` or another XKB option that turns Super+Space into a layout switch | every `Super+Ctrl+Space` switches the keyboard layout |

- **0003:** niri hides the cursor on any key, Super or Ctrl included, before checking whether it triggers a bind. The patch only hides it for non-modifier keys that reach the application. Without the patch, the daemon emits a null motion with every command and the cursor reappears as soon as you press the arrow.
- **0004:** in `on_pointer_button`, niri takes any click with Super held as one of its own gestures. The patch makes `zwlr_virtual_pointer_v1` buttons skip those gestures. It affects any virtual pointer client (for example wayvnc).
- **0005:** with `grp:win_space_toggle`, Space is `ISO_Next_Group` whenever Super is held, and smithay switches the layout before niri looks up the bind. The patch restores the layout if the key triggers a bind. Alternative without a patch: another layout shortcut, such as `grp:alt_shift_toggle`.

Build and install:

```sh
git clone https://github.com/niri-wm/niri.git ~/.local/src/niri
cd ~/.local/src/niri && git checkout v26.04
for p in ~/.config/DankMaterialShell/plugins/niriMouseKeys/patches/000[345]-*.patch; do
    git apply --check "$p" && git apply "$p" || { echo "does not apply: $p"; break; }
done
cargo build --release && target/release/niri validate
sudo cp -p "$(command -v niri)" "$(command -v niri).bak-pre-mousekeys"
sudo install -m755 target/release/niri "$(command -v niri)"
```

`install` removes the destination before copying, so it works with niri running. If niri comes from a package (`/usr/bin/niri`), install to `/usr/local/bin/niri` so an update does not overwrite it. The new niri is used the next time you log in.

## Troubleshooting

| Symptom | Likely cause | What to do |
|---|---|---|
| Each key press jumps 40 px, no continuous movement | No access to `/dev/input` | `input` group and log in again. `niri-mousekeys check` reports it. |
| The click never arrives | Some key is still held | Log: `click dropped: keys still held after ... (evdev codes [...])`. Codes are in `/usr/include/linux/input-event-codes.h`. |
| Nothing happens on key press | Binds pointing to another path, or binary not built | `niri-mousekeys check`; the plugin page shows the daemon state. |
| "binary not found" warning when enabling the plugin | Not built yet | **Build and install** button, or the `cargo install` above. |
| The cursor blinks when it starts moving | `hide-when-typing` | Remove the option or apply patch 0003. |
| Every `Super+Ctrl+Space` switches the layout | `grp:win_space_toggle` without patch 0005 | Patch 0005 or change the layout shortcut. `niri msg action switch-layout prev` switches back. |
| Holding Space or clicking moves the window | `dragWaitMods=0` on niri without patch 0004 | Turn `dragWaitMods` back on, or install patch 0004 and check that the running binary is the patched one. |
| Holding Space over a window does nothing | Super and Ctrl are still held (`dragWaitMods`), or the pointer moved first | Release Super and Ctrl while keeping Space down, and only then move. Log: `drag not started: release Super and Ctrl while keeping Space down` or `drag not started: the pointer moved before the button went down`. |
| Dragging a file copies it | The app sees Ctrl on drop | Release Super and Ctrl before Space. |
| The left button stays stuck | The daemon died from `kill -9` in the middle of a drag | A click with the mouse or touchpad, or a short drag with `Super+Ctrl+Space`. |

## Uninstall

1. Disable the plugin (which stops the daemon with SIGTERM) and remove it from **Settings → Plugins** or with `dms plugins uninstall niriMouseKeys`.
2. Delete the 9 binds and uncomment the 6 defaults in `config.kdl`.
3. `rm ~/.local/bin/niri-mousekeys` and `rm -r "${XDG_CACHE_HOME:-$HOME/.cache}/niri-mousekeys"`.
4. If you added the group only for this: `sudo gpasswd -d "$USER" input` and log in again.

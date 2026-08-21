# ZJU Connect GUI

Native desktop wrapper for the [`zju-connect`](https://github.com/Mythologyli/zju-connect) CLI,
built in Rust with a web frontend (axum + Alpine.js + Pico.css).

The app runs a local HTTP server, opens your browser to manage the connection, and stays in the
system tray. No Electron, no Wails — just a small Rust binary and a browser tab.

## Building

Requires a stable Rust toolchain (pinned by `rust-toolchain.toml`).

```sh
# Linux
cargo build --release
cargo test

# Windows (see note below)
RUSTUP_TOOLCHAIN=stable-x86_64-pc-windows-gnullvm cargo build --release
```

### Windows build note

The host MSVC toolchain conflicts with `link.exe` from coreutils (a hard-link utility, not the
MSVC linker). The project uses the `x86_64-pc-windows-gnullvm` toolchain as a workaround, which
requires MinGW-w64 installed. See `.cargo/config.toml` for linker configuration. macOS and Linux
builds are unaffected.

### Cross-compile from Linux to Windows

```sh
cargo build --release --target x86_64-pc-windows-gnu
# → target/x86_64-pc-windows-gnu/release/zju-connect-gui.exe
```

### Linux AppImage

```sh
bash scripts/build_linux_appimage.sh
# → zju-connect-gui-x86_64.AppImage
```

## Layout

```
src/
  backend/                   platform-agnostic core: launch options, settings
                             store, pending-connect store, relaunch args, paths,
                             external links (open EIP), proxy/ supervisor, and
                             platform/ (windows elevation + console signaling,
                             unix stubs)
  web/                       HTTP server + frontend
    server.rs                axum router, port selection, SSE endpoint
    handlers.rs              REST API: start/stop, settings, status, elevate
    bridge.rs                WebUiBridge: ProxyEvent → SSE broadcast channel
    assets.rs                embedded frontend assets (Pico.css, Alpine.js, htmx)
  tray.rs + tray/            system tray (ksni on Linux, tray-icon on Windows/macOS);
                             left-click opens browser, menu has show + quit
  main.rs                    entrypoint: single-instance lock, ProxyManager init,
                             web server + tray start, elevation flow, graceful shutdown
  lib.rs                     re-exports backend module for tests
web/
  index.html                 Alpine.js SPA: settings, logs, captcha/input modals
  static/                    frontend libraries (embedded at compile time)
tests/
  proxy_manager.rs           integration tests (real shell-script mock binary)
assets/                      icons (gemini.png, gemini.ico, gemini.svg)
packaging/linux/             .desktop file + AppImage AppRun launcher
scripts/                     build_linux_appimage.sh
.github/workflows/           ci.yml + build-packages.yml
```

## Architecture notes

- `src/backend/proxy/manager.rs` directly spawns `zju-connect` via `tokio::process::Command`
  with stdin/stdout/stderr piped. Logs flow over pipes; `SubmitInput` writes to the child's stdin.
- On Windows, `src/backend/platform/windows_impl.rs` allocates a hidden console
  at startup so `GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, child_pid)` can
  deliver graceful shutdown to the child without a console window flashing.
  When TUN mode is requested from a non-elevated process, the app calls
  `ShellExecuteW("runas")` to relaunch itself elevated, then quits; the
  elevated copy waits for the un-elevated parent's pid to exit, reads the
  pending-connect marker, and resumes the connection automatically.
- The web server binds to `127.0.0.1` on a high port (tries last-used port from
  `web_port.txt`, falls back to OS-assigned random). The port is written to
  `web_port.txt` in the app directory for persistence across restarts.
- Real-time updates flow over SSE (`GET /api/events`). Backend events from
  `ProxyManager` are converted to JSON by `WebUiBridge` and broadcast via
  `tokio::sync::broadcast`, then streamed to the browser.
- The tray icon lives in `src/tray/`, split by platform: Linux uses `ksni`
  (StatusNotifierItem over zbus), while Windows and macOS use `tray-icon`
  with a Win32 message pump on a dedicated thread. Tray creation is
  best-effort — on unsupported desktops the app starts without a tray (warning logged).
- Single-instance enforcement runs at startup
  (`platform::acquire_single_instance` in `src/main.rs`). Unix uses
  `flock(LOCK_EX|LOCK_NB)` on `app_dir/instance.lock`; Windows uses a
  `Local\` named mutex. A second launch logs and exits cleanly with status 0.
- The web UI is protected by a per-launch random token: the app opens
  `http://localhost:{port}/?token=...`, the page sends it as the
  `X-Auth-Token` header (SSE passes `?token=`), and the server validates the
  `Host` header against `localhost`/`127.0.0.1` to blunt DNS-rebinding and
  local-process attacks on the (possibly elevated) API.
- Closing the browser tab does NOT stop the background process. Quit only via
  the tray menu or Ctrl+C in the terminal.

## Known limitations

- The EIP browser file-picker uses each platform's stock dialog helper
  (PowerShell on Windows, zenity/kdialog/yad on Linux, osascript on macOS);
  on a desktop that ships none of them, pick a detected browser from the
  list instead.
- macOS compiles but is not exercised in CI.
- On GNOME the tray icon requires the AppIndicator/KStatusNotifierItem
  shell extension (KDE Plasma works out of the box).
- Credentials are stored in plaintext in `gui_settings.json` (0600 on Unix)
  and passed to `zju-connect` on its command line — visible in the process
  list (`ps` / Process Explorer). This is an upstream CLI contract.

## License

MIT.

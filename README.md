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

### Static musl release builds (Linux)

The official Linux artifacts are built as fully static musl binaries, so they run on any
distribution regardless of its glibc version:

```sh
sudo apt install musl-tools   # provides musl-gcc
CC_x86_64_unknown_linux_musl=musl-gcc \
  cargo build --release --target x86_64-unknown-linux-musl
```

Any Linux build compiles the vendored C in `vendor/` (ProxyBridge and its netfilter
dependencies) into the binary, so a C compiler is required. For a `*-linux-musl` target that
compiler must target musl — see `vendor/README.md`.

### Windows release builds

The official Windows artifacts link the VCRuntime **statically** while leaving the Universal
CRT dynamic (`static_vcruntime` in `build.rs`). The UCRT is a component of Windows 10+, but
`vcruntime140.dll` ships with the VC++ Redistributable — so the released `.exe` runs on a stock
Windows install with no prerequisites. Only the optional ProxyBridge support needs the bundled
`proxybridge/` directory next to it (and admin rights, for the WinDivert driver).

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

## Release packages

`build-packages.yml` publishes one archive per platform (`.tar.gz` for Linux and
macOS, `.zip` for Windows). Every one of them unpacks to the same shape, with the
GUI at the root and the `zju-connect` core in `bin/` — the exact relative path the
GUI launches, so extracting the archive is all the setup there is:

```
zju-connect-gui          # zju-connect-gui.exe on Windows
bin/zju-connect          # bin\zju-connect.exe on Windows
```

The core is the matching asset from the latest
[Mythologyli/zju-connect](https://github.com/Mythologyli/zju-connect) release
(`linux-amd64`/`linux-arm64`, `windows-amd64`/`windows-arm64`,
`darwin-arm64`), fetched at build time; the build log prints the tag it used.

Windows x86_64 additionally ships `proxybridge/` (`WinDivert.dll`,
`WinDivert64.sys`). ProxyBridge itself is compiled into the executable from the
vendored source in `vendor/proxybridge-win-4.0.0/`, so no core DLL ships or is
loaded at run time, and `WinDivert.dll` is loaded from that directory on first
use — nothing has to sit next to the executable. That optional feature installs
the signed WinDivert driver as a kernel service the first time it runs, which
needs administrator rights. Windows arm64 has no `proxybridge/` — there is no
ARM64 WinDivert driver, so the feature is unavailable there.

The archives contain only the binaries. The sources and licence texts for the
libraries linked into them live in `vendor/` and are not redistributed.

## Layout

```
src/
  backend/                   platform-agnostic core: launch options, settings
                             store, pending-connect store, relaunch args,
                             elevation handshake, paths, external links (open
                             EIP), proxy/ supervisor, and platform/ (Windows
                             elevation + console signaling, Linux pkexec
                             elevation, unix helpers)
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
- Elevation (TUN mode / ProxyBridge) always means *relaunching this app* with
  administrator rights, because ProxyBridge intercepts in-process:
  `ShellExecuteW("runas")` on Windows, `pkexec` (polkit) on Linux. The relaunch
  is coordinated by a handshake (`src/backend/elevation.rs`): the parent
  generates a random token, the elevated instance creates
  `app_dir/.elevate-ready-<token>` as soon as it is running, and only then does
  the parent release the single-instance lock and exit. That is what keeps a
  dismissed or impossible authorization prompt from turning into an endless
  "waiting for authorization" state: the parent reports the real outcome
  (cancelled / no agent / refused / timeout) and stays alive for a retry.
- On Linux, `src/backend/platform/pkexec.rs` runs the app through
  `pkexec --disable-internal-agent /usr/bin/env DISPLAY=… …`, so the polkit
  dialog is drawn by the session's authentication agent (统信 UOS / Deepin:
  `dde-polkit-agent`) instead of pkexec's textual agent on a terminal that a
  desktop launch does not have. The elevated instance clears the parent-death
  signal `pkexec` sets (otherwise it would be killed the moment the original
  GUI exits), detaches stdio, hands root-created state files back to
  `PKEXEC_UID`, and opens the browser in that user's session — Chrome-family
  browsers refuse to start as root.
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
- Linux elevation needs a graphical polkit agent and a setuid `/usr/bin/pkexec`.
  Both are checked before the authorization request is made, and the UI shows a
  concrete message (missing agent / stripped setuid bit / dismissed prompt) if
  it cannot proceed. Without a polkit agent the app can still be started from a
  terminal with `sudo`, which skips the relaunch entirely.
- On GNOME the tray icon requires the AppIndicator/KStatusNotifierItem
  shell extension (KDE Plasma works out of the box).
- Elevation runs the binary at its current path with administrator rights. In
  the AppImage build that path is the copy the launcher keeps under
  `$XDG_STATE_HOME/zju-connect-gui/appimage/`, i.e. inside the user's home:
  keep it private (the default `0700`) — anything that can replace that binary
  could otherwise obtain root through the authorization prompt.
- Credentials are stored in plaintext in `gui_settings.json` (0600 on Unix)
  and passed to `zju-connect` on its command line — visible in the process
  list (`ps` / Process Explorer). This is an upstream CLI contract.

## License

MIT for this repository's own code.

The Linux release artifacts also link in two GPL-2.0 libraries (`libnetfilter_queue`,
`libnfnetlink`) and one LGPL-2.1 library (`libmnl`) as part of the ProxyBridge integration, so
those binaries as a whole are distributed under GPL-2.0. Their sources and license texts are kept
in `vendor/` — see `vendor/README.md`.

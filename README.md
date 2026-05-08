# ZJU Connect GUI

Native desktop wrapper for the [`zju-connect`](https://github.com/Mythologyli/zju-connect) CLI,
built in Rust + [Slint](https://slint.dev/). Replaces the previous Go + iup-go
implementation; the prior Wails-era visual language is preserved.

## Status

This branch (`refactor/rust-slint`) is a port-in-progress. The Go tree is gone;
all source now lives under `src/` (Rust) and `ui/` (Slint).

## Building

Requires a stable Rust toolchain (pinned by `rust-toolchain.toml`).

```sh
# Linux runtime deps (Arch package names; substitute for your distro)
sudo pacman -S --needed libxkbcommon fontconfig libxcb libxcursor libxrandr libxi pkgconf

cargo build              # debug
cargo build --release    # optimized + LTO + symbol strip
cargo run                # launch the GUI
cargo test               # 36 unit tests + 5 proxy-manager integration tests
cargo fmt && cargo clippy --all-targets -- -D warnings
```

### Cross-compile to Windows

A `mingw-w64` toolchain is configured in `.cargo/config.toml`:

```sh
sudo pacman -S mingw-w64-gcc mingw-w64-binutils mingw-w64-crt mingw-w64-headers mingw-w64-winpthreads
rustup target add x86_64-pc-windows-gnu
cargo build --release --target x86_64-pc-windows-gnu
# → target/x86_64-pc-windows-gnu/release/zju-connect-gui.exe
```

For an MSVC build (used by CI), use `--target x86_64-pc-windows-msvc` with
the Visual Studio Build Tools installed.

### Linux AppImage

```sh
bash scripts/build_linux_appimage.sh
# → zju-connect-gui-x86_64.AppImage
```

## Layout

```
src/
  backend/                 platform-agnostic core: launch options, settings
                           store, pending-connect store, relaunch args, paths,
                           external links (open EIP), proxy/ supervisor, and
                           platform/ (windows elevation + console signaling,
                           unix stubs)
  app.rs                   App coordinator: wires Slint UI ↔ ProxyManager,
                           settings, persist debounce, resume-after-elevation
  ui_glue.rs               UiBridge: marshals ProxyEvents onto the slint event
                           loop and updates AppWindow properties
  main.rs                  entrypoint: console init, argv parse, App::run
ui/
  theme.slint              design-token global (colors, radii, spacing, motion)
  main.slint               AppWindow component (header, tabs, settings card,
                           log panel, FAB, modals)
tests/
  proxy_manager.rs         integration tests (real shell-script mock binary)
assets/                    icons (gemini.png, gemini.ico, gemini.svg)
packaging/linux/           .desktop file + AppImage AppRun launcher
scripts/                   build_linux_appimage.sh
.github/workflows/         ci.yml + build-packages.yml (cargo-based)
```

## Architecture notes

- `src/backend/proxy/manager.rs` directly spawns `zju-connect` via `tokio::process::Command`
  with stdin/stdout/stderr piped; the Go-era PowerShell supervisor is gone.
  Logs flow over pipes; `SubmitInput` writes to the child's stdin instead of
  a polling input file.
- On Windows, `src/backend/platform/windows_impl.rs` allocates a hidden console
  at startup so `GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, child_pid)` can
  deliver graceful shutdown to the child without a console window flashing.
  When TUN mode is requested from a non-elevated GUI, the app calls
  `ShellExecuteExW("runas")` to relaunch itself elevated, then quits; the
  elevated copy waits for the un-elevated parent's pid to exit, reads the
  pending-connect marker, and resumes the connection automatically.
- Slint UI updates from tokio worker threads are routed through
  `slint::invoke_from_event_loop`. The log model is a `VecModel<SharedString>`
  set on the window once and mutated in-place via downcast from `get_logs()`.

## Known limitations (v1)

- No system tray icon yet (planned for a follow-up commit).
- No single-instance lock (will be added with the tray work since both touch
  the startup path).
- The "browse..." button for the EIP browser program is a stub (paste the
  path manually for now).
- macOS compiles but is not exercised in CI.
- Title-bar drag uses the OS chrome; the frameless Wails-style bar will
  return once the cross-platform drag plumbing in Slint is hooked up.

## License

MIT.

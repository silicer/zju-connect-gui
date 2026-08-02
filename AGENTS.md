# AGENTS.md

Operational guide for AI agents (and humans pairing with them) working on
this repository.

## Repo at a glance

- Language: Rust (edition 2021, stable toolchain pinned by `rust-toolchain.toml`).
- UI: Web frontend served by axum on localhost (Alpine.js + Pico.css + SSE).
- Async: tokio multi-thread runtime; child supervision lives in
  `src/backend/proxy/manager.rs`.
- Frontend libraries (Alpine.js, Pico.css, htmx) are embedded via `include_str!`
  in `src/web/assets.rs` at compile time — no CDN dependency at runtime.

## Hard rules

- **Never re-introduce the Go tree** or any of its build artifacts. The Wails
  webview was the original reason for this rewrite — bringing it back is a
  regression.
- **Don't strip the Linux fallback to start_kill** in `proxy::manager`. SIGINT
  / CTRL_BREAK can fail (process already dying, console missing) and the kill
  fallback after the grace period mirrors the original Go semantics.
- **Don't pass CREATE_NO_WINDOW to spawned children** on Windows. That detaches
  the child from any console handle, which breaks `GenerateConsoleCtrlEvent`.
  The hidden console allocated by `platform::init_console_for_signaling` is
  load-bearing.

## Where things live

```
src/
  backend/                     OS-agnostic core
    launch_options.rs          fields + normalize + validate + build_args
    settings_store.rs          gui_settings.json (reapplies fixed defaults
                               on load AND save)
    pending_connect_store.rs   gui_pending_connect.json with 5-min TTL
    relaunch_args.rs           --resume-pending-connect --wait-parent-pid=N
    paths.rs                   resolve_app_dir() = parent of current_exe
    external_links.rs          open_eip + EIP_URL
    proxy/                     supervisor + helpers
      manager.rs               ProxyManager, supervise_child task,
                               retry/readiness/eip-open generation logic
      proxybridge.rs           in-process libproxybridge.so / ProxyBridgeCore.dll
                               binding (dlopen + C API; macOS stubbed out)
      logs.rs                  chunked stream reader + prompt detection
      readiness.rs             HTTP-bind dial poll
      captcha.rs               60s deadline, size-stable file polling
      retry.rs                 ±20% jitter exponential backoff
    platform/                  cfg-gated: windows_impl.rs vs unix_impl.rs
                               public API: is_process_elevated,
                               relaunch_self_elevated, signal_child_to_quit,
                               wait_for_process_exit, escape_arg,
                               init_console_for_signaling,
                               acquire_single_instance, SingleInstanceGuard
  web/                         HTTP server + SSE bridge
    server.rs                  axum router, port selection (try-last, then
                               OS-assigned), SSE endpoint with BroadcastStream
    handlers.rs                REST handlers: /api/settings, /api/start,
                               /api/stop, /api/submit-input, /api/submit-captcha,
                               /api/clear-logs, /api/elevate, /api/status
    bridge.rs                  WebUiBridge: implements UiBridge trait, converts
                               ProxyEvent → SseEvent → broadcast send
    assets.rs                  include_str!-embedded frontend files
  tray.rs + tray/              Linux ksni impl, Windows/macOS tray-icon impl;
                               left-click opens browser, menu has 打开网页 + 退出
  main.rs                      console init, single-instance lock acquire,
                               argv parse, web server start, tray init,
                               elevation flow, graceful shutdown
web/
  index.html                   Alpine.js SPA: settings tab, logs tab, captcha
                               modal (click-to-mark), input modal, start/stop
                               buttons, status pill, SSE event handlers
  static/
    alpine.min.js              Alpine.js 3.14.1
    htmx.min.js                htmx 1.9.12
    pico.min.css               Pico.css v2
tests/
  proxy_manager.rs             integration tests with shell-script mock binary
```

## Adding a new launch_options field

1. Add the field to `LaunchOptions` in `src/backend/launch_options.rs`.
2. If it has a default, add a `DEFAULT_*` constant and seed it in
   `normalize_launch_options`.
3. If it's user-tunable, expose it in `web/index.html` as a field in the
   Alpine.js `settings` data object and add a UI control in the settings tab.
4. The frontend sends settings to `POST /api/settings` which calls
   `LaunchOptions::normalize_and_validate`, so no additional Rust wiring is
   needed for simple fields.
5. Update `build_args` if it should reach the CLI.
6. Add a unit test in `launch_options.rs` if the value goes through validation
   or normalization.

## Adding a backend → UI event

1. Extend `ProxyEvent` in `src/backend/proxy/manager.rs`.
2. Emit it from the appropriate manager state transition.
3. Add the variant mapping in `SseEvent::from_proxy_event` in
   `src/web/bridge.rs`.
4. Handle the SSE event type in the Alpine.js `connectSse()` function in
   `web/index.html`.

## API reference

| Method | Path | Purpose |
|--------|------|---------|
| GET | `/api/settings` | Load current settings |
| POST | `/api/settings` | Save settings (partial merge) |
| POST | `/api/start` | Start proxy; returns 412 if elevation needed |
| POST | `/api/stop` | Stop proxy |
| POST | `/api/submit-input` | Submit SMS/credential input |
| POST | `/api/submit-captcha` | Submit captcha coordinates |
| POST | `/api/clear-logs` | Clear log buffer |
| POST | `/api/elevate` | Trigger UAC elevation (Windows only) |
| GET | `/api/status` | Snapshot of current proxy state |
| GET | `/api/events` | SSE stream: log, state, need_input, need_captcha, error |
| GET | `/` | Serve index.html |
| GET | `/static/{path}` | Serve embedded static files |

## Verification gates (run before committing)

```sh
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
```

## What's deferred (intentionally)

- Native file picker for the EIP browser path (manual paste only)
- macOS validation (compiles, not exercised on CI)

## Known build pitfalls

- On Windows, the MSVC toolchain (`stable-x86_64-pc-windows-msvc`) conflicts
  with `link.exe` from coreutils (a Unix hard-link tool). Build with
  `RUSTUP_TOOLCHAIN=stable-x86_64-pc-windows-gnullvm` instead. Requires
  MinGW-w64 (e.g. `scoop install mingw-mstorsjo-llvm-ucrt`).

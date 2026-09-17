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
    external_links.rs          open_eip (opens the EIP portal in the
                               configured browser — new window by default,
                               OS default handler when unset; never touches
                               the browser's proxy settings) + EIP_URL
    browser_detect.rs          installed-browser detection (Windows registry
                               StartMenuInternet + known paths, unix PATH
                               scan) + native file-picker dialog (PowerShell
                               / zenity / kdialog / yad / osascript)
    proxy/                     supervisor + helpers
      manager.rs               ProxyManager, supervise_child task,
                               retry/readiness/eip-open generation logic.
                               The automatic EIP open is latched once per GUI
                               run (`eip_opened`), never re-armed by a
                               reconnect
      proxybridge.rs           ProxyBridge C API binding, statically linked on
                               Linux and Windows x86_64 (macOS/Windows arm64
                               stubbed out; upstream's DLL is no longer used)
      dns_probe.rs             readiness probe for the core's tunnel-backed
                               DNS server before the UDP DNS hijack is armed
                               (Linux and Windows x86_64)
      windivert.rs             WinDivert kernel driver ensure/install/start
                               (Windows only, no-op elsewhere)
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
                               OS-assigned), per-launch token auth middleware
                               (X-Auth-Token header / ?token= query), SSE
                               endpoint with BroadcastStream
    handlers.rs                REST handlers: /api/settings, /api/start,
                               /api/stop, /api/submit-input, /api/elevate,
                               /api/status, /api/browsers,
                               /api/select-browser-file, /api/open-eip
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
vendor/                        C sources compiled into the binary by build.rs
                               (Linux: ProxyBridge + its netfilter stack;
                               Windows x86_64: ProxyBridge, plus a shim that
                               loads WinDivert from `proxybridge/` at run
                               time). Provenance, licensing and the local
                               patches are documented in vendor/README.md
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
| POST | `/api/submit-input` | Submit SMS/callback input or captcha coordinates (`value` + optional `kind`) |
| POST | `/api/elevate` | Trigger elevation flow (Windows only; no-op signal on other OSes) |
| GET | `/api/status` | Snapshot of current proxy state |
| GET | `/api/browsers` | List locally installed browsers (name/path/chrome-firefox kind) |
| POST | `/api/select-browser-file` | Open a native file-picker dialog; returns `{path}` or `{path: null}` on cancel; 409 while a dialog is already open |
| POST | `/api/open-eip` | Open the EIP portal in the browser; an optional `{options}` body (the UI's live settings) overrides the settings store, so a freshly picked browser applies without reconnecting; no session required |
| GET | `/api/events` | SSE stream: log, state, need_input, need_captcha, error |
| GET | `/` | Serve index.html |
| GET | `/static/{path}` | Serve embedded static files |

Auth: every `/api/*` route (including SSE) requires the per-launch token — pass it as the `X-Auth-Token` header; only the SSE stream additionally accepts `?token=` (EventSource cannot set headers), so a leaked `?token=` URL cannot drive state-changing requests. The app opens `http://localhost:{port}/?token=...`; the token is regenerated each launch and never persisted. The server also rejects requests whose `Host` header is not `localhost:{port}` / `127.0.0.1:{port}` (case-insensitive, DNS-rebinding protection).

## Verification gates (run before committing)

```sh
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
```

## What's deferred (intentionally)

- macOS validation (compiles, not exercised on CI)

## Known build pitfalls

- On Windows, the MSVC toolchain (`stable-x86_64-pc-windows-msvc`) conflicts
  with `link.exe` from coreutils (a Unix hard-link tool). Build with
  `RUSTUP_TOOLCHAIN=stable-x86_64-pc-windows-gnullvm` instead. Requires
  MinGW-w64 (e.g. `scoop install mingw-mstorsjo-llvm-ucrt`).
- On Linux, `build.rs` compiles the vendored C in `vendor/` into the binary, so
  a C compiler is required. For a `*-linux-musl` target it must target musl
  (`CC_<triple>=musl-gcc`, i.e. `musl-tools`): the default host `cc` emits
  glibc-only references such as `__ctype_b_loc`, and the link fails. `zig cc`
  also works but needs a wrapper — see the header of `build.rs`.
- On Windows x86_64, `build.rs` compiles the vendored ProxyBridge source as
  well, so a C compiler is required: `cl.exe` on the MSVC targets CI uses, or
  mingw/clang for a `*-pc-windows-gnu` build. Nothing else: the WinDivert DLL
  is not linked against — it is loaded at run time from the package's
  `proxybridge/` directory (`vendor/proxybridge-win-4.0.0/windivert_dynamic.c`),
  so there is no import library to generate and nothing is downloaded.
- `musl-gcc` does not search `/usr/include` at all (its specs file replaces the
  include path), so the kernel UAPI headers need `linux-libc-dev` on
  Debian/Ubuntu; `build.rs` puts them on the include path with `-idirafter`.

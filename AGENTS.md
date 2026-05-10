# AGENTS.md

Operational guide for AI agents (and humans pairing with them) working on
this repository.

## Repo at a glance

- Language: Rust (edition 2021, stable toolchain pinned by `rust-toolchain.toml`).
- UI: Slint 1.16 with the femtovg renderer over winit.
- Async: tokio multi-thread runtime; child supervision lives in
  `src/backend/proxy/manager.rs`.
- Branch: `refactor/rust-slint`. The Go tree was deleted in commit 2fd9483
  during phase 1 of the rewrite.

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
                               on load AND save — see fragile-invariants
                               section in plans/gleaming-popping-valiant.md)
    pending_connect_store.rs   gui_pending_connect.json with 5-min TTL
    relaunch_args.rs           --resume-pending-connect --wait-parent-pid=N
    paths.rs                   resolve_app_dir() = parent of current_exe
    external_links.rs          open_eip + EIP_URL
    proxy/                     supervisor + helpers
      manager.rs               ProxyManager, supervise_child task,
                               retry/readiness/eip-open generation logic
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
  app.rs                       App coordinator. Wires Slint UI ↔ manager,
                               settings persistence, resume-after-elevation,
                               tray controller (Option, best-effort)
  ui_glue.rs                   UiBridge: ProxyEvent → AppWindow via
                               slint::invoke_from_event_loop. Log model is
                               looked up via window.get_logs().as_any().downcast
  tray.rs + tray/              Linux ksni impl, Windows/macOS tray-icon impl;
                               left-click toggles window, menu has show + quit
  main.rs                      console init, single-instance lock acquire,
                               argv parse, app::App::run
ui/
  theme.slint                  design tokens
  main.slint                   AppWindow + components inline
tests/
  proxy_manager.rs             integration tests with shell-script mock binary
```

## Adding a new launch_options field

1. Add the field to `LaunchOptions` in `src/backend/launch_options.rs`.
2. If it has a default, add a `DEFAULT_*` constant and seed it in
   `normalize_launch_options`.
3. If it's user-tunable (not a fixed default like `protocol`), expose it in
   `ui/main.slint` as an `in-out property` on `AppWindow`.
4. Wire reads in `read_options_from_window` and writes in
   `apply_options_to_window` (`src/app.rs`).
5. Hook the `changed` callback on the new property in `ui/main.slint` so the
   debounced autosave fires.
6. Update `build_args` if it should reach the CLI.
7. Add a unit test in `launch_options.rs` if the value goes through validation
   or normalization.

## Adding a backend → UI event

1. Extend `ProxyEvent` in `src/backend/proxy/manager.rs`.
2. Emit it from the appropriate manager state transition.
3. Handle it in `apply_event` (`src/ui_glue.rs`).
4. If it requires UI state, add a property to `AppWindow` and write to it in
   the handler.

## Verification gates (run before committing)

```sh
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
```

Cross-compile sanity (does not need Windows):

```sh
cargo build --target x86_64-pc-windows-gnu
```

## What's deferred (intentionally)

- Frameless window with custom drag (uses OS chrome for now)
- Native file picker for the EIP browser path (manual paste only)
- macOS validation (compiles, not exercised on CI)

These are tracked as follow-up work; see plans/gleaming-popping-valiant.md
for the original phase plan.

## Known build pitfalls

- rustc 1.95.0 has an incremental-cache ICE (`invalid enum variant tag while
  decoding SymbolExportKind`) that fires under our slint+ksni dep set after a
  `cargo clean`. Workaround: `CARGO_INCREMENTAL=0 cargo build` (and the same
  for `test` / `clippy`). The bug is upstream; remove the env var once we
  bump the toolchain past it.

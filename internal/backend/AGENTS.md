# BACKEND GUIDE

## OVERVIEW
`internal/backend/` owns `zju-connect` process orchestration, launch args, persistence, prompt/captcha flow, and OS-specific behavior.

The package stays flat on purpose. The main cleanup was splitting the old `proxy_manager.go` hotspot into several files in
the same package so responsibility boundaries are easier to follow without changing backend wiring.

## WHERE TO LOOK
| Task | File | Notes |
|------|------|-------|
| Proxy manager entrypoints/shared state | `proxy_manager.go` | `ProxyManager` type, public entrypoints, shared runtime helpers |
| Process lifecycle and retry flow | `proxy_manager_lifecycle.go` | Start/stop orchestration, reconnect scheduling, backoff |
| Log streams and prompt/captcha detection | `proxy_manager_logs.go` | Stream consumption, line handling, prompt classification |
| Readiness transitions | `proxy_manager_readiness.go` | HTTP bind readiness, connected state, EIP open-once |
| Bounded file polling | `proxy_manager_polling.go` | Captcha polling, captcha file monitoring, elevated PID/process waits |
| Windows elevated process helpers | `proxy_manager_elevated.go` | Elevated launch script, elevated stop flow, PowerShell quoting |
| CLI arg shape | `launch_options.go` | `LaunchOptions`, validation, hidden defaults, TUN/debug flags |
| Saved GUI settings | `user_settings_store.go` | Reads/writes `gui_settings.json`; re-applies fixed defaults on load/save |
| Elevated resume marker | `pending_connect_store.go` | One-shot resume state for Windows TUN relaunch |
| App directory resolution | `paths.go` | Keep runtime files app-relative |
| Windows self-elevation | `self_elevation_windows.go`, `elevation_windows.go` | Whole-app elevation path |
| Platform stop/process behavior | `signal_*.go`, `process_attrs_*.go`, `process_state_*.go` | Keep OS-specific logic split by file |

## FIXED DEFAULTS
These are not mere frontend defaults. They are enforced in arg building and settings persistence:

- `-protocol atrust`
- `-server sslvpn.scmcc.com.cn`
- `-port 443`
- `-disable-zju-config`
- `-secondary-dns-server 223.5.5.5`
- `-auth-type auth/psw`
- `-login-domain AD`
- `-client-data-file client_data.json`

Additional flags:

- TUN mode: `-tun-mode -add-route -dns-hijack -fake-ip`
- Debug mode: `-debug-dump`

If any launch option meaning changes, update `launch_options.go`, `user_settings_store.go`, and the frontend fixed-parameter list together.

## WINDOWS RULES
- TUN mode requires elevated application restart on Windows.
- Current design is whole-app elevation plus pending resume. Keep that contract.
- Graceful stop and signal handling are platform-specific. Do not collapse `*_windows.go` and `*_other.go` into generic logic unless behavior is proven identical.
- Relative runtime files matter: PID/input/stop/log helper files are created beside the app during elevated flows.
- Keep bounded polling for captcha/PID helper files unless there is a demonstrated correctness or latency problem; file
  watchers would still need stability checks and platform-specific handling.

## TESTS
Tests currently live only here:

- `launch_options_test.go`
- `proxy_manager_test.go`
- `user_settings_store_test.go`

Add or update tests here when changing launch normalization, backend persistence, or log-driven helper behavior.

## ANTI-PATTERNS
- Do not hardcode CLI flags outside `launch_options.go` and settings enforcement.
- Do not bypass `ProxyManager` to spawn or kill `zju-connect` directly.
- Do not move fixed defaults into user-editable state unless the UI and persistence model are intentionally changed.
- Do not assume logs are cosmetic; prompt detection and startup success are log-driven.
- Do not treat Unix and Windows stop semantics as equivalent.

# BACKEND GUIDE

## OVERVIEW
`internal/backend/` owns `zju-connect` process orchestration, launch args, persistence, prompt/captcha flow, and OS-specific behavior.

## WHERE TO LOOK
| Task | File | Notes |
|------|------|-------|
| Process lifecycle | `proxy_manager.go` | Main hotspot; start/stop, stream reads, prompt detection, EIP open-once |
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

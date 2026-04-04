# PROJECT KNOWLEDGE BASE

**Generated:** 2026-03-19
**Commit:** 36f3845
**Branch:** master

## OVERVIEW
`zju-connect-gui` is an IUP-Go native desktop wrapper around the `zju-connect` CLI binary. The Go side owns process lifecycle, tray behavior, elevation, persistence, the native UI event bridge, and packaged icon assets.

## STRUCTURE
```text
./
├── assets/                   # App/tray icon sources used by go:embed
├── docs/reference/           # Internal migration and IUP reference notes
├── main.go                  # IUP bootstrap, tray start, elevated relaunch wait
├── app.go                   # App API, window actions, start/stop, resume flow
├── ui_iup.go                # Native IUP UI: config/log tabs, prompts, captcha canvas
├── tray.go                  # Systray menu and quit/restore actions
├── tray_icon_*.go           # Platform-specific tray icon bytes
├── internal/backend/        # CLI orchestration, persistence, OS split helpers, tests
```

## WHERE TO LOOK
| Task | Location | Notes |
|------|----------|-------|
| Native startup or window lifecycle | `main.go`, `app.go`, `ui_iup.go` | IUP init, hide-to-tray close behavior, startup/shutdown hooks |
| Tray UX | `tray.go`, `tray_icon_*.go` | Menu actions only; current systray version has no icon double-click callback |
| Start/stop entrypoints and shared backend state | `internal/backend/proxy_manager.go` | `ProxyManager` type, public API, shared runtime helpers |
| Retry/lifecycle, log parsing, readiness, polling, elevated flow | `internal/backend/proxy_manager_*.go` | Backend hotspot now split by responsibility; still emits `log`, `state`, `need-input`, `need-captcha`, `error` |
| Fixed CLI args or launch validation | `internal/backend/launch_options.go` | Source of truth for arg building |
| Persisted GUI settings | `internal/backend/user_settings_store.go` | Enforces fixed defaults when loading and saving |
| Elevated TUN resume | `app.go`, `internal/backend/pending_connect_store.go`, `internal/backend/self_elevation_windows.go` | Whole-app elevation, not child-only elevation |
| Native UI behavior | `ui_iup.go` | Main UI state, queued events, autosave, dialogs, captcha point selection |

## CODE MAP
| Symbol | Location | Role |
|--------|----------|------|
| `main()` | `main.go` | Opens IUP, waits for elevated relaunch cleanup, builds native UI |
| `(*App).startup()` | `app.go` | Resolves app dir, constructs backend stores/managers |
| `(*App).Start()` | `app.go` | Saves settings, handles Windows TUN self-elevation, starts proxy |
| `(*App).ResumePendingConnect()` | `app.go` | Restarts saved connection after elevated relaunch |
| `(*App).ShowWindow()` | `app.go` | Restore/focus path used by tray and second-instance callback |
| `(*ProxyManager).Start()` / `Stop()` | `internal/backend/proxy_manager.go` | Public lifecycle entrypoints |
| `(*ProxyManager).handleLogLine()` | `internal/backend/proxy_manager_logs.go` | Prompt detection, startup detection, EIP readiness trigger |
| `(*iupUI).handleEvent()` | `ui_iup.go` | Routes backend events to the native widgets safely on the UI thread |

## CONVENTIONS
- The GUI wraps a compiled CLI; do not introduce assumptions that require editing upstream `zju-connect`.
- Fixed launch defaults are enforced in more than one place. If a parameter becomes fixed or configurable, update backend arg building, settings persistence, and native UI display together.
- Paths are app-relative at runtime. `client_data.json`, `gui_settings.json`, captcha files, pending resume markers, and temporary elevated-run files live next to the executable/app dir.
- Windows TUN flow is whole-app elevation with pending resume. Do not reintroduce child-only elevation wrappers.
- Close means hide-to-tray unless `Quit()` explicitly enables shutdown.
- The native GUI treats `VPN client started` as the success signal.
- IUP is not thread-safe. Marshal backend-driven UI changes through the queued/timer-driven bridge in `ui_iup.go` instead of touching widgets directly from worker goroutines.

## ANTI-PATTERNS (THIS PROJECT)
- Do not reintroduce Wails/WebView-specific runtime assumptions or build steps.
- Do not force-stop with ad hoc kill logic when the existing graceful stop path is available.
- Do not add tray behavior that depends on tray icon click/double-click support without replacing the current systray layer.
- Do not add dark mode or theme switching unless the user explicitly changes the visual constraint.
- Do not hardcode fixed launch values only in the UI; backend persistence must match.

## UNIQUE STYLES
- Backend platform differences use `*_windows.go` / `*_other.go` file splits instead of large runtime branches.
- Backend orchestration stays in one package, but the old monolithic `proxy_manager.go` has been split into `proxy_manager_*.go`
  files for lifecycle, logs, readiness, polling, and elevated-process handling.
- Logs are a first-class control signal: startup, prompts, captcha flow, and some UX state changes are inferred from emitted log text.
- The new native UI intentionally keeps most desktop behavior in a single Go file (`ui_iup.go`) instead of introducing a large widget tree abstraction.

## COMMANDS
```bash
go run .

go build .

go build -tags gtk4 .

go test ./internal/backend

GOOS=windows GOARCH=amd64 CGO_ENABLED=1 go build -ldflags "-H=windowsgui" -o zju-connect-gui.exe .
```

## NOTES
- Repository size is still small, but backend complexity now clusters around the `internal/backend/proxy_manager_*.go` group and `ui_iup.go`.
- There are currently no UI tests; verification usually means backend tests + native Go build + packaged desktop builds on CI.

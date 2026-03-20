# PROJECT KNOWLEDGE BASE

**Generated:** 2026-03-19
**Commit:** 36f3845
**Branch:** master

## OVERVIEW
`zju-connect-gui` is a Wails desktop wrapper around the `zju-connect` CLI binary. The Go side owns process lifecycle, tray behavior, elevation, and persistence; the Vue side is a thin control surface that reacts to Wails events and exposes only a subset of launch options.

## STRUCTURE
```text
./
├── main.go                  # Wails bootstrap, tray start, single-instance lock
├── app.go                   # Bound app API, window actions, start/stop, resume flow
├── tray.go                  # Systray menu and quit/restore actions
├── tray_icon_*.go           # Platform-specific tray icon bytes
├── internal/backend/        # CLI orchestration, persistence, OS split helpers, tests
├── frontend/                # Vue UI, Vite config, generated Wails bindings
├── build/                   # Wails build output; generated
└── wails.json               # Wails project config
```

## WHERE TO LOOK
| Task | Location | Notes |
|------|----------|-------|
| Wails startup or window lifecycle | `main.go`, `app.go` | `HideWindowOnClose`, single-instance lock, startup/shutdown hooks |
| Tray UX | `tray.go`, `tray_icon_*.go` | Menu actions only; current systray version has no icon double-click callback |
| Start/stop, logs, prompts, captcha | `internal/backend/proxy_manager.go` | Backend hotspot; emits `log`, `state`, `need-input`, `need-captcha`, `error` |
| Fixed CLI args or launch validation | `internal/backend/launch_options.go` | Source of truth for arg building |
| Persisted GUI settings | `internal/backend/user_settings_store.go` | Enforces fixed defaults when loading and saving |
| Elevated TUN resume | `app.go`, `internal/backend/pending_connect_store.go`, `internal/backend/self_elevation_windows.go` | Whole-app elevation, not child-only elevation |
| Frontend behavior | `frontend/src/App.vue` | Main UI state, Wails bindings, status/log/prompt handling |
| Frontend styling | `frontend/src/style.css` | Light theme and `#F2F2F2` background |
| Generated bindings | `frontend/wailsjs/` | Do not edit by hand |

## CODE MAP
| Symbol | Location | Role |
|--------|----------|------|
| `main()` | `main.go` | Starts tray before `wails.Run`, embeds frontend assets |
| `(*App).startup()` | `app.go` | Resolves app dir, constructs backend stores/managers |
| `(*App).Start()` | `app.go` | Saves settings, handles Windows TUN self-elevation, starts proxy |
| `(*App).ResumePendingConnect()` | `app.go` | Restarts saved connection after elevated relaunch |
| `(*App).ShowWindow()` | `app.go` | Restore/focus path used by tray and second-instance callback |
| `(*ProxyManager).Start()` / `Stop()` | `internal/backend/proxy_manager.go` | CLI lifecycle and graceful shutdown |
| `(*ProxyManager).handleLogLine()` | `internal/backend/proxy_manager.go` | Prompt detection, startup detection, EIP auto-open hook |

## CONVENTIONS
- The GUI wraps a compiled CLI; do not introduce assumptions that require editing upstream `zju-connect`.
- Fixed launch defaults are enforced in more than one place. If a parameter becomes fixed or configurable, update backend arg building, settings persistence, and frontend display together.
- Paths are app-relative at runtime. `client_data.json`, `gui_settings.json`, captcha files, pending resume markers, and temporary elevated-run files live next to the executable/app dir.
- Windows TUN flow is whole-app elevation with pending resume. Do not reintroduce child-only elevation wrappers.
- Close means hide-to-tray unless `Quit()` explicitly enables shutdown.
- The frontend treats `VPN client started` as the success signal.

## ANTI-PATTERNS (THIS PROJECT)
- Do not edit `frontend/wailsjs/**` or `build/**`; both are generated.
- Do not force-stop with ad hoc kill logic when the existing graceful stop path is available.
- Do not add tray behavior that depends on tray icon click/double-click support without replacing the current systray layer.
- Do not add dark mode or theme switching unless the user explicitly changes the visual constraint.
- Do not hardcode fixed launch values only in the UI; backend persistence must match.

## UNIQUE STYLES
- Frontend logic is intentionally centralized in one SFC instead of a component tree.
- Backend platform differences use `*_windows.go` / `*_other.go` file splits instead of large runtime branches.
- Logs are a first-class control signal: startup, prompts, captcha flow, and some UX state changes are inferred from emitted log text.

## COMMANDS
```bash
wails dev

cd frontend && npm run build

go test ./internal/backend

wails build -platform windows/amd64 -skipbindings
```

## NOTES
- Repository size is small (~52 files) but two files dominate complexity: `internal/backend/proxy_manager.go` and `frontend/src/App.vue`.
- There are currently no frontend tests; verification usually means backend tests + frontend build + Windows package build.
- `frontend/dist/` is build output and may exist in the repo locally, but it is not a source directory.

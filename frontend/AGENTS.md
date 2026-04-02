# LEGACY FRONTEND GUIDE

## OVERVIEW
`frontend/` is the retired Wails/Vue UI kept only as a migration reference. The shipped desktop UI now lives in `../ui_iup.go`.

## WHERE TO LOOK
| Task | File | Notes |
|------|------|-------|
| Historical UI behavior | `src/App.vue` | Main reference for parity checks while maintaining `../ui_iup.go` |
| App mount | `src/main.ts` | Legacy only |
| Global visuals | `src/style.css` | Legacy styling reference |
| Vite/build settings | `package.json`, `vite.config.ts`, `tailwind.config.js`, `tsconfig*.json` | Retained only for historical context |

## CURRENT SHAPE
- `src/App.vue` is roughly 590 lines and intentionally centralizes the UI.
- The app has two tabs: config and logs.
- Launch option state, autosave, start/stop actions, status text, captcha modal, and manual input modal all live in `App.vue`.
- Log display is capped at 1000 lines.
- The UI treats any log line containing `VPN client started` as successful startup.

## WAILS BRIDGE
- New feature work should target `../ui_iup.go`, not this directory.
- This directory is useful when checking legacy behavior parity such as prompt wording, tab structure, or fixed default launch options.
- Current event set includes `log`, `state`, `need-captcha`, `need-input`, and `error`.
- Prefer matching the existing event-driven pattern instead of introducing parallel frontend-only state machines.

## VISUAL CONSTRAINTS
- Light theme only.
- App background is fixed at `#F2F2F2`.
- Do not add dark mode, theme switching, or unrelated redesign work unless the user explicitly asks.

## FORM AND SETTINGS RULES
- The UI exposes only a subset of launch options.
- Fixed args are displayed in the collapsible list; they are not user-editable.
- The current “仅代理模式” checkbox is the inverse of backend `tunMode`.
- If a launch option changes semantics, sync `App.vue` with backend normalization and persistence code.

## ANTI-PATTERNS
- Do not revive this directory as the active runtime UI unless the user explicitly asks for a web frontend again.
- Do not add new Wails bindings or runtime dependencies here.
- Do not invent frontend test infrastructure unless the user explicitly asks to restore a web frontend.

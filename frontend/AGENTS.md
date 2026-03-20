# FRONTEND GUIDE

## OVERVIEW
`frontend/` is a small Wails/Vue surface where almost all real UI behavior lives in one file: `src/App.vue`.

## WHERE TO LOOK
| Task | File | Notes |
|------|------|-------|
| UI behavior and state | `src/App.vue` | Main hotspot; tabs, forms, logs, prompts, captcha, status, autosave |
| App mount | `src/main.ts` | Only mounts `App`; rarely needs changes |
| Global visuals | `src/style.css` | Light theme, base font stack, `#F2F2F2` background |
| Vite/build settings | `package.json`, `vite.config.ts`, `tailwind.config.js`, `tsconfig*.json` | Minimal frontend toolchain |
| Generated Wails bindings | `wailsjs/` | Generated from Go bindings and runtime; do not edit |
| Build output | `dist/` | Generated bundle; do not edit |

## CURRENT SHAPE
- `src/App.vue` is roughly 590 lines and intentionally centralizes the UI.
- The app has two tabs: config and logs.
- Launch option state, autosave, start/stop actions, status text, captcha modal, and manual input modal all live in `App.vue`.
- Log display is capped at 1000 lines.
- The UI treats any log line containing `VPN client started` as successful startup.

## WAILS BRIDGE
- Go bindings are imported from `wailsjs/go/main/App`.
- Runtime events are imported from `wailsjs/runtime/runtime`.
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
- Do not edit `wailsjs/**` or `dist/**`.
- Do not split `App.vue` into components unless the task clearly justifies a real architecture change.
- Do not add generic browser-web patterns without checking the desktop/Wails context first.
- Do not invent frontend test infrastructure unless the user asks; there are currently no frontend tests.
- Do not override the fixed light visual system with arbitrary colors or theme helpers.

# zju-connect-gui

`zju-connect-gui` is a Wails desktop wrapper around the `zju-connect` CLI. The Go backend manages process lifecycle,
tray behavior, elevation, and persistence, while the Vue frontend provides a thin control surface for the fixed launch
options exposed by the GUI.

## Connection behavior

- Automatic reconnect is **session-scoped inside the running desktop app**, not a system daemon or background service.
- When the `zju-connect` child process exits unexpectedly, the GUI retries automatically with **exponential backoff**.
- A successful connection resets the backoff window after the backend confirms the mode-specific readiness gate.
- Clicking **Stop** cancels any pending retry and suppresses further reconnect attempts until the user starts again.
- If the connection is interrupted while waiting for manual input such as SMS, callback URL, or captcha, the current
  session stops and the user must start a new connection manually.
- Because closing the window hides the app to the tray by default, reconnect attempts may continue while the app remains
  running in the tray.

## Branching and release flow

The repository now follows a three-layer branch model:

- `feature/*`: short-lived feature or fix branches. Daily development starts here, with one branch segment after `feature/`.
- `dev`: integration branch. Feature branches merge into `dev` through pull requests.
- `master`: stable branch. Once `dev` is validated, promote it into `master` through a pull request.

Release tags must be created from `master` after the promotion merge is complete. Tag names should use the `v*`
pattern, such as `v1.0.0`.

## GitHub Actions CI

The repository CI is split across two workflows:

- `.github/workflows/ci.yml`: lightweight validation for `push` to `feature/*` and pull requests targeting `dev`
- `.github/workflows/build-packages.yml`: repository verification for pull requests targeting `master`, plus packaging for pushes to `dev`, pushes to `master`, and manual `workflow_dispatch` runs

Recommended required checks for branch protection should follow the target branch:

- `dev` pull requests: `backend-test`, `frontend-build`
- `dev` branch pushes: packaged artifacts from `.github/workflows/build-packages.yml` for integrated product testing
- `master` pull requests: `Verify repository`
- `master` branch pushes: packaged artifacts from `.github/workflows/build-packages.yml` for release preparation from the merged commit

This split keeps feature-branch feedback fast, produces integrated testable packages from `dev`, lets `master` pull
requests act as a release gate, and generates final release-preparation artifacts only after the merge lands on `master`.

Tags are still created after the `master` promotion merge, but in this minimal setup they do not trigger a separate GitHub Actions
workflow.

## Local development

Run the app in live development mode:

```bash
wails dev
```

Build the frontend bundle:

```bash
mkdir -p frontend/dist && touch frontend/dist/.gitkeep
wails generate module
cd frontend && npm run build
```

Run backend tests:

```bash
go test ./internal/backend
```

Build the Windows desktop package:

```bash
wails build -platform windows/amd64 -skipbindings
```

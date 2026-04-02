# Legacy Build Metadata

This directory contains legacy build metadata from the old Wails-based packaging flow.

The active migration no longer uses `wails build`, WebView assets, or the metadata in this directory during normal
development, CI verification, or package assembly. Current builds are driven directly by `go build` and the workflows
in `.github/workflows/`.

Keep these files only if you intentionally need them as historical reference while finishing the migration. Otherwise,
they are safe candidates for later cleanup once the native packaging story is fully settled.

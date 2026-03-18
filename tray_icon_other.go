//go:build !windows

package main

import _ "embed"

//go:embed frontend/src/assets/images/logo-universal.png
var trayIconBytes []byte

//go:build !windows

package main

import _ "embed"

//go:embed assets/gemini.png
var trayIconBytes []byte

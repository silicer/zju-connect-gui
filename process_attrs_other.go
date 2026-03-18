//go:build !windows

package main

import "os/exec"

func applyProcessAttributes(_ *exec.Cmd) {
}

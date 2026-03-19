//go:build !windows

package backend

import "os/exec"

func applyProcessAttributes(_ *exec.Cmd) {
}

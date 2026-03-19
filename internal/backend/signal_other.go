//go:build !windows

package backend

import (
	"os"
	"os/exec"
)

func signalProcessInterrupt(cmd *exec.Cmd) error {
	if cmd == nil || cmd.Process == nil {
		return nil
	}
	return cmd.Process.Signal(os.Interrupt)
}

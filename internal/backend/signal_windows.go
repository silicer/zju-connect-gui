//go:build windows

package backend

import (
	"os/exec"

	"github.com/containers/winquit/pkg/winquit"
)

func signalProcessInterrupt(cmd *exec.Cmd) error {
	if cmd == nil || cmd.Process == nil {
		return nil
	}
	return winquit.RequestQuit(cmd.Process.Pid)
}

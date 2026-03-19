//go:build windows

package backend

import (
	"os/exec"
	"syscall"

	"golang.org/x/sys/windows"
)

func applyProcessAttributes(cmd *exec.Cmd) {
	cmd.SysProcAttr = &syscall.SysProcAttr{
		HideWindow:    true,
		CreationFlags: windows.CREATE_NO_WINDOW,
	}
}

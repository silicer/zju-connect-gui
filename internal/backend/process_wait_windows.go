//go:build windows

package backend

import (
	"errors"
	"fmt"
	"time"

	"golang.org/x/sys/windows"
)

func WaitForProcessExit(pid int, timeout time.Duration) error {
	if pid <= 0 {
		return nil
	}

	handle, err := windows.OpenProcess(windows.SYNCHRONIZE, false, uint32(pid))
	if err != nil {
		if errors.Is(err, windows.ERROR_INVALID_PARAMETER) {
			return nil
		}
		return fmt.Errorf("failed to open parent process %d: %w", pid, err)
	}
	defer windows.CloseHandle(handle)

	var waitMilliseconds uint32 = windows.INFINITE
	if timeout > 0 {
		waitMilliseconds = uint32(timeout / time.Millisecond)
		if waitMilliseconds == 0 {
			waitMilliseconds = 1
		}
	}

	result, err := windows.WaitForSingleObject(handle, waitMilliseconds)
	if err != nil {
		return fmt.Errorf("failed waiting for parent process %d: %w", pid, err)
	}

	switch result {
	case uint32(windows.WAIT_OBJECT_0):
		return nil
	case uint32(windows.WAIT_TIMEOUT):
		return fmt.Errorf("timed out waiting for parent process %d to exit", pid)
	default:
		return fmt.Errorf("unexpected wait result %d while waiting for parent process %d", result, pid)
	}
}

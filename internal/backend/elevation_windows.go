//go:build windows

package backend

import (
	"errors"
	"fmt"
	"strings"

	"golang.org/x/sys/windows"
)

func launchElevatedPowerShellScript(scriptPath string, cwd string) error {
	verbPtr, err := windows.UTF16PtrFromString("runas")
	if err != nil {
		return fmt.Errorf("failed to encode elevation verb: %w", err)
	}

	filePtr, err := windows.UTF16PtrFromString("powershell.exe")
	if err != nil {
		return fmt.Errorf("failed to encode powershell file path: %w", err)
	}

	args := fmt.Sprintf(
		"-NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File \"%s\"",
		strings.ReplaceAll(scriptPath, "\"", "\"\""),
	)
	argsPtr, err := windows.UTF16PtrFromString(args)
	if err != nil {
		return fmt.Errorf("failed to encode powershell args: %w", err)
	}

	cwdPtr, err := windows.UTF16PtrFromString(cwd)
	if err != nil {
		return fmt.Errorf("failed to encode powershell cwd: %w", err)
	}

	err = windows.ShellExecute(0, verbPtr, filePtr, argsPtr, cwdPtr, windows.SW_HIDE)
	if err != nil {
		if errors.Is(err, windows.ERROR_CANCELLED) || err == windows.ERROR_CANCELLED {
			return errors.New("elevation cancelled by user")
		}
		return fmt.Errorf("failed to request elevation: %w", err)
	}

	return nil
}

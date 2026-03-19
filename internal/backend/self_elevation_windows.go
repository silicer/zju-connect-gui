//go:build windows

package backend

import (
	"errors"
	"fmt"
	"os"
	"syscall"

	"golang.org/x/sys/windows"
)

func IsProcessElevated() (bool, error) {
	var token windows.Token
	if err := windows.OpenProcessToken(windows.CurrentProcess(), windows.TOKEN_QUERY, &token); err != nil {
		return false, fmt.Errorf("failed to open process token: %w", err)
	}
	defer token.Close()

	return token.IsElevated(), nil
}

func RelaunchSelfElevated(cwd string, extraArgs []string) error {
	exePath, err := os.Executable()
	if err != nil {
		return fmt.Errorf("failed to resolve executable path: %w", err)
	}

	verbPtr, err := windows.UTF16PtrFromString("runas")
	if err != nil {
		return fmt.Errorf("failed to encode elevation verb: %w", err)
	}

	filePtr, err := windows.UTF16PtrFromString(exePath)
	if err != nil {
		return fmt.Errorf("failed to encode executable path: %w", err)
	}

	quotedArgs := ""
	if len(extraArgs) > 0 {
		quoted := make([]string, 0, len(extraArgs))
		for _, arg := range extraArgs {
			quoted = append(quoted, syscall.EscapeArg(arg))
		}
		quotedArgs = quoted[0]
		for _, arg := range quoted[1:] {
			quotedArgs += " " + arg
		}
	}

	argsPtr, err := windows.UTF16PtrFromString(quotedArgs)
	if err != nil {
		return fmt.Errorf("failed to encode relaunch arguments: %w", err)
	}

	cwdPtr, err := windows.UTF16PtrFromString(cwd)
	if err != nil {
		return fmt.Errorf("failed to encode working directory: %w", err)
	}

	err = windows.ShellExecute(0, verbPtr, filePtr, argsPtr, cwdPtr, windows.SW_NORMAL)
	if err != nil {
		if errors.Is(err, windows.ERROR_CANCELLED) || err == windows.ERROR_CANCELLED {
			return errors.New("elevation cancelled by user")
		}
		return fmt.Errorf("failed to relaunch application as administrator: %w", err)
	}

	return nil
}

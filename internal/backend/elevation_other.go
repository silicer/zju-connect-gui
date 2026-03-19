//go:build !windows

package backend

import "errors"

func launchElevatedPowerShellScript(_ string, _ string) error {
	return errors.New("elevation is only supported on windows")
}

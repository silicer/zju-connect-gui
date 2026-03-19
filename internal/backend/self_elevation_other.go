//go:build !windows

package backend

import "errors"

func IsProcessElevated() (bool, error) {
	return false, nil
}

func RelaunchSelfElevated(_ string, _ []string) error {
	return errors.New("self elevation is only supported on windows")
}

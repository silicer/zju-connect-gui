//go:build !windows

package backend

import "time"

func WaitForProcessExit(_ int, _ time.Duration) error {
	return nil
}

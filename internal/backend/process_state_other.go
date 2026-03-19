//go:build !windows

package backend

func isWindowsProcessRunning(_ int) (bool, error) {
	return false, nil
}

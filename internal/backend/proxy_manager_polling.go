package backend

import (
	"context"
	"errors"
	"fmt"
	"log"
	"os"
	"strconv"
	"strings"
	"time"
)

const (
	pidPollInterval            = 250 * time.Millisecond
	processStatePollInterval   = 250 * time.Millisecond
	captchaPollInterval        = 500 * time.Millisecond
	captchaMonitorPollInterval = 500 * time.Millisecond
	captchaPollTimeout         = 60 * time.Second
	elevatedStopPollTimeout    = 20 * time.Second
)

func waitPIDFromFile(path string, timeout time.Duration) (int, error) {
	deadline := time.Now().Add(timeout)
	for time.Now().Before(deadline) {
		data, err := os.ReadFile(path)
		if err == nil {
			value := strings.TrimSpace(string(data))
			if value != "" {
				if message, ok := strings.CutPrefix(value, "ERR:"); ok {
					return 0, errors.New(strings.TrimSpace(message))
				}
				pid, parseErr := strconv.Atoi(value)
				if parseErr == nil && pid > 0 {
					return pid, nil
				}
			}
		}
		time.Sleep(pidPollInterval)
	}

	return 0, fmt.Errorf("failed to get elevated process pid from %s", path)
}

func waitProcessRunning(pid int, timeout time.Duration) error {
	deadline := time.Now().Add(timeout)
	for time.Now().Before(deadline) {
		running, err := isWindowsProcessRunning(pid)
		if err == nil && running {
			return nil
		}
		time.Sleep(processStatePollInterval)
	}
	return fmt.Errorf("elevated process did not start correctly (pid=%d)", pid)
}

func waitProcessStopped(pid int, timeout time.Duration) error {
	deadline := time.Now().Add(timeout)
	for time.Now().Before(deadline) {
		running, err := isWindowsProcessRunning(pid)
		if err == nil && !running {
			return nil
		}
		time.Sleep(processStatePollInterval)
	}
	return fmt.Errorf("elevated process did not stop in time (pid=%d)", pid)
}

func (p *ProxyManager) pollCaptcha(path string) {
	defer p.stopCaptchaPoll()

	deadline := time.Now().Add(captchaPollTimeout)
	var lastSize int64
	var stableCount int
	for time.Now().Before(deadline) {
		time.Sleep(captchaPollInterval)
		info, err := os.Stat(path)
		if err != nil {
			continue
		}
		if info.Size() == 0 {
			continue
		}
		if info.Size() == lastSize {
			stableCount++
		} else {
			stableCount = 0
		}
		lastSize = info.Size()
		if stableCount < 1 {
			continue
		}

		if p.emitCaptcha(path) {
			return
		}
	}
	p.setAwaiting("")
}

func (p *ProxyManager) monitorCaptchaFile(ctx context.Context, path string) {
	var lastModUnixNano int64
	var lastSize int64
	for {
		select {
		case <-ctx.Done():
			return
		case <-time.After(captchaMonitorPollInterval):
		}

		info, err := os.Stat(path)
		if err != nil || info.Size() == 0 {
			continue
		}

		modUnixNano := info.ModTime().UnixNano()
		if modUnixNano == lastModUnixNano && info.Size() == lastSize {
			continue
		}

		lastModUnixNano = modUnixNano
		lastSize = info.Size()
		log.Printf("captcha file updated: %s (%d bytes)", path, info.Size())
		p.requestCaptcha()
	}
}

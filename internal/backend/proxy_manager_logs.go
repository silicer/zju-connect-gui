package backend

import (
	"context"
	"encoding/base64"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"strings"
	"time"
)

const logTailPollInterval = 500 * time.Millisecond

func (p *ProxyManager) readStream(_ context.Context, reader io.Reader) {
	buf := make([]byte, 32*1024)
	pending := ""
	for {
		n, err := reader.Read(buf)
		if n > 0 {
			p.consumeLogChunk(buf[:n], &pending, false)
		}
		if err != nil {
			p.consumeLogChunk(nil, &pending, true)
			return
		}
	}
}

func (p *ProxyManager) tailLogFile(ctx context.Context, path string) {
	var offset int64
	var openFailures int
	attached := false
	pending := ""
	draining := false
	for {
		if !draining {
			select {
			case <-ctx.Done():
				draining = true
			case <-time.After(logTailPollInterval):
			}
		}

		file, err := os.Open(path)
		if err != nil {
			if draining {
				p.consumeLogChunk(nil, &pending, true)
				return
			}
			openFailures++
			if openFailures%20 == 0 {
				p.emit("log", fmt.Sprintf("[elevated] waiting for log file %s: %v", filepath.Base(path), err))
			}
			continue
		}
		if !attached {
			p.emit("log", fmt.Sprintf("[elevated] attached log file %s", filepath.Base(path)))
			attached = true
		}
		if openFailures > 0 {
			openFailures = 0
		}

		_, err = file.Seek(offset, io.SeekStart)
		if err != nil {
			_ = file.Close()
			if draining {
				p.consumeLogChunk(nil, &pending, true)
				return
			}
			continue
		}

		data, readErr := io.ReadAll(file)
		_ = file.Close()
		if len(data) > 0 {
			offset += int64(len(data))
			p.consumeLogChunk(data, &pending, false)
		}
		if readErr != nil && !errors.Is(readErr, io.EOF) && !draining {
			continue
		}
		if draining {
			p.consumeLogChunk(nil, &pending, true)
			return
		}
	}
}

func (p *ProxyManager) consumeLogChunk(chunk []byte, pending *string, flush bool) {
	combined := *pending
	if len(chunk) > 0 {
		combined += string(chunk)
	}

	for {
		idx := strings.IndexAny(combined, "\r\n")
		if idx < 0 {
			break
		}

		line := combined[:idx]
		combined = combined[idx+1:]
		for len(combined) > 0 && (combined[0] == '\r' || combined[0] == '\n') {
			combined = combined[1:]
		}
		p.handleLogLine(line)
	}

	trimmedPending := strings.TrimSpace(combined)
	if flush {
		if trimmedPending != "" {
			p.handleLogLine(combined)
		}
		*pending = ""
		return
	}

	*pending = combined
	if trimmedPending != "" {
		p.detectPrompt(trimmedPending)
	}
}

func (p *ProxyManager) handleLogLine(line string) {
	trimmed := strings.TrimSpace(line)
	if trimmed == "" {
		return
	}
	p.emit("log", trimmed)
	p.detectPrompt(trimmed)
	tunMode, httpBind := p.readinessMode()
	if tunMode {
		if isRouteAddedLine(trimmed) {
			p.beginHTTPReadyWait(httpBind)
		}
		return
	}
	if isVPNClientStartedLine(trimmed) {
		p.beginHTTPReadyWait(httpBind)
	}
}

func isVPNClientStartedLine(line string) bool {
	return strings.Contains(strings.TrimSpace(line), "VPN client started")
}

func isRouteAddedLine(line string) bool {
	return strings.Contains(strings.TrimSpace(line), "Add route to ")
}

func (p *ProxyManager) detectPrompt(line string) {
	trimmed := strings.TrimSpace(line)
	if trimmed == "" {
		return
	}

	lower := strings.ToLower(trimmed)
	if strings.Contains(lower, "sms verification code") ||
		strings.Contains(lower, "sms code") ||
		strings.Contains(lower, "sms check code") ||
		strings.Contains(trimmed, "短信验证码") {
		p.requestInput("sms", "Please enter the SMS code")
		return
	}
	if strings.Contains(lower, "callback url") {
		p.requestInput("callback", "Please enter the callback URL")
		return
	}
	if strings.Contains(lower, "graph check code") ||
		strings.Contains(lower, "graph code json") ||
		strings.Contains(lower, "rand code") ||
		(strings.Contains(lower, "captcha") && !strings.Contains(lower, "sms")) ||
		strings.Contains(trimmed, "图形验证码") {
		p.requestCaptcha()
	}
}

func (p *ProxyManager) requestCaptcha() {
	p.ensureAwaiting("captcha")

	p.mu.Lock()
	path := p.captchaPath
	p.mu.Unlock()
	if path == "" {
		path = filepath.Join(p.appDir, "gui_captcha.png")
	}
	if !p.tryStartCaptchaPoll() {
		return
	}
	go p.pollCaptcha(path)
}

func (p *ProxyManager) requestInput(kind, prompt string) {
	if !p.setAwaiting(kind) {
		return
	}
	p.showWindow()
	p.emit("need-input", map[string]any{
		"type":   kind,
		"prompt": prompt,
	})
}

func (p *ProxyManager) emitCaptcha(path string) bool {
	data, err := os.ReadFile(path)
	if err != nil || len(data) == 0 {
		return false
	}
	encoded := base64.StdEncoding.EncodeToString(data)
	p.showWindow()
	p.emit("need-captcha", map[string]any{
		"base64":    encoded,
		"updatedAt": time.Now().UnixMilli(),
	})
	return true
}

func (p *ProxyManager) ensureAwaiting(value string) {
	p.mu.Lock()
	changed := p.awaiting != value
	p.awaiting = value
	p.mu.Unlock()

	if changed {
		p.emitStateWithDetails("awaiting", "", 0, 0)
	}
}

func (p *ProxyManager) tryStartCaptchaPoll() bool {
	p.mu.Lock()
	defer p.mu.Unlock()
	if p.captchaPoll {
		return false
	}
	p.captchaPoll = true
	return true
}

func (p *ProxyManager) stopCaptchaPoll() {
	p.mu.Lock()
	p.captchaPoll = false
	p.mu.Unlock()
}

func (p *ProxyManager) setAwaiting(value string) bool {
	p.mu.Lock()
	if p.awaiting != "" && p.awaiting == value {
		p.mu.Unlock()
		return false
	}
	p.awaiting = value
	p.mu.Unlock()

	p.emitStateWithDetails("awaiting", "", 0, 0)
	return true
}

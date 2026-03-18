package main

import (
	"bufio"
	"context"
	"encoding/base64"
	"errors"
	"fmt"
	"io"
	"log"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strconv"
	"strings"
	"sync"
	"time"

	wailsRuntime "github.com/wailsapp/wails/v2/pkg/runtime"
)

type ProxyManager struct {
	appDir string
	ctx    context.Context

	mu          sync.Mutex
	cmd         *exec.Cmd
	stdin       io.WriteCloser
	waitDone    chan struct{}
	logCancel   context.CancelFunc
	awaiting    string
	captchaPath string
	elevated    bool
	elevatedPID int
}

func NewProxyManager(appDir string) *ProxyManager {
	return &ProxyManager{
		appDir: appDir,
	}
}

func (p *ProxyManager) SetContext(ctx context.Context) {
	p.ctx = ctx
}

func (p *ProxyManager) IsRunning() bool {
	p.mu.Lock()
	defer p.mu.Unlock()
	return p.cmd != nil || p.elevated
}

func (p *ProxyManager) Start(options LaunchOptions) error {
	p.mu.Lock()
	if p.cmd != nil || p.elevated {
		p.mu.Unlock()
		return errors.New("zju-connect is already running")
	}
	p.mu.Unlock()

	options = normalizeLaunchOptions(options)
	if err := options.Validate(); err != nil {
		return err
	}

	captchaPath := filepath.Join(p.appDir, "gui_captcha.png")

	p.mu.Lock()
	p.captchaPath = captchaPath
	p.mu.Unlock()

	_ = os.Remove(captchaPath)

	if runtime.GOOS == "windows" && options.TunMode {
		return p.startElevated(captchaPath, options)
	}
	return p.startNormal(captchaPath, options)
}

func (p *ProxyManager) Stop() error {
	p.mu.Lock()
	cmd := p.cmd
	waitDone := p.waitDone
	logCancel := p.logCancel
	elevated := p.elevated
	pid := p.elevatedPID
	p.mu.Unlock()

	if logCancel != nil {
		logCancel()
	}

	if elevated {
		return p.stopElevated(pid)
	}

	if cmd == nil {
		return nil
	}

	_ = cmd.Process.Signal(os.Interrupt)
	select {
	case <-waitDone:
		return nil
	case <-time.After(5 * time.Second):
		_ = cmd.Process.Kill()
		select {
		case <-waitDone:
			return nil
		case <-time.After(5 * time.Second):
			return errors.New("timeout waiting for process to stop")
		}
	}
}

func (p *ProxyManager) SubmitInput(value string) error {
	value = strings.TrimSpace(value)
	if value == "" {
		return errors.New("input cannot be empty")
	}

	p.mu.Lock()
	stdin := p.stdin
	awaiting := p.awaiting
	elevated := p.elevated
	p.mu.Unlock()

	if elevated {
		return errors.New("cannot submit input to elevated process")
	}
	if stdin == nil {
		return errors.New("process is not running")
	}
	if _, err := io.WriteString(stdin, value+"\n"); err != nil {
		return err
	}
	if awaiting != "" {
		p.setAwaiting("")
	}
	return nil
}

func (p *ProxyManager) startNormal(captchaPath string, options LaunchOptions) error {
	binPath := p.binaryPath()
	if _, err := os.Stat(binPath); err != nil {
		return fmt.Errorf("zju-connect binary not found: %s", binPath)
	}

	cmd := exec.Command(binPath, options.BuildArgs(captchaPath)...)
	cmd.Dir = p.appDir
	applyProcessAttributes(cmd)

	stdin, err := cmd.StdinPipe()
	if err != nil {
		return err
	}
	stdout, err := cmd.StdoutPipe()
	if err != nil {
		return err
	}
	stderr, err := cmd.StderrPipe()
	if err != nil {
		return err
	}

	if err := cmd.Start(); err != nil {
		return err
	}

	logCtx, logCancel := context.WithCancel(context.Background())
	waitDone := make(chan struct{})

	p.mu.Lock()
	p.cmd = cmd
	p.stdin = stdin
	p.waitDone = waitDone
	p.logCancel = logCancel
	p.awaiting = ""
	p.mu.Unlock()

	p.emitState("running")

	go p.readStream(logCtx, stdout)
	go p.readStream(logCtx, stderr)
	go p.monitorCaptchaFile(logCtx, captchaPath)
	go func() {
		_ = cmd.Wait()
		logCancel()
		p.mu.Lock()
		p.cmd = nil
		p.stdin = nil
		p.awaiting = ""
		p.mu.Unlock()
		p.emitState("stopped")
		close(waitDone)
	}()

	return nil
}

func (p *ProxyManager) startElevated(captchaPath string, options LaunchOptions) error {
	binPath := p.binaryPath()
	if _, err := os.Stat(binPath); err != nil {
		return fmt.Errorf("zju-connect binary not found: %s", binPath)
	}

	logPath := filepath.Join(p.appDir, "zju-connect.log")
	args := options.BuildArgs(captchaPath)
	argsString := strings.Join(escapeArgs(args), " ")
	command := fmt.Sprintf(
		"Start-Process -Verb RunAs -WindowStyle Hidden -FilePath '%s' -ArgumentList '%s' -RedirectStandardOutput '%s' -RedirectStandardError '%s' -PassThru | Select-Object -ExpandProperty Id",
		escapePowerShell(binPath),
		escapePowerShell(argsString),
		escapePowerShell(logPath),
		escapePowerShell(logPath),
	)

	output, err := exec.Command("powershell", "-NoProfile", "-NonInteractive", "-Command", command).Output()
	if err != nil {
		return fmt.Errorf("failed to start elevated process: %w", err)
	}

	pidString := strings.TrimSpace(string(output))
	pid, err := strconv.Atoi(pidString)
	if err != nil {
		return fmt.Errorf("failed to parse elevated pid: %s", pidString)
	}

	logCtx, logCancel := context.WithCancel(context.Background())

	p.mu.Lock()
	p.elevated = true
	p.elevatedPID = pid
	p.logCancel = logCancel
	p.awaiting = ""
	p.mu.Unlock()

	p.emitState("running")
	go p.tailLogFile(logCtx, logPath)
	go p.monitorCaptchaFile(logCtx, captchaPath)

	return nil
}

func (p *ProxyManager) stopElevated(pid int) error {
	if pid == 0 {
		p.mu.Lock()
		p.elevated = false
		p.mu.Unlock()
		return nil
	}
	_ = exec.Command("taskkill", "/PID", strconv.Itoa(pid), "/T").Run()
	p.mu.Lock()
	p.elevated = false
	p.elevatedPID = 0
	p.awaiting = ""
	p.mu.Unlock()
	p.emitState("stopped")
	return nil
}

func (p *ProxyManager) readStream(ctx context.Context, reader io.Reader) {
	scanner := bufio.NewScanner(reader)
	buf := make([]byte, 0, 64*1024)
	scanner.Buffer(buf, 1024*1024)
	for scanner.Scan() {
		select {
		case <-ctx.Done():
			return
		default:
		}
		line := scanner.Text()
		p.handleLogLine(line)
	}
}

func (p *ProxyManager) tailLogFile(ctx context.Context, path string) {
	var offset int64
	for {
		select {
		case <-ctx.Done():
			return
		case <-time.After(500 * time.Millisecond):
		}

		file, err := os.Open(path)
		if err != nil {
			continue
		}

		_, err = file.Seek(offset, io.SeekStart)
		if err != nil {
			_ = file.Close()
			continue
		}

		reader := bufio.NewReader(file)
		for {
			line, err := reader.ReadString('\n')
			if len(line) > 0 {
				offset += int64(len(line))
				p.handleLogLine(strings.TrimRight(line, "\r\n"))
			}
			if err != nil {
				if errors.Is(err, io.EOF) {
					break
				}
				break
			}
		}
		_ = file.Close()
	}
}

func (p *ProxyManager) handleLogLine(line string) {
	trimmed := strings.TrimSpace(line)
	if trimmed == "" {
		return
	}
	p.emit("log", trimmed)

	lower := strings.ToLower(trimmed)
	if strings.Contains(lower, "graph check code") ||
		strings.Contains(lower, "captcha") ||
		strings.Contains(lower, "check code") ||
		strings.Contains(trimmed, "验证码") {
		p.requestCaptcha()
		return
	}
	if strings.Contains(lower, "callback url") {
		p.requestInput("callback", "Please enter the callback URL")
		return
	}
	if strings.Contains(lower, "sms code") || strings.Contains(lower, "sms check code") {
		p.requestInput("sms", "Please enter the SMS code")
	}
}

func (p *ProxyManager) requestCaptcha() {
	if !p.setAwaiting("captcha") {
		return
	}

	p.mu.Lock()
	path := p.captchaPath
	p.mu.Unlock()
	if path == "" {
		path = filepath.Join(p.appDir, "gui_captcha.png")
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

func (p *ProxyManager) pollCaptcha(path string) {
	deadline := time.Now().Add(60 * time.Second)
	var lastSize int64
	var stableCount int
	for time.Now().Before(deadline) {
		time.Sleep(500 * time.Millisecond)
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

		data, err := os.ReadFile(path)
		if err != nil {
			continue
		}
		encoded := base64.StdEncoding.EncodeToString(data)
		p.showWindow()
		p.emit("need-captcha", map[string]any{
			"base64": encoded,
		})
		return
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
		case <-time.After(500 * time.Millisecond):
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

func (p *ProxyManager) setAwaiting(value string) bool {
	p.mu.Lock()
	defer p.mu.Unlock()
	if p.awaiting != "" && p.awaiting == value {
		return false
	}
	p.awaiting = value
	p.emitState("awaiting")
	return true
}

func (p *ProxyManager) emitState(state string) {
	p.mu.Lock()
	awaiting := p.awaiting
	running := p.cmd != nil || p.elevated
	p.mu.Unlock()
	status := map[string]any{
		"state":    state,
		"awaiting": awaiting,
		"running":  running,
	}
	p.emit("state", status)
}

func (p *ProxyManager) emit(event string, payload any) {
	if p.ctx == nil {
		return
	}
	wailsRuntime.EventsEmit(p.ctx, event, payload)
}

func (p *ProxyManager) showWindow() {
	if p.ctx == nil {
		return
	}
	wailsRuntime.WindowShow(p.ctx)
	wailsRuntime.WindowUnminimise(p.ctx)
	wailsRuntime.WindowSetAlwaysOnTop(p.ctx, true)
	wailsRuntime.WindowSetAlwaysOnTop(p.ctx, false)
}

func (p *ProxyManager) binaryPath() string {
	executable := "zju-connect"
	if runtime.GOOS == "windows" {
		executable = "zju-connect.exe"
	}
	return filepath.Join(p.appDir, "bin", executable)
}

func escapeArgs(args []string) []string {
	result := make([]string, 0, len(args))
	for _, arg := range args {
		if strings.ContainsAny(arg, " \t\n\"") {
			result = append(result, strconv.Quote(arg))
		} else {
			result = append(result, arg)
		}
	}
	return result
}

func escapePowerShell(value string) string {
	return strings.ReplaceAll(value, "'", "''")
}

package backend

import (
	"context"
	"errors"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"sync"
	"time"
)

type retryTimer interface {
	Stop() bool
}

type ProxyManager struct {
	appDir string
	ui     UIBridge

	mu               sync.Mutex
	cmd              *exec.Cmd
	stdin            io.WriteCloser
	waitDone         chan struct{}
	logCancel        context.CancelFunc
	awaiting         string
	captchaPoll      bool
	captchaPath      string
	delayedEIPTimer  retryTimer
	eipOpened        bool
	eipOptions       LaunchOptions
	elevated         bool
	elevatedPID      int
	ready            bool
	readyWaitGen     uint64
	sessionActive    bool
	lastOptions      LaunchOptions
	retryAttempt     int
	retryTimer       retryTimer
	retryGeneration  uint64
	retryBaseDelay   time.Duration
	retryMaxDelay    time.Duration
	afterFunc        func(time.Duration, func()) retryTimer
	autoOpenDelay    func() time.Duration
	retryJitter      func(time.Duration, int) time.Duration
	startProcess     func(string, LaunchOptions) error
	waitForHTTPReady func(string, uint64)
	openEIP          func(LaunchOptions) error
}

func NewProxyManager(appDir string) *ProxyManager {
	return &ProxyManager{
		appDir:         appDir,
		retryBaseDelay: time.Second,
		retryMaxDelay:  time.Minute,
		afterFunc: func(delay time.Duration, fn func()) retryTimer {
			return time.AfterFunc(delay, fn)
		},
		autoOpenDelay: defaultEIPAutoOpenDelay,
		retryJitter:   defaultRetryJitter,
	}
}

func (p *ProxyManager) SetUI(ui UIBridge) {
	p.ui = ui
}

func (p *ProxyManager) IsRunning() bool {
	p.mu.Lock()
	defer p.mu.Unlock()
	return p.sessionActive
}

func (p *ProxyManager) Start(options LaunchOptions) error {
	options = normalizeLaunchOptions(options)
	if !filepath.IsAbs(options.ClientDataFile) {
		options.ClientDataFile = filepath.Join(p.appDir, options.ClientDataFile)
	}
	if err := options.Validate(); err != nil {
		return err
	}

	p.mu.Lock()
	if p.cmd != nil || p.elevated {
		p.mu.Unlock()
		return errors.New("zju-connect is already running")
	}
	p.retryGeneration++
	p.stopRetryTimerLocked()
	p.stopDelayedEIPTimerLocked()
	p.mu.Unlock()

	captchaPath := filepath.Join(p.appDir, "gui_captcha.png")

	p.mu.Lock()
	p.captchaPath = captchaPath
	p.eipOptions = options
	p.eipOpened = false
	p.lastOptions = options
	p.ready = false
	p.readyWaitGen = 0
	p.sessionActive = true
	p.retryAttempt = 0
	p.awaiting = ""
	p.captchaPoll = false
	p.mu.Unlock()

	_ = os.Remove(captchaPath)

	if runtime.GOOS == "windows" && options.TunMode {
		elevated, err := IsProcessElevated()
		if err != nil {
			p.mu.Lock()
			p.sessionActive = false
			p.awaiting = ""
			p.captchaPoll = false
			p.mu.Unlock()
			return err
		}
		if !elevated {
			p.mu.Lock()
			p.sessionActive = false
			p.awaiting = ""
			p.captchaPoll = false
			p.mu.Unlock()
			return errors.New("tun mode requires elevated application restart")
		}
	}
	if err := p.startManaged(captchaPath, options); err != nil {
		p.mu.Lock()
		p.sessionActive = false
		p.awaiting = ""
		p.captchaPoll = false
		p.mu.Unlock()
		p.emitStateWithDetails("stopped", "", 0, 0)
		return err
	}
	return nil
}

func (p *ProxyManager) Stop() error {
	p.mu.Lock()
	cmd := p.cmd
	waitDone := p.waitDone
	elevated := p.elevated
	pid := p.elevatedPID
	p.retryGeneration++
	p.stopRetryTimerLocked()
	p.stopDelayedEIPTimerLocked()
	p.sessionActive = false
	p.retryAttempt = 0
	p.ready = false
	p.readyWaitGen = 0
	p.awaiting = ""
	p.captchaPoll = false
	p.mu.Unlock()

	if elevated {
		return p.stopElevated(pid)
	}

	if cmd == nil {
		p.emitStateWithDetails("stopped", "", 0, 0)
		return nil
	}

	if err := signalProcessInterrupt(cmd); err != nil {
		p.emit("log", fmt.Sprintf("[stop] graceful signal failed, falling back to default interrupt: %v", err))
		_ = cmd.Process.Signal(os.Interrupt)
	} else if runtime.GOOS == "windows" {
		p.emit("log", fmt.Sprintf("[stop] graceful quit requested for pid=%d", cmd.Process.Pid))
	}
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
		inputPath := filepath.Join(p.appDir, "zju-connect.input")
		file, err := os.OpenFile(inputPath, os.O_CREATE|os.O_WRONLY|os.O_APPEND, 0o600)
		if err != nil {
			return fmt.Errorf("failed to open elevated input channel: %w", err)
		}
		defer file.Close()

		if _, err := io.WriteString(file, value+"\n"); err != nil {
			return fmt.Errorf("failed to submit input to elevated process: %w", err)
		}
		if awaiting != "" {
			p.setAwaiting("")
		}
		return nil
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

func (p *ProxyManager) emitState(state string) {
	p.emitStateWithDetails(state, "", 0, 0)
}

func (p *ProxyManager) emitStateWithDetails(state string, message string, retryAttempt int, retryDelay time.Duration) {
	p.mu.Lock()
	awaiting := p.awaiting
	running := p.sessionActive
	p.mu.Unlock()
	status := map[string]any{
		"state":    state,
		"awaiting": awaiting,
		"running":  running,
	}
	if message != "" {
		status["message"] = message
	}
	if retryAttempt > 0 {
		status["retryAttempt"] = retryAttempt
	}
	if retryDelay > 0 {
		status["retryDelayMs"] = retryDelay.Milliseconds()
	}
	p.emit("state", status)
}

func (p *ProxyManager) emit(event string, payload any) {
	if p.ui == nil {
		return
	}
	p.ui.EmitEvent(event, payload)
}

func (p *ProxyManager) showWindow() {
	if p.ui == nil {
		return
	}
	p.ui.ShowWindow()
}

func (p *ProxyManager) binaryPath() string {
	executable := "zju-connect"
	if runtime.GOOS == "windows" {
		executable = "zju-connect.exe"
	}
	return filepath.Join(p.appDir, "bin", executable)
}

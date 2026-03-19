package backend

import (
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
	captchaPoll bool
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
	if !filepath.IsAbs(options.ClientDataFile) {
		options.ClientDataFile = filepath.Join(p.appDir, options.ClientDataFile)
	}
	if err := options.Validate(); err != nil {
		return err
	}

	captchaPath := filepath.Join(p.appDir, "gui_captcha.png")

	p.mu.Lock()
	p.captchaPath = captchaPath
	p.mu.Unlock()

	_ = os.Remove(captchaPath)

	if runtime.GOOS == "windows" && options.TunMode {
		elevated, err := IsProcessElevated()
		if err != nil {
			return err
		}
		if !elevated {
			return errors.New("tun mode requires elevated application restart")
		}
	}
	return p.startNormal(captchaPath, options)
}

func (p *ProxyManager) Stop() error {
	p.mu.Lock()
	cmd := p.cmd
	waitDone := p.waitDone
	elevated := p.elevated
	pid := p.elevatedPID
	p.mu.Unlock()

	if elevated {
		return p.stopElevated(pid)
	}

	if cmd == nil {
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
	p.captchaPoll = false
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
		p.captchaPoll = false
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

	runID := time.Now().UnixMilli()
	logPath := filepath.Join(p.appDir, fmt.Sprintf("zju-connect-%d.log", runID))
	errLogPath := filepath.Join(p.appDir, fmt.Sprintf("zju-connect-%d.err.log", runID))
	supervisorLogPath := filepath.Join(p.appDir, fmt.Sprintf("zju-connect-%d.supervisor.log", runID))
	pidPath := filepath.Join(p.appDir, "zju-connect.pid")
	inputPath := filepath.Join(p.appDir, "zju-connect.input")
	stopPath := filepath.Join(p.appDir, "zju-connect.stop")
	scriptPath := filepath.Join(p.appDir, "start-elevated.ps1")
	args := options.BuildArgs(captchaPath)

	_ = os.Remove(pidPath)
	_ = os.Remove(inputPath)
	_ = os.Remove(stopPath)
	script := buildElevatedLaunchScript(binPath, args, p.appDir, logPath, errLogPath, supervisorLogPath, pidPath, inputPath, stopPath)
	if err := os.WriteFile(scriptPath, []byte(script), 0o600); err != nil {
		return fmt.Errorf("failed to write elevated launch script: %w", err)
	}
	defer func() {
		_ = os.Remove(scriptPath)
	}()

	if err := launchElevatedPowerShellScript(scriptPath, p.appDir); err != nil {
		return err
	}

	pid, err := waitPIDFromFile(pidPath, 45*time.Second)
	if err != nil {
		return err
	}
	if err := waitProcessRunning(pid, 5*time.Second); err != nil {
		return err
	}

	logCtx, logCancel := context.WithCancel(context.Background())

	p.mu.Lock()
	p.elevated = true
	p.elevatedPID = pid
	p.logCancel = logCancel
	p.awaiting = ""
	p.captchaPoll = false
	p.mu.Unlock()

	p.emitState("running")
	go p.tailLogFile(logCtx, logPath)
	go p.tailLogFile(logCtx, errLogPath)
	go p.tailLogFile(logCtx, supervisorLogPath)
	go p.monitorCaptchaFile(logCtx, captchaPath)
	p.emit("log", fmt.Sprintf("[elevated] process started (pid=%d)", pid))
	p.emit("log", fmt.Sprintf("[elevated] tailing logs: %s, %s, %s", filepath.Base(logPath), filepath.Base(errLogPath), filepath.Base(supervisorLogPath)))

	return nil
}

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
		time.Sleep(250 * time.Millisecond)
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
		time.Sleep(250 * time.Millisecond)
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
		time.Sleep(250 * time.Millisecond)
	}
	return fmt.Errorf("elevated process did not stop in time (pid=%d)", pid)
}

func (p *ProxyManager) stopElevated(pid int) error {
	if pid == 0 {
		p.mu.Lock()
		p.elevated = false
		logCancel := p.logCancel
		p.logCancel = nil
		p.mu.Unlock()
		if logCancel != nil {
			logCancel()
		}
		return nil
	}

	stopPath := filepath.Join(p.appDir, "zju-connect.stop")
	if err := os.WriteFile(stopPath, []byte("stop\n"), 0o600); err != nil {
		return fmt.Errorf("failed to request elevated graceful stop: %w", err)
	}

	if err := waitProcessStopped(pid, 12*time.Second); err != nil {
		fallbackErr := p.stopElevatedWithUAC(pid)
		if fallbackErr != nil {
			return fmt.Errorf("failed to stop elevated process %d gracefully: %v; fallback failed: %w", pid, err, fallbackErr)
		}
		if waitErr := waitProcessStopped(pid, 10*time.Second); waitErr != nil {
			return waitErr
		}
	}

	p.mu.Lock()
	logCancel := p.logCancel
	p.logCancel = nil
	p.elevated = false
	p.elevatedPID = 0
	p.awaiting = ""
	p.captchaPoll = false
	p.mu.Unlock()
	if logCancel != nil {
		logCancel()
	}
	_ = os.Remove(filepath.Join(p.appDir, "zju-connect.pid"))
	_ = os.Remove(filepath.Join(p.appDir, "zju-connect.input"))
	_ = os.Remove(filepath.Join(p.appDir, "zju-connect.stop"))
	p.emit("log", fmt.Sprintf("[elevated] process stopped (pid=%d)", pid))
	p.emitState("stopped")
	return nil
}

func (p *ProxyManager) stopElevatedWithUAC(pid int) error {
	if runtime.GOOS != "windows" {
		return errors.New("elevated stop fallback is only supported on windows")
	}

	resultPath := filepath.Join(p.appDir, "stop-elevated.result")
	scriptPath := filepath.Join(p.appDir, "stop-elevated.ps1")
	_ = os.Remove(resultPath)

	script := strings.Join([]string{
		"$ErrorActionPreference = 'Stop'",
		"try {",
		fmt.Sprintf("  Stop-Process -Id %d -Force -ErrorAction Stop", pid),
		fmt.Sprintf("  Set-Content -Path '%s' -Value 'OK' -Encoding ascii", escapePowerShell(resultPath)),
		"} catch {",
		fmt.Sprintf("  Set-Content -Path '%s' -Value ('ERR:' + $_.Exception.Message) -Encoding utf8", escapePowerShell(resultPath)),
		"  exit 1",
		"}",
	}, "\n")

	if err := os.WriteFile(scriptPath, []byte(script), 0o600); err != nil {
		return fmt.Errorf("failed to write elevated stop script: %w", err)
	}
	defer func() {
		_ = os.Remove(scriptPath)
		_ = os.Remove(resultPath)
	}()

	if err := launchElevatedPowerShellScript(scriptPath, p.appDir); err != nil {
		return err
	}

	deadline := time.Now().Add(20 * time.Second)
	for time.Now().Before(deadline) {
		data, readErr := os.ReadFile(resultPath)
		if readErr == nil {
			value := strings.TrimSpace(string(data))
			if value == "OK" {
				return nil
			}
			if message, ok := strings.CutPrefix(value, "ERR:"); ok {
				return errors.New(strings.TrimSpace(message))
			}
		}
		time.Sleep(250 * time.Millisecond)
	}

	return errors.New("timed out waiting for elevated stop confirmation")
}

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
			case <-time.After(500 * time.Millisecond):
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

func (p *ProxyManager) pollCaptcha(path string) {
	defer p.stopCaptchaPoll()

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
		p.emitState("awaiting")
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

func buildPowerShellArgumentList(args []string) string {
	quoted := make([]string, 0, len(args))
	for _, arg := range args {
		quoted = append(quoted, fmt.Sprintf("'%s'", escapePowerShell(arg)))
	}
	return strings.Join(quoted, ", ")
}

func buildElevatedLaunchScript(binPath string, args []string, appDir string, logPath string, errLogPath string, supervisorLogPath string, pidPath string, inputPath string, stopPath string) string {
	commandLine := escapePowerShell(buildWindowsCommandLine(args))
	return fmt.Sprintf(`$ErrorActionPreference = 'Stop'
try {
  Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class ConsoleSignal {
  [DllImport("kernel32.dll", SetLastError=true)] public static extern bool FreeConsole();
  [DllImport("kernel32.dll", SetLastError=true)] public static extern bool AttachConsole(uint dwProcessId);
  [DllImport("kernel32.dll", SetLastError=true)] public static extern bool SetConsoleCtrlHandler(IntPtr handler, bool add);
  [DllImport("kernel32.dll", SetLastError=true)] public static extern bool GenerateConsoleCtrlEvent(uint ctrlEvent, uint processGroupId);
}
'@ | Out-Null

  $psi = New-Object System.Diagnostics.ProcessStartInfo
  $psi.FileName = '%s'
  $psi.Arguments = '%s'
  $psi.WorkingDirectory = '%s'
  $psi.UseShellExecute = $false
  $psi.CreateNoWindow = $true
  $psi.RedirectStandardInput = $true
  $psi.RedirectStandardOutput = $true
  $psi.RedirectStandardError = $true
  $psi.StandardOutputEncoding = [System.Text.Encoding]::UTF8
  $psi.StandardErrorEncoding = [System.Text.Encoding]::UTF8

  $process = New-Object System.Diagnostics.Process
  $process.StartInfo = $psi

  $stdoutFs = New-Object System.IO.FileStream('%s', [System.IO.FileMode]::Create, [System.IO.FileAccess]::Write, [System.IO.FileShare]::ReadWrite)
  $stderrFs = New-Object System.IO.FileStream('%s', [System.IO.FileMode]::Create, [System.IO.FileAccess]::Write, [System.IO.FileShare]::ReadWrite)
  $supervisorFs = New-Object System.IO.FileStream('%s', [System.IO.FileMode]::Create, [System.IO.FileAccess]::Write, [System.IO.FileShare]::ReadWrite)
  $supervisorWriter = New-Object System.IO.StreamWriter($supervisorFs, [System.Text.Encoding]::UTF8)
  $supervisorWriter.AutoFlush = $true

  if (-not (Test-Path -LiteralPath '%s')) {
    New-Item -ItemType File -Path '%s' -Force | Out-Null
  }
  if (Test-Path -LiteralPath '%s') {
    Remove-Item -LiteralPath '%s' -Force
  }

  $inputOffset = 0
  $stopRequested = $false

  $process.Start() | Out-Null
  $stdoutTask = $process.StandardOutput.BaseStream.CopyToAsync($stdoutFs)
  $stderrTask = $process.StandardError.BaseStream.CopyToAsync($stderrFs)
  $supervisorWriter.WriteLine('[elevated] stdout/stderr raw stream copy attached')

  Set-Content -Path '%s' -Value $process.Id -Encoding ascii

  while (-not $process.HasExited) {
    if ((-not $stopRequested) -and (Test-Path -LiteralPath '%s')) {
      $stopRequested = $true
      $supervisorWriter.WriteLine('[stop] stop request detected')
      try {
        [ConsoleSignal]::FreeConsole() | Out-Null
        if ([ConsoleSignal]::AttachConsole([uint32]$process.Id)) {
          [ConsoleSignal]::SetConsoleCtrlHandler([IntPtr]::Zero, $true) | Out-Null
          if ([ConsoleSignal]::GenerateConsoleCtrlEvent(1, 0)) {
            $supervisorWriter.WriteLine('[stop] sent CTRL_BREAK_EVENT')
          } else {
            $supervisorWriter.WriteLine('[stop] GenerateConsoleCtrlEvent failed')
          }
          Start-Sleep -Milliseconds 200
          [ConsoleSignal]::FreeConsole() | Out-Null
          [ConsoleSignal]::SetConsoleCtrlHandler([IntPtr]::Zero, $false) | Out-Null
        } else {
          $supervisorWriter.WriteLine('[stop] AttachConsole failed; fallback to stdin close')
        }
      } catch {
        $supervisorWriter.WriteLine('[stop] ctrl-break error: ' + $_.Exception.Message)
      }

      try {
        $process.StandardInput.Close()
      } catch {}

      $graceDeadline = [DateTime]::UtcNow.AddSeconds(10)
      while ((-not $process.HasExited) -and ([DateTime]::UtcNow -lt $graceDeadline)) {
        Start-Sleep -Milliseconds 150
      }

      if (-not $process.HasExited) {
        $supervisorWriter.WriteLine('[stop] graceful stop timeout, force stopping process')
        try {
          Stop-Process -Id $process.Id -Force -ErrorAction Stop
        } catch {
          $supervisorWriter.WriteLine('[stop] force stop error: ' + $_.Exception.Message)
        }
      }
      continue
    }

    if (Test-Path -LiteralPath '%s') {
      $inputInfo = Get-Item -LiteralPath '%s'
      if ($inputInfo.Length -gt $inputOffset) {
        $fs = [System.IO.File]::Open('%s', [System.IO.FileMode]::Open, [System.IO.FileAccess]::Read, [System.IO.FileShare]::ReadWrite)
        try {
          $null = $fs.Seek($inputOffset, [System.IO.SeekOrigin]::Begin)
          $sr = New-Object System.IO.StreamReader($fs, [System.Text.Encoding]::UTF8, $true, 4096, $true)
          try {
            while (($line = $sr.ReadLine()) -ne $null) {
              $process.StandardInput.WriteLine($line)
              $process.StandardInput.Flush()
            }
            $inputOffset = $fs.Position
          } finally {
            $sr.Close()
          }
        } finally {
          $fs.Close()
        }
      }
    }
    Start-Sleep -Milliseconds 150
  }

  $process.WaitForExit()
  $stdoutTask.Wait(1000) | Out-Null
  $stderrTask.Wait(1000) | Out-Null
  $supervisorWriter.WriteLine('[elevated] child process exited with code ' + $process.ExitCode)
  $supervisorWriter.Close()
  $stdoutFs.Close()
  $stderrFs.Close()
  $supervisorFs.Close()
} catch {
  Set-Content -Path '%s' -Value ('ERR:' + $_.Exception.Message) -Encoding utf8
  exit 1
}
`,
		escapePowerShell(binPath),
		commandLine,
		escapePowerShell(appDir),
		escapePowerShell(logPath),
		escapePowerShell(errLogPath),
		escapePowerShell(supervisorLogPath),
		escapePowerShell(inputPath),
		escapePowerShell(inputPath),
		escapePowerShell(stopPath),
		escapePowerShell(stopPath),
		escapePowerShell(pidPath),
		escapePowerShell(stopPath),
		escapePowerShell(inputPath),
		escapePowerShell(inputPath),
		escapePowerShell(inputPath),
		escapePowerShell(pidPath),
	)
}

func buildWindowsCommandLine(args []string) string {
	quoted := make([]string, 0, len(args))
	for _, arg := range args {
		quoted = append(quoted, quoteWindowsArg(arg))
	}
	return strings.Join(quoted, " ")
}

func quoteWindowsArg(value string) string {
	if value == "" {
		return `""`
	}

	needsQuotes := false
	for _, ch := range value {
		if ch == ' ' || ch == '\t' || ch == '"' {
			needsQuotes = true
			break
		}
	}
	if !needsQuotes {
		return value
	}

	var b strings.Builder
	b.WriteByte('"')
	backslashes := 0
	for _, ch := range value {
		if ch == '\\' {
			backslashes++
			continue
		}

		if ch == '"' {
			b.WriteString(strings.Repeat("\\", backslashes*2+1))
			b.WriteByte('"')
			backslashes = 0
			continue
		}

		if backslashes > 0 {
			b.WriteString(strings.Repeat("\\", backslashes))
			backslashes = 0
		}
		b.WriteRune(ch)
	}

	if backslashes > 0 {
		b.WriteString(strings.Repeat("\\", backslashes*2))
	}
	b.WriteByte('"')
	return b.String()
}

func escapePowerShell(value string) string {
	return strings.ReplaceAll(value, "'", "''")
}

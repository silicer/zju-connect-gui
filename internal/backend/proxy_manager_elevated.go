package backend

import (
	"context"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"runtime"
	"strings"
	"time"
)

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
	p.eipOpened = false
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

func (p *ProxyManager) stopElevated(pid int) error {
	if pid == 0 {
		p.mu.Lock()
		p.elevated = false
		logCancel := p.logCancel
		p.logCancel = nil
		p.eipOpened = false
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
	p.eipOpened = false
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

	deadline := time.Now().Add(elevatedStopPollTimeout)
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
		time.Sleep(pidPollInterval)
	}

	return errors.New("timed out waiting for elevated stop confirmation")
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

package backend

import (
	"context"
	"fmt"
	"math/rand"
	"os"
	"os/exec"
	"sync"
	"time"
)

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
	p.mu.Unlock()

	p.emitStateWithDetails("running", "", 0, 0)

	var streamWG sync.WaitGroup
	streamWG.Add(2)
	go func() {
		defer streamWG.Done()
		p.readStream(logCtx, stdout)
	}()
	go func() {
		defer streamWG.Done()
		p.readStream(logCtx, stderr)
	}()
	go p.monitorCaptchaFile(logCtx, captchaPath)
	go func() {
		waitErr := cmd.Wait()
		logCancel()
		streamWG.Wait()
		p.mu.Lock()
		p.cmd = nil
		p.stdin = nil
		p.captchaPoll = false
		p.logCancel = nil
		p.mu.Unlock()
		close(waitDone)
		p.handleProcessExit(waitErr)
	}()

	return nil
}

func (p *ProxyManager) startManaged(captchaPath string, options LaunchOptions) error {
	if p.startProcess != nil {
		return p.startProcess(captchaPath, options)
	}
	return p.startNormal(captchaPath, options)
}

func (p *ProxyManager) handleProcessExit(waitErr error) {
	p.mu.Lock()
	if !p.sessionActive {
		p.ready = false
		p.readyWaitGen = 0
		p.retryAttempt = 0
		p.mu.Unlock()
		p.emitStateWithDetails("stopped", "", 0, 0)
		return
	}
	if p.awaiting != "" {
		blockedReason := p.awaiting
		p.sessionActive = false
		p.ready = false
		p.readyWaitGen = 0
		p.retryAttempt = 0
		p.awaiting = ""
		p.captchaPoll = false
		p.mu.Unlock()
		p.emit("log", fmt.Sprintf("[reconnect] process exited while awaiting %s; automatic reconnect paused until manual restart", blockedReason))
		p.emitStateWithDetails("stopped", fmt.Sprintf("连接在等待 %s 时中断，请手动重新连接", blockedReason), 0, 0)
		return
	}
	p.ready = false
	p.retryGeneration++

	attempt, delay := p.scheduleRetryLocked()
	p.mu.Unlock()

	p.emit("log", fmt.Sprintf("[reconnect] process exited (%v), retrying in %s (attempt %d)", waitErr, formatRetryDelay(delay), attempt))
	p.emitStateWithDetails("retrying", fmt.Sprintf("连接已断开，将在 %s 后重试（第 %d 次）", formatRetryDelay(delay), attempt), attempt, delay)
}

func (p *ProxyManager) runRetryAttempt(generation uint64) {
	p.mu.Lock()
	if generation != p.retryGeneration || !p.sessionActive || p.awaiting != "" || p.cmd != nil || p.elevated {
		p.mu.Unlock()
		return
	}
	options := p.lastOptions
	captchaPath := p.captchaPath
	p.retryTimer = nil
	p.eipOptions = options
	p.mu.Unlock()

	p.emitStateWithDetails("connecting", "正在重新连接...", 0, 0)
	if err := p.startManaged(captchaPath, options); err != nil {
		p.mu.Lock()
		if generation != p.retryGeneration || !p.sessionActive {
			p.mu.Unlock()
			return
		}
		attempt, delay := p.scheduleRetryLocked()
		p.mu.Unlock()
		p.emit("log", fmt.Sprintf("[reconnect] retry start failed: %v; retrying in %s (attempt %d)", err, formatRetryDelay(delay), attempt))
		p.emitStateWithDetails("retrying", fmt.Sprintf("重新连接失败，将在 %s 后重试（第 %d 次）", formatRetryDelay(delay), attempt), attempt, delay)
	}
}

func (p *ProxyManager) nextRetryDelayLocked(attempt int) time.Duration {
	delay := p.retryBaseDelay
	if delay <= 0 {
		delay = time.Second
	}
	maxDelay := p.retryMaxDelay
	if maxDelay <= 0 {
		maxDelay = time.Minute
	}
	for i := 1; i < attempt; i++ {
		if delay >= maxDelay {
			break
		}
		delay *= 2
		if delay > maxDelay {
			delay = maxDelay
		}
	}
	if p.retryJitter != nil {
		delay = p.retryJitter(delay, attempt)
	}
	if delay < time.Second {
		delay = time.Second
	}
	if delay > maxDelay {
		delay = maxDelay
	}
	return delay
}

func (p *ProxyManager) scheduleRetryLocked() (int, time.Duration) {
	p.retryAttempt++
	attempt := p.retryAttempt
	delay := p.nextRetryDelayLocked(attempt)
	generation := p.retryGeneration
	afterFunc := p.afterFunc
	if afterFunc == nil {
		afterFunc = func(delay time.Duration, fn func()) retryTimer {
			return time.AfterFunc(delay, fn)
		}
	}
	p.retryTimer = afterFunc(delay, func() {
		p.runRetryAttempt(generation)
	})
	return attempt, delay
}

func (p *ProxyManager) stopRetryTimerLocked() {
	if p.retryTimer == nil {
		return
	}
	p.retryTimer.Stop()
	p.retryTimer = nil
}

func defaultRetryJitter(delay time.Duration, _ int) time.Duration {
	const jitterFraction = 0.2
	if delay <= 0 {
		return time.Second
	}
	spread := int64(float64(delay) * jitterFraction)
	if spread <= 0 {
		return delay
	}
	offset := rand.Int63n(spread*2+1) - spread
	jittered := int64(delay) + offset
	if jittered <= 0 {
		return time.Second
	}
	return time.Duration(jittered)
}

func formatRetryDelay(delay time.Duration) string {
	rounded := delay.Round(time.Second)
	if rounded < time.Second {
		return delay.String()
	}
	totalSeconds := int(rounded / time.Second)
	if totalSeconds < 60 {
		return fmt.Sprintf("%d 秒", totalSeconds)
	}
	minutes := totalSeconds / 60
	seconds := totalSeconds % 60
	if seconds == 0 {
		return fmt.Sprintf("%d 分钟", minutes)
	}
	return fmt.Sprintf("%d 分 %d 秒", minutes, seconds)
}

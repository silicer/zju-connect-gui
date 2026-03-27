package backend

import (
	"context"
	"errors"
	"reflect"
	"strings"
	"testing"
	"time"
)

type fakeRetryTimer struct {
	stopped bool
}

func (t *fakeRetryTimer) Stop() bool {
	wasRunning := !t.stopped
	t.stopped = true
	return wasRunning
}

type scheduledRetry struct {
	delay time.Duration
	fn    func()
	timer *fakeRetryTimer
}

func newTestProxyManager() (*ProxyManager, *[]scheduledRetry) {
	p := NewProxyManager("/tmp")
	p.retryJitter = func(delay time.Duration, _ int) time.Duration {
		return delay
	}
	scheduled := make([]scheduledRetry, 0, 4)
	p.afterFunc = func(delay time.Duration, fn func()) retryTimer {
		timer := &fakeRetryTimer{}
		scheduled = append(scheduled, scheduledRetry{delay: delay, fn: fn, timer: timer})
		return timer
	}
	return p, &scheduled
}

func testLaunchOptions() LaunchOptions {
	options := DefaultLaunchOptions()
	options.Username = "student"
	options.Password = "secret"
	return options
}

func primeReconnectSession(p *ProxyManager, options LaunchOptions) {
	p.mu.Lock()
	p.sessionActive = true
	p.lastOptions = options
	p.captchaPath = "gui_captcha.png"
	p.retryGeneration = 1
	p.mu.Unlock()
}

func TestIsVPNClientStartedLine(t *testing.T) {
	tests := []struct {
		name string
		line string
		want bool
	}{
		{name: "exact match", line: "VPN client started", want: true},
		{name: "embedded in log", line: "INFO VPN client started successfully", want: true},
		{name: "trimmed match", line: "  VPN client started  ", want: true},
		{name: "different message", line: "VPN client stopped", want: false},
		{name: "empty", line: "", want: false},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			if got := isVPNClientStartedLine(tt.line); got != tt.want {
				t.Fatalf("isVPNClientStartedLine(%q) = %v, want %v", tt.line, got, tt.want)
			}
		})
	}
}

func TestUnexpectedExitSchedulesReconnect(t *testing.T) {
	p, scheduled := newTestProxyManager()
	options := testLaunchOptions()
	primeReconnectSession(p, options)

	var starts int
	p.startProcess = func(captchaPath string, got LaunchOptions) error {
		starts++
		if captchaPath != "gui_captcha.png" {
			t.Fatalf("unexpected captcha path: %s", captchaPath)
		}
		if !reflect.DeepEqual(got, options) {
			t.Fatalf("unexpected retry options: %#v", got)
		}
		return nil
	}

	p.handleProcessExit(errors.New("network lost"))

	if len(*scheduled) != 1 {
		t.Fatalf("expected 1 scheduled reconnect, got %d", len(*scheduled))
	}
	if got := (*scheduled)[0].delay; got != time.Second {
		t.Fatalf("expected first retry delay to be %s, got %s", time.Second, got)
	}

	(*scheduled)[0].fn()

	if starts != 1 {
		t.Fatalf("expected retry start to run once, got %d", starts)
	}
}

func TestManualStopSuppressesReconnect(t *testing.T) {
	p, scheduled := newTestProxyManager()
	options := testLaunchOptions()
	primeReconnectSession(p, options)

	var starts int
	p.startProcess = func(string, LaunchOptions) error {
		starts++
		return nil
	}

	p.handleProcessExit(errors.New("network lost"))

	if len(*scheduled) != 1 {
		t.Fatalf("expected 1 scheduled reconnect, got %d", len(*scheduled))
	}
	if err := p.Stop(); err != nil {
		t.Fatalf("Stop() returned error: %v", err)
	}
	if !(*scheduled)[0].timer.stopped {
		t.Fatal("expected pending retry timer to be stopped")
	}

	(*scheduled)[0].fn()

	if starts != 0 {
		t.Fatalf("expected stopped session to suppress retries, got %d retry starts", starts)
	}
	if p.IsRunning() {
		t.Fatal("expected session to be inactive after Stop")
	}
}

func TestSuccessfulStartResetsBackoff(t *testing.T) {
	p, scheduled := newTestProxyManager()
	options := testLaunchOptions()
	primeReconnectSession(p, options)

	p.startProcess = func(string, LaunchOptions) error {
		return nil
	}

	p.handleProcessExit(errors.New("first disconnect"))
	if len(*scheduled) != 1 {
		t.Fatalf("expected first reconnect schedule, got %d", len(*scheduled))
	}
	if got := (*scheduled)[0].delay; got != time.Second {
		t.Fatalf("expected first retry delay to be %s, got %s", time.Second, got)
	}

	(*scheduled)[0].fn()
	p.handleLogLine("VPN client started")
	p.handleProcessExit(errors.New("second disconnect"))

	if len(*scheduled) != 2 {
		t.Fatalf("expected second reconnect schedule, got %d", len(*scheduled))
	}
	if got := (*scheduled)[1].delay; got != time.Second {
		t.Fatalf("expected retry delay reset to %s after success, got %s", time.Second, got)
	}
}

func TestPromptOrCaptchaBlocksRetry(t *testing.T) {
	p, scheduled := newTestProxyManager()
	options := testLaunchOptions()
	primeReconnectSession(p, options)

	p.setAwaiting("sms")
	p.handleProcessExit(errors.New("prompt blocked exit"))

	if len(*scheduled) != 0 {
		t.Fatalf("expected no reconnect while awaiting input, got %d schedules", len(*scheduled))
	}
	if p.IsRunning() {
		t.Fatal("expected interrupted prompt flow to require manual restart")
	}
}

func TestExplicitRestartStartsFresh(t *testing.T) {
	p, scheduled := newTestProxyManager()
	options := testLaunchOptions()
	primeReconnectSession(p, options)

	var starts int
	p.startProcess = func(string, LaunchOptions) error {
		starts++
		return nil
	}

	p.handleProcessExit(errors.New("network lost"))
	if len(*scheduled) != 1 {
		t.Fatalf("expected first reconnect schedule, got %d", len(*scheduled))
	}

	if err := p.Start(options); err != nil {
		t.Fatalf("Start() returned error: %v", err)
	}
	if starts != 1 {
		t.Fatalf("expected explicit restart to start immediately once, got %d starts", starts)
	}
	if !(*scheduled)[0].timer.stopped {
		t.Fatal("expected explicit restart to cancel previous retry timer")
	}

	p.handleProcessExit(errors.New("network lost again"))
	if len(*scheduled) != 2 {
		t.Fatalf("expected second reconnect schedule, got %d", len(*scheduled))
	}
	if got := (*scheduled)[1].delay; got != time.Second {
		t.Fatalf("expected explicit restart to reset retry delay to %s, got %s", time.Second, got)
	}
}

func TestRetryStartFailureKeepsRetrying(t *testing.T) {
	p, scheduled := newTestProxyManager()
	options := testLaunchOptions()
	primeReconnectSession(p, options)

	p.startProcess = func(string, LaunchOptions) error {
		return errors.New("temporary spawn failure")
	}

	p.handleProcessExit(errors.New("network lost"))
	if len(*scheduled) != 1 {
		t.Fatalf("expected first reconnect schedule, got %d", len(*scheduled))
	}

	(*scheduled)[0].fn()

	if len(*scheduled) != 2 {
		t.Fatalf("expected failed retry start to reschedule, got %d schedules", len(*scheduled))
	}
	if got := (*scheduled)[1].delay; got != 2*time.Second {
		t.Fatalf("expected second retry delay to back off to %s, got %s", 2*time.Second, got)
	}
	if !p.IsRunning() {
		t.Fatal("expected session to remain active after transient retry start failure")
	}
}

func TestInvalidStartDoesNotCancelPendingRetry(t *testing.T) {
	p, scheduled := newTestProxyManager()
	options := testLaunchOptions()
	primeReconnectSession(p, options)

	p.handleProcessExit(errors.New("network lost"))
	if len(*scheduled) != 1 {
		t.Fatalf("expected pending retry schedule, got %d", len(*scheduled))
	}

	invalid := options
	invalid.Username = ""
	if err := p.Start(invalid); err == nil {
		t.Fatal("expected invalid Start() to fail validation")
	}
	if (*scheduled)[0].timer.stopped {
		t.Fatal("expected invalid Start() not to cancel pending retry")
	}
	if !p.IsRunning() {
		t.Fatal("expected session to remain active after invalid restart attempt")
	}
}

func TestReadStreamFlushesFinalSuccessLine(t *testing.T) {
	p, _ := newTestProxyManager()
	p.mu.Lock()
	p.retryAttempt = 3
	p.mu.Unlock()

	p.readStream(context.TODO(), strings.NewReader("VPN client started"))

	p.mu.Lock()
	defer p.mu.Unlock()
	if p.retryAttempt != 0 {
		t.Fatalf("expected flushed success line to reset retryAttempt, got %d", p.retryAttempt)
	}
}

func TestReadStreamFlushesFinalPromptLine(t *testing.T) {
	p, scheduled := newTestProxyManager()
	options := testLaunchOptions()
	primeReconnectSession(p, options)

	p.readStream(context.TODO(), strings.NewReader("SMS verification code"))
	p.handleProcessExit(errors.New("process exited after prompt"))

	if len(*scheduled) != 0 {
		t.Fatalf("expected no reconnect after flushed prompt line, got %d schedules", len(*scheduled))
	}
	if p.IsRunning() {
		t.Fatal("expected flushed prompt line plus exit to stop session for manual restart")
	}
}

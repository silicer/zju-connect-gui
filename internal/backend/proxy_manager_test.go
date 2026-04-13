package backend

import (
	"context"
	"errors"
	"reflect"
	"strings"
	"sync"
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
	options.EIPAutoOpen = false
	return options
}

func primeReconnectSession(p *ProxyManager, options LaunchOptions) {
	p.mu.Lock()
	p.sessionActive = true
	p.eipOptions = options
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

func TestIsRouteAddedLine(t *testing.T) {
	tests := []struct {
		name string
		line string
		want bool
	}{
		{name: "exact match", line: "Add route to 10.0.0.0/8", want: true},
		{name: "embedded in log", line: "INFO Add route to 10.0.0.0/8", want: true},
		{name: "trimmed match", line: "  Add route to 10.0.0.0/8  ", want: true},
		{name: "different message", line: "Added route to 10.0.0.0/8", want: false},
		{name: "empty", line: "", want: false},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			if got := isRouteAddedLine(tt.line); got != tt.want {
				t.Fatalf("isRouteAddedLine(%q) = %v, want %v", tt.line, got, tt.want)
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
	options.TunMode = false
	primeReconnectSession(p, options)

	p.startProcess = func(string, LaunchOptions) error {
		return nil
	}
	ready := make(chan struct{}, 1)
	p.waitForHTTPReady = func(bind string, generation uint64) {
		if bind != options.HTTPBind {
			t.Fatalf("unexpected HTTP bind: %s", bind)
		}
		p.markReadyForGeneration(generation)
		ready <- struct{}{}
	}
	p.openEIP = func(LaunchOptions) error {
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
	select {
	case <-ready:
	case <-time.After(time.Second):
		t.Fatal("timed out waiting for HTTP readiness callback")
	}
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
	p.sessionActive = true
	p.retryGeneration = 1
	p.eipOptions = testLaunchOptions()
	p.eipOptions.TunMode = false
	p.retryAttempt = 3
	p.mu.Unlock()
	ready := make(chan struct{}, 1)
	p.waitForHTTPReady = func(bind string, generation uint64) {
		p.markReadyForGeneration(generation)
		ready <- struct{}{}
	}
	p.openEIP = func(LaunchOptions) error {
		return nil
	}

	p.readStream(context.TODO(), strings.NewReader("VPN client started"))
	select {
	case <-ready:
	case <-time.After(time.Second):
		t.Fatal("timed out waiting for HTTP readiness callback")
	}

	p.mu.Lock()
	defer p.mu.Unlock()
	if p.retryAttempt != 0 {
		t.Fatalf("expected flushed success line to reset retryAttempt, got %d", p.retryAttempt)
	}
	if !p.ready {
		t.Fatal("expected flushed success line to mark session ready after HTTP bind probe")
	}
}

func TestNonTunStartWaitsForHTTPBindBeforeConnected(t *testing.T) {
	p, _ := newTestProxyManager()
	options := testLaunchOptions()
	options.TunMode = false
	primeReconnectSession(p, options)

	called := make(chan uint64, 1)
	p.waitForHTTPReady = func(bind string, generation uint64) {
		if bind != options.HTTPBind {
			t.Fatalf("unexpected HTTP bind: %s", bind)
		}
		called <- generation
	}

	p.mu.Lock()
	p.retryAttempt = 3
	p.mu.Unlock()

	p.handleLogLine("VPN client started")

	var generation uint64
	select {
	case generation = <-called:
	case <-time.After(time.Second):
		t.Fatal("timed out waiting for HTTP readiness wait to start")
	}

	p.mu.Lock()
	if p.retryAttempt != 3 {
		p.mu.Unlock()
		t.Fatalf("expected retryAttempt to remain unchanged before HTTP bind is ready, got %d", p.retryAttempt)
	}
	if p.ready {
		p.mu.Unlock()
		t.Fatal("expected session to remain not ready before HTTP bind is ready")
	}
	p.mu.Unlock()

	p.openEIP = func(LaunchOptions) error {
		return nil
	}
	p.markReadyForGeneration(generation)

	p.mu.Lock()
	defer p.mu.Unlock()
	if p.retryAttempt != 0 {
		t.Fatalf("expected retryAttempt reset after HTTP bind became ready, got %d", p.retryAttempt)
	}
	if !p.ready {
		t.Fatal("expected session ready after HTTP bind became ready")
	}
}

func TestTunStartWaitsForAddRouteLogBeforeConnected(t *testing.T) {
	p, scheduled := newTestProxyManager()
	options := testLaunchOptions()
	options.TunMode = true
	options.EIPAutoOpen = true
	primeReconnectSession(p, options)

	p.mu.Lock()
	p.retryAttempt = 3
	p.mu.Unlock()
	called := make(chan uint64, 1)
	p.waitForHTTPReady = func(bind string, generation uint64) {
		if bind != options.HTTPBind {
			t.Fatalf("unexpected HTTP bind: %s", bind)
		}
		called <- generation
	}

	p.handleLogLine("VPN client started")
	p.mu.Lock()
	if p.retryAttempt != 3 {
		p.mu.Unlock()
		t.Fatalf("expected TUN mode to ignore VPN client started for readiness, got retryAttempt %d", p.retryAttempt)
	}
	if p.ready {
		p.mu.Unlock()
		t.Fatal("expected TUN mode to remain not ready before route log")
	}
	p.mu.Unlock()

	p.handleLogLine("Add route to 10.0.0.0/8")
	var generation uint64
	select {
	case generation = <-called:
	case <-time.After(time.Second):
		t.Fatal("timed out waiting for TUN readiness wait to start")
	}

	p.mu.Lock()
	if p.retryAttempt != 3 {
		p.mu.Unlock()
		t.Fatalf("expected route signal alone not to reset retryAttempt, got %d", p.retryAttempt)
	}
	if p.ready {
		p.mu.Unlock()
		t.Fatal("expected route signal alone not to mark session ready")
	}
	p.mu.Unlock()

	ready := make(chan struct{}, 1)
	p.openEIP = func(LaunchOptions) error {
		ready <- struct{}{}
		return nil
	}
	p.markReadyForGeneration(generation)
	if len(*scheduled) != 1 {
		t.Fatalf("expected 1 delayed EIP open schedule, got %d", len(*scheduled))
	}
	(*scheduled)[0].fn()
	select {
	case <-ready:
	case <-time.After(time.Second):
		t.Fatal("timed out waiting for TUN HTTP readiness to open EIP")
	}

	p.mu.Lock()
	defer p.mu.Unlock()
	if p.retryAttempt != 0 {
		t.Fatalf("expected retryAttempt reset after TUN readiness completed, got %d", p.retryAttempt)
	}
	if !p.ready {
		t.Fatal("expected session ready after TUN readiness completed")
	}
}

func TestRouteLogDoesNotConnectInNonTunMode(t *testing.T) {
	p, _ := newTestProxyManager()
	options := testLaunchOptions()
	options.TunMode = false
	primeReconnectSession(p, options)

	p.mu.Lock()
	p.retryAttempt = 2
	p.mu.Unlock()
	p.openEIP = func(LaunchOptions) error {
		t.Fatal("did not expect EIP open in non-TUN route log test")
		return nil
	}

	p.handleLogLine("Add route to 10.0.0.0/8")

	p.mu.Lock()
	defer p.mu.Unlock()
	if p.retryAttempt != 2 {
		t.Fatalf("expected route log ignored in non-TUN mode, got retryAttempt %d", p.retryAttempt)
	}
	if p.ready {
		t.Fatal("expected route log ignored in non-TUN mode")
	}
}

func TestStaleHTTPReadyResultIgnoredAfterStop(t *testing.T) {
	p, _ := newTestProxyManager()
	options := testLaunchOptions()
	options.TunMode = false
	primeReconnectSession(p, options)

	called := make(chan uint64, 1)
	p.waitForHTTPReady = func(_ string, generation uint64) {
		called <- generation
	}
	p.openEIP = func(LaunchOptions) error {
		t.Fatal("did not expect stale readiness result to open EIP")
		return nil
	}

	p.handleLogLine("VPN client started")

	var generation uint64
	select {
	case generation = <-called:
	case <-time.After(time.Second):
		t.Fatal("timed out waiting for HTTP readiness wait to start")
	}

	if err := p.Stop(); err != nil {
		t.Fatalf("Stop() returned error: %v", err)
	}
	p.markReadyForGeneration(generation)

	p.mu.Lock()
	defer p.mu.Unlock()
	if p.ready {
		t.Fatal("expected stale readiness result to be ignored after Stop")
	}
}

func TestMarkReadyOpensEIPOnlyOnce(t *testing.T) {
	p, scheduled := newTestProxyManager()
	options := testLaunchOptions()
	options.TunMode = true
	options.EIPAutoOpen = true
	primeReconnectSession(p, options)

	var mu sync.Mutex
	openCalls := 0
	opened := make(chan struct{}, 1)
	p.openEIP = func(LaunchOptions) error {
		mu.Lock()
		openCalls++
		mu.Unlock()
		opened <- struct{}{}
		return nil
	}

	p.markReadyForGeneration(1)
	if len(*scheduled) != 1 {
		t.Fatalf("expected 1 delayed EIP open schedule, got %d", len(*scheduled))
	}
	(*scheduled)[0].fn()
	select {
	case <-opened:
	case <-time.After(time.Second):
		t.Fatal("timed out waiting for first EIP open")
	}
	p.markReadyForGeneration(1)
	time.Sleep(50 * time.Millisecond)

	mu.Lock()
	defer mu.Unlock()
	if openCalls != 1 {
		t.Fatalf("expected EIP to open once, got %d calls", openCalls)
	}
}

func TestMarkReadyWithAutoOpenDisabledDoesNotScheduleOpen(t *testing.T) {
	p, scheduled := newTestProxyManager()
	options := testLaunchOptions()
	options.TunMode = false
	options.EIPAutoOpen = false
	primeReconnectSession(p, options)

	p.openEIP = func(LaunchOptions) error {
		t.Fatal("did not expect EIP open when auto-open disabled")
		return nil
	}

	p.markReadyForGeneration(1)

	if len(*scheduled) != 0 {
		t.Fatalf("expected no delayed EIP open schedule, got %d", len(*scheduled))
	}
	p.mu.Lock()
	defer p.mu.Unlock()
	if !p.ready {
		t.Fatal("expected session ready even when EIP auto-open disabled")
	}
}

func TestMarkReadySchedulesDelayedEIPOpen(t *testing.T) {
	p, scheduled := newTestProxyManager()
	options := testLaunchOptions()
	options.TunMode = false
	options.EIPAutoOpen = true
	primeReconnectSession(p, options)
	p.autoOpenDelay = func() time.Duration {
		return 4 * time.Second
	}

	openCalls := 0
	p.openEIP = func(got LaunchOptions) error {
		openCalls++
		if !reflect.DeepEqual(got, options) {
			t.Fatalf("unexpected delayed EIP options: %#v", got)
		}
		return nil
	}

	p.markReadyForGeneration(1)

	if len(*scheduled) != 1 {
		t.Fatalf("expected 1 delayed EIP open schedule, got %d", len(*scheduled))
	}
	if got := (*scheduled)[0].delay; got != 4*time.Second {
		t.Fatalf("expected delayed EIP open after %s, got %s", 4*time.Second, got)
	}
	if openCalls != 0 {
		t.Fatalf("expected delayed EIP open not to run before timer, got %d calls", openCalls)
	}

	(*scheduled)[0].fn()

	if openCalls != 1 {
		t.Fatalf("expected delayed EIP open to run once, got %d calls", openCalls)
	}
}

func TestDelayedEIPOpenCanceledByStop(t *testing.T) {
	p, scheduled := newTestProxyManager()
	options := testLaunchOptions()
	options.TunMode = false
	options.EIPAutoOpen = true
	primeReconnectSession(p, options)
	p.autoOpenDelay = func() time.Duration {
		return 4 * time.Second
	}

	openCalls := 0
	p.openEIP = func(LaunchOptions) error {
		openCalls++
		return nil
	}

	p.markReadyForGeneration(1)
	if len(*scheduled) != 1 {
		t.Fatalf("expected 1 delayed EIP open schedule, got %d", len(*scheduled))
	}

	if err := p.Stop(); err != nil {
		t.Fatalf("Stop() returned error: %v", err)
	}
	if !(*scheduled)[0].timer.stopped {
		t.Fatal("expected Stop to cancel delayed EIP open timer")
	}

	(*scheduled)[0].fn()

	if openCalls != 0 {
		t.Fatalf("expected stopped session to suppress delayed EIP open, got %d calls", openCalls)
	}
}

func TestDelayedEIPOpenCanceledAfterGenerationChange(t *testing.T) {
	p, scheduled := newTestProxyManager()
	options := testLaunchOptions()
	options.TunMode = false
	options.EIPAutoOpen = true
	primeReconnectSession(p, options)
	p.autoOpenDelay = func() time.Duration {
		return 4 * time.Second
	}

	openCalls := 0
	p.openEIP = func(LaunchOptions) error {
		openCalls++
		return nil
	}

	p.markReadyForGeneration(1)
	if len(*scheduled) != 1 {
		t.Fatalf("expected 1 delayed EIP open schedule, got %d", len(*scheduled))
	}

	p.handleProcessExit(errors.New("network lost"))
	if !(*scheduled)[0].timer.stopped {
		t.Fatal("expected generation change to cancel delayed EIP open timer")
	}

	(*scheduled)[0].fn()

	if openCalls != 0 {
		t.Fatalf("expected stale delayed EIP open to be ignored after generation change, got %d calls", openCalls)
	}
}

func TestReadStreamFlushesFinalRouteLine(t *testing.T) {
	p, _ := newTestProxyManager()
	options := testLaunchOptions()
	options.TunMode = true
	primeReconnectSession(p, options)

	p.mu.Lock()
	p.retryAttempt = 3
	p.mu.Unlock()
	readyWait := make(chan struct{}, 1)
	p.waitForHTTPReady = func(bind string, generation uint64) {
		if bind != options.HTTPBind {
			t.Fatalf("unexpected HTTP bind: %s", bind)
		}
		p.markReadyForGeneration(generation)
		readyWait <- struct{}{}
	}
	p.openEIP = func(LaunchOptions) error {
		return nil
	}

	p.readStream(context.TODO(), strings.NewReader("Add route to 10.0.0.0/8"))
	select {
	case <-readyWait:
	case <-time.After(time.Second):
		t.Fatal("timed out waiting for flushed route line readiness wait")
	}

	p.mu.Lock()
	defer p.mu.Unlock()
	if p.retryAttempt != 0 {
		t.Fatalf("expected flushed route line to reset retryAttempt, got %d", p.retryAttempt)
	}
	if !p.ready {
		t.Fatal("expected flushed route line to mark session ready")
	}
}

func TestStaleHTTPReadyResultIgnoredAfterUnexpectedExit(t *testing.T) {
	p, scheduled := newTestProxyManager()
	options := testLaunchOptions()
	options.TunMode = false
	primeReconnectSession(p, options)

	called := make(chan uint64, 1)
	p.waitForHTTPReady = func(_ string, generation uint64) {
		called <- generation
	}
	p.handleLogLine("VPN client started")

	var generation uint64
	select {
	case generation = <-called:
	case <-time.After(time.Second):
		t.Fatal("timed out waiting for HTTP readiness wait to start")
	}

	p.handleProcessExit(errors.New("network lost"))
	if len(*scheduled) != 1 {
		t.Fatalf("expected reconnect schedule after unexpected exit, got %d", len(*scheduled))
	}
	p.markReadyForGeneration(generation)

	p.mu.Lock()
	defer p.mu.Unlock()
	if p.ready {
		t.Fatal("expected stale readiness result to be ignored after unexpected exit")
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

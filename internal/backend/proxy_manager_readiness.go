package backend

import (
	"fmt"
	"net"
	"time"
)

const httpReadyPollInterval = 100 * time.Millisecond

func (p *ProxyManager) readinessMode() (bool, string) {
	p.mu.Lock()
	defer p.mu.Unlock()
	return p.eipOptions.TunMode, p.eipOptions.HTTPBind
}

func (p *ProxyManager) beginHTTPReadyWait(bind string) {
	p.mu.Lock()
	if !p.sessionActive || p.ready || p.readyWaitGen == p.retryGeneration {
		p.mu.Unlock()
		return
	}
	generation := p.retryGeneration
	p.readyWaitGen = generation
	waitFn := p.waitForHTTPReady
	p.mu.Unlock()

	if bind == "" {
		p.markReadyForGeneration(generation)
		return
	}
	if waitFn == nil {
		waitFn = p.waitForHTTPBindReady
	}
	go waitFn(bind, generation)
}

func (p *ProxyManager) waitForHTTPBindReady(bind string, generation uint64) {
	target := readinessDialAddress(bind)
	for {
		if !p.shouldContinueReadyWait(generation) {
			return
		}
		conn, err := net.DialTimeout("tcp", target, 200*time.Millisecond)
		if err == nil {
			_ = conn.Close()
			p.markReadyForGeneration(generation)
			return
		}
		time.Sleep(httpReadyPollInterval)
	}
}

func (p *ProxyManager) shouldContinueReadyWait(generation uint64) bool {
	p.mu.Lock()
	defer p.mu.Unlock()
	return p.sessionActive && !p.ready && p.retryGeneration == generation && p.readyWaitGen == generation
}

func (p *ProxyManager) openEIPURLOnce() {
	p.mu.Lock()
	if p.eipOpened {
		p.mu.Unlock()
		return
	}
	ctx := p.ctx
	options := p.eipOptions
	openEIP := p.openEIP
	p.eipOpened = true
	p.mu.Unlock()
	if openEIP == nil {
		openEIP = OpenEIP
	}

	go func() {
		if err := openEIP(ctx, options); err != nil {
			p.mu.Lock()
			p.eipOpened = false
			p.mu.Unlock()
			p.emit("log", fmt.Sprintf("[eip] failed to open EIP URL: %v", err))
		}
	}()
}

func (p *ProxyManager) markReady() {
	p.mu.Lock()
	generation := p.retryGeneration
	p.mu.Unlock()
	p.markReadyForGeneration(generation)
}

func (p *ProxyManager) markReadyForGeneration(generation uint64) {
	p.mu.Lock()
	if generation != p.retryGeneration || !p.sessionActive || p.ready {
		p.mu.Unlock()
		return
	}
	p.ready = true
	p.readyWaitGen = 0
	p.retryAttempt = 0
	p.mu.Unlock()
	p.emitStateWithDetails("connected", "已启动", 0, 0)
	p.openEIPURLOnce()
}

func readinessDialAddress(bind string) string {
	host, port, err := net.SplitHostPort(bind)
	if err != nil {
		return bind
	}
	switch host {
	case "", "0.0.0.0":
		host = "127.0.0.1"
	case "::":
		host = "::1"
	}
	return net.JoinHostPort(host, port)
}

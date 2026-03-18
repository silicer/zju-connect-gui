package main

import (
	"context"
	"errors"
	"log"
	"sync"

	"github.com/wailsapp/wails/v2/pkg/runtime"
)

// App struct
type App struct {
	ctx   context.Context
	proxy *ProxyManager

	closeMu    sync.Mutex
	allowClose bool
}

// NewApp creates a new App application struct
func NewApp() *App {
	return &App{}
}

// startup is called when the app starts. The context is saved
// so we can call the runtime methods
func (a *App) startup(ctx context.Context) {
	a.ctx = ctx

	appDir, err := ResolveAppDir()
	if err != nil {
		log.Printf("failed to resolve app dir: %v", err)
		return
	}

	proxy := NewProxyManager(appDir)
	proxy.SetContext(ctx)

	a.proxy = proxy
}

// Greet returns a greeting for the given name
func (a *App) onBeforeClose(_ context.Context) bool {
	a.closeMu.Lock()
	allow := a.allowClose
	a.closeMu.Unlock()
	if allow {
		return false
	}

	if a.ctx != nil {
		runtime.WindowHide(a.ctx)
	}
	return true
}

func (a *App) shutdown(_ context.Context) {
	if a.proxy != nil {
		_ = a.proxy.Stop()
	}
	quitTray()
}

func (a *App) ShowWindow() {
	if a.ctx == nil {
		return
	}
	runtime.WindowShow(a.ctx)
	runtime.WindowUnminimise(a.ctx)
	runtime.WindowSetAlwaysOnTop(a.ctx, true)
	runtime.WindowSetAlwaysOnTop(a.ctx, false)
}

func (a *App) HideWindow() {
	if a.ctx == nil {
		return
	}
	runtime.WindowHide(a.ctx)
}

func (a *App) Quit() {
	a.closeMu.Lock()
	a.allowClose = true
	a.closeMu.Unlock()

	if a.proxy != nil {
		_ = a.proxy.Stop()
	}
	quitTray()
	if a.ctx != nil {
		runtime.Quit(a.ctx)
	}
}

func (a *App) Start(options LaunchOptions) error {
	if a.proxy == nil {
		return errors.New("proxy manager not initialized")
	}
	return a.proxy.Start(options)
}

func (a *App) Stop() error {
	if a.proxy == nil {
		return nil
	}
	return a.proxy.Stop()
}

func (a *App) SubmitInput(value string) error {
	if a.proxy == nil {
		return errors.New("proxy manager not initialized")
	}
	return a.proxy.SubmitInput(value)
}

func (a *App) IsRunning() bool {
	if a.proxy == nil {
		return false
	}
	return a.proxy.IsRunning()
}

package main

import (
	"context"
	"errors"
	"log"
	stdRuntime "runtime"
	"strings"
	"sync"
	"time"

	"zju-connect-gui/internal/backend"

	"github.com/wailsapp/wails/v2/pkg/options"
	wailsRuntime "github.com/wailsapp/wails/v2/pkg/runtime"
)

// App struct
type App struct {
	ctx     context.Context
	proxy   *backend.ProxyManager
	store   *backend.UserSettingsStore
	pending *backend.PendingConnectStore
	appDir  string

	closeMu    sync.Mutex
	allowClose bool
}

type LaunchOptions = backend.LaunchOptions

// NewApp creates a new App application struct
func NewApp() *App {
	return &App{}
}

// startup is called when the app starts. The context is saved
// so we can call the runtime methods
func (a *App) startup(ctx context.Context) {
	a.ctx = ctx

	appDir, err := backend.ResolveAppDir()
	if err != nil {
		log.Printf("failed to resolve app dir: %v", err)
		return
	}

	proxy := backend.NewProxyManager(appDir)
	proxy.SetContext(ctx)
	store := backend.NewUserSettingsStore(appDir)
	pending := backend.NewPendingConnectStore(appDir)

	a.proxy = proxy
	a.store = store
	a.pending = pending
	a.appDir = appDir
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
		wailsRuntime.WindowHide(a.ctx)
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
	wailsRuntime.WindowShow(a.ctx)
	wailsRuntime.WindowUnminimise(a.ctx)
	wailsRuntime.WindowSetAlwaysOnTop(a.ctx, true)
	wailsRuntime.WindowSetAlwaysOnTop(a.ctx, false)
}

func (a *App) onSecondInstanceLaunch(secondInstanceData options.SecondInstanceData) {
	a.ShowWindow()
	if a.ctx == nil {
		return
	}
	if len(secondInstanceData.Args) == 0 {
		return
	}
	go wailsRuntime.EventsEmit(a.ctx, "log", "Second instance launch intercepted with args: "+strings.Join(secondInstanceData.Args, " "))
}

func (a *App) HideWindow() {
	if a.ctx == nil {
		return
	}
	wailsRuntime.WindowHide(a.ctx)
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
		wailsRuntime.Quit(a.ctx)
	}
}

func (a *App) Start(options LaunchOptions) error {
	if a.proxy == nil {
		return errors.New("proxy manager not initialized")
	}
	if a.store != nil {
		if err := a.store.Save(options); err != nil {
			return err
		}
	}

	if stdRuntime.GOOS == "windows" && options.TunMode {
		elevated, err := backend.IsProcessElevated()
		if err != nil {
			return err
		}
		if !elevated {
			if a.pending == nil {
				return errors.New("pending connect store not initialized")
			}
			if err := a.pending.MarkResumeConnect(); err != nil {
				return err
			}
			if err := backend.RelaunchSelfElevated(a.appDir, nil); err != nil {
				_ = a.pending.Clear()
				return err
			}
			go func() {
				time.Sleep(200 * time.Millisecond)
				a.Quit()
			}()
			return nil
		}
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

func (a *App) GetSavedLaunchOptions() (LaunchOptions, error) {
	if a.store == nil {
		return backend.DefaultLaunchOptions(), errors.New("settings store not initialized")
	}
	return a.store.Load()
}

func (a *App) SaveLaunchOptions(options LaunchOptions) error {
	if a.store == nil {
		return errors.New("settings store not initialized")
	}
	return a.store.Save(options)
}

func (a *App) ResumePendingConnect() (bool, error) {
	if a.pending == nil {
		return false, errors.New("pending connect store not initialized")
	}
	if a.store == nil {
		return false, errors.New("settings store not initialized")
	}
	if a.proxy == nil {
		return false, errors.New("proxy manager not initialized")
	}

	pending, err := a.pending.HasResumeConnect()
	if err != nil || !pending {
		return false, err
	}

	if stdRuntime.GOOS == "windows" {
		elevated, err := backend.IsProcessElevated()
		if err != nil {
			return false, err
		}
		if !elevated {
			return false, errors.New("pending TUN resume requires elevated application")
		}
	}

	options, err := a.store.Load()
	if err != nil {
		return false, err
	}
	if err := a.proxy.Start(options); err != nil {
		return false, err
	}
	if err := a.pending.Clear(); err != nil {
		return true, err
	}

	return true, nil
}

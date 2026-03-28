package main

import (
	"context"
	"errors"
	"log"
	"os"
	stdRuntime "runtime"
	"strings"
	"sync"

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

func (a *App) OpenEIP() {
	options := backend.DefaultLaunchOptions()
	if a.store != nil {
		saved, err := a.store.Load()
		if err != nil {
			log.Printf("failed to load saved launch options for EIP: %v", err)
		} else {
			options = saved
		}
	}

	if err := backend.OpenEIP(a.ctx, options); err != nil {
		log.Printf("failed to open EIP URL: %v", err)
	}
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
	backendOptions := options.toBackend()
	if a.store != nil {
		if err := a.store.Save(backendOptions); err != nil {
			return err
		}
	}

	if stdRuntime.GOOS == "windows" && backendOptions.TunMode {
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
			extraArgs := backend.BuildElevatedRelaunchArgs(os.Getpid())
			if err := backend.RelaunchSelfElevated(a.appDir, extraArgs); err != nil {
				_ = a.pending.Clear()
				return err
			}
			go func() {
				a.Quit()
			}()
			return nil
		}
	}

	return a.proxy.Start(backendOptions)
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
		return launchOptionsFromBackend(backend.DefaultLaunchOptions()), errors.New("settings store not initialized")
	}
	options, err := a.store.Load()
	return launchOptionsFromBackend(options), err
}

func (a *App) SaveLaunchOptions(options LaunchOptions) error {
	if a.store == nil {
		return errors.New("settings store not initialized")
	}
	return a.store.Save(options.toBackend())
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

func (a *App) PickEIPBrowserProgram() (string, error) {
	if a.ctx == nil {
		return "", errors.New("context not initialized")
	}

	path, err := wailsRuntime.OpenFileDialog(a.ctx, wailsRuntime.OpenDialogOptions{
		Title: "选择浏览器程序",
	})
	if err != nil {
		return "", err
	}

	return path, nil
}

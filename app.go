package main

import (
	"errors"
	"log"
	"os"
	stdRuntime "runtime"
	"strings"
	"sync"

	"zju-connect-gui/internal/backend"
)

// App struct
type App struct {
	ui      DesktopUI
	proxy   *backend.ProxyManager
	store   *backend.UserSettingsStore
	pending *backend.PendingConnectStore
	appDir  string

	closeMu    sync.Mutex
	allowClose bool
}

// NewApp creates a new App application struct
func NewApp(ui DesktopUI) *App {
	return &App{ui: ui}
}

func (a *App) SetUI(ui DesktopUI) {
	a.ui = ui
}

func (a *App) startup() error {
	appDir, err := backend.ResolveAppDir()
	if err != nil {
		return err
	}

	proxy := backend.NewProxyManager(appDir)
	proxy.SetUI(a)
	store := backend.NewUserSettingsStore(appDir)
	pending := backend.NewPendingConnectStore(appDir)

	a.proxy = proxy
	a.store = store
	a.pending = pending
	a.appDir = appDir
	return nil
}

func (a *App) EmitEvent(event string, payload any) {
	if a.ui == nil {
		return
	}
	a.ui.EmitEvent(event, payload)
}

func (a *App) shutdown() {
	if a.proxy != nil {
		_ = a.proxy.Stop()
	}
	quitTray()
}

func (a *App) canClose() bool {
	a.closeMu.Lock()
	defer a.closeMu.Unlock()
	return a.allowClose
}

func (a *App) ShowWindow() {
	if a.ui == nil {
		return
	}
	a.ui.ShowWindow()
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

	if err := backend.OpenEIP(options); err != nil {
		log.Printf("failed to open EIP URL: %v", err)
	}
}

func (a *App) onSecondInstanceLaunch(args []string) {
	a.ShowWindow()
	if a.ui == nil {
		return
	}
	if len(args) == 0 {
		return
	}
	go a.EmitEvent("log", "Second instance launch intercepted with args: "+strings.Join(args, " "))
}

func (a *App) HideWindow() {
	if a.ui == nil {
		return
	}
	a.ui.HideWindow()
}

func (a *App) Quit() {
	a.closeMu.Lock()
	a.allowClose = true
	a.closeMu.Unlock()

	if a.proxy != nil {
		_ = a.proxy.Stop()
	}
	quitTray()
	if a.ui != nil {
		a.ui.Quit()
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
	if a.ui == nil {
		return "", errors.New("desktop UI not initialized")
	}

	path, err := a.ui.OpenFileDialog("选择浏览器程序")
	if err != nil {
		return "", err
	}

	return path, nil
}

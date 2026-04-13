package backend

import (
	"encoding/json"
	"os"
	"path/filepath"
	"testing"
	"time"
)

func TestDefaultLaunchOptions_FixedDefaults(t *testing.T) {
	defaults := DefaultLaunchOptions()

	if defaults.Port != defaultPort {
		t.Fatalf("expected fixed port %d, got %d", defaultPort, defaults.Port)
	}
	if !defaults.EIPAutoOpen {
		t.Fatal("expected default launch options to enable EIP auto-open when no settings exist")
	}
	if !defaults.TunMode {
		t.Fatal("expected default launch options to enable TUN when no settings exist")
	}
}

func TestUserSettingsStoreLoad_ForcesFixedPort(t *testing.T) {
	tempDir := t.TempDir()
	store := NewUserSettingsStore(tempDir)

	if err := store.Save(LaunchOptions{
		Port:      8443,
		Username:  "user1",
		Password:  "pass1",
		SocksBind: "127.0.0.1:2080",
		HTTPBind:  "127.0.0.1:2888",
		TunMode:   false,
	}); err != nil {
		t.Fatalf("save settings: %v", err)
	}

	options, err := store.Load()
	if err != nil {
		t.Fatalf("load settings: %v", err)
	}

	if options.Port != defaultPort {
		t.Fatalf("expected fixed port %d after load, got %d", defaultPort, options.Port)
	}
	if options.TunMode {
		t.Fatal("expected explicit tun setting to be preserved when loading settings")
	}
}

func TestUserSettingsStoreLoad_PreservesEIPBrowserSettings(t *testing.T) {
	tempDir := t.TempDir()
	store := NewUserSettingsStore(tempDir)

	if err := store.Save(LaunchOptions{
		Username:          "user1",
		Password:          "pass1",
		SocksBind:         "127.0.0.1:1080",
		HTTPBind:          "127.0.0.1:8888",
		TunMode:           true,
		EIPAutoOpen:       false,
		EIPBrowserProgram: "  /usr/bin/browser  ",
		EIPBrowserArgs:    []string{" --new-window ", "", " --profile "},
	}); err != nil {
		t.Fatalf("save settings: %v", err)
	}

	options, err := store.Load()
	if err != nil {
		t.Fatalf("load settings: %v", err)
	}

	if options.EIPBrowserProgram != "/usr/bin/browser" {
		t.Fatalf("expected normalized browser program, got %q", options.EIPBrowserProgram)
	}
	if options.EIPAutoOpen {
		t.Fatal("expected disabled EIP auto-open setting to persist")
	}
	if len(options.EIPBrowserArgs) != 2 {
		t.Fatalf("expected 2 normalized browser args, got %#v", options.EIPBrowserArgs)
	}
	if options.EIPBrowserArgs[0] != "--new-window" || options.EIPBrowserArgs[1] != "--profile" {
		t.Fatalf("unexpected normalized browser args: %#v", options.EIPBrowserArgs)
	}
}

func TestUserSettingsStoreLoad_MissingEIPAutoOpenDefaultsEnabled(t *testing.T) {
	tempDir := t.TempDir()
	store := NewUserSettingsStore(tempDir)

	legacy := []byte(`{
	  "username": "user1",
	  "password": "pass1",
	  "socksBind": "127.0.0.1:1080",
	  "httpBind": "127.0.0.1:8888",
	  "tunMode": false,
	  "eipBrowserProgram": "/usr/bin/browser",
	  "eipBrowserArgs": ["--new-window"]
	}`)
	if err := os.WriteFile(filepath.Join(tempDir, settingsFileName), legacy, 0o600); err != nil {
		t.Fatalf("write legacy settings: %v", err)
	}

	options, err := store.Load()
	if err != nil {
		t.Fatalf("load settings: %v", err)
	}

	if !options.EIPAutoOpen {
		t.Fatal("expected legacy settings without eipAutoOpen to default enabled")
	}
}

func TestPendingConnectStoreHasResumeConnect_ClearsStaleMarker(t *testing.T) {
	tempDir := t.TempDir()
	store := NewPendingConnectStore(tempDir)

	stale, err := json.Marshal(pendingConnectState{
		ResumeConnect: true,
		CreatedAt:     time.Now().UTC().Add(-pendingConnectMaxAge - time.Minute),
	})
	if err != nil {
		t.Fatalf("marshal stale pending state: %v", err)
	}
	if err := os.WriteFile(filepath.Join(tempDir, pendingConnectFileName), stale, 0o600); err != nil {
		t.Fatalf("write stale pending state: %v", err)
	}

	pending, err := store.HasResumeConnect()
	if err != nil {
		t.Fatalf("read pending state: %v", err)
	}
	if pending {
		t.Fatal("expected stale pending state to be ignored")
	}
	if _, err := os.Stat(filepath.Join(tempDir, pendingConnectFileName)); !os.IsNotExist(err) {
		t.Fatalf("expected stale pending marker to be removed, stat err=%v", err)
	}
}

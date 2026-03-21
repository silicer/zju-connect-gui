package backend

import "testing"

func TestDefaultLaunchOptions_FixedDefaults(t *testing.T) {
	defaults := DefaultLaunchOptions()

	if defaults.Port != defaultPort {
		t.Fatalf("expected fixed port %d, got %d", defaultPort, defaults.Port)
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
	if len(options.EIPBrowserArgs) != 2 {
		t.Fatalf("expected 2 normalized browser args, got %#v", options.EIPBrowserArgs)
	}
	if options.EIPBrowserArgs[0] != "--new-window" || options.EIPBrowserArgs[1] != "--profile" {
		t.Fatalf("unexpected normalized browser args: %#v", options.EIPBrowserArgs)
	}
}

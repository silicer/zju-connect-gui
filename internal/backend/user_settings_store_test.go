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

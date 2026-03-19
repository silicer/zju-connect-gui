package backend

import (
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"sync"
)

const settingsFileName = "gui_settings.json"

type UserSettingsStore struct {
	path string
	mu   sync.Mutex
}

func NewUserSettingsStore(appDir string) *UserSettingsStore {
	return &UserSettingsStore{
		path: filepath.Join(appDir, settingsFileName),
	}
}

func DefaultLaunchOptions() LaunchOptions {
	defaults := normalizeLaunchOptions(LaunchOptions{})
	defaults.Protocol = defaultProtocol
	defaults.Server = defaultServer
	defaults.SecondaryDNSServer = defaultSecondaryDNSServer
	defaults.AuthType = defaultAuthType
	defaults.LoginDomain = defaultLoginDomain
	defaults.ClientDataFile = defaultClientDataFile
	return defaults
}

func (s *UserSettingsStore) Load() (LaunchOptions, error) {
	s.mu.Lock()
	defer s.mu.Unlock()

	defaults := DefaultLaunchOptions()
	data, err := os.ReadFile(s.path)
	if errors.Is(err, os.ErrNotExist) {
		return defaults, nil
	}
	if err != nil {
		return defaults, fmt.Errorf("failed to read settings: %w", err)
	}

	var options LaunchOptions
	if err := json.Unmarshal(data, &options); err != nil {
		return defaults, fmt.Errorf("failed to parse settings: %w", err)
	}

	options = normalizeLaunchOptions(options)
	options.Protocol = defaultProtocol
	options.Server = defaultServer
	options.SecondaryDNSServer = defaultSecondaryDNSServer
	options.AuthType = defaultAuthType
	options.LoginDomain = defaultLoginDomain
	options.ClientDataFile = defaultClientDataFile

	return options, nil
}

func (s *UserSettingsStore) Save(options LaunchOptions) error {
	s.mu.Lock()
	defer s.mu.Unlock()

	options = normalizeLaunchOptions(options)
	options.Protocol = defaultProtocol
	options.Server = defaultServer
	options.SecondaryDNSServer = defaultSecondaryDNSServer
	options.AuthType = defaultAuthType
	options.LoginDomain = defaultLoginDomain
	options.ClientDataFile = defaultClientDataFile

	data, err := json.MarshalIndent(options, "", "  ")
	if err != nil {
		return fmt.Errorf("failed to marshal settings: %w", err)
	}

	if err := os.WriteFile(s.path, data, 0o600); err != nil {
		return fmt.Errorf("failed to write settings: %w", err)
	}

	return nil
}

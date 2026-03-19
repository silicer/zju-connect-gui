package backend

import (
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"sync"
	"time"
)

const pendingConnectFileName = "gui_pending_connect.json"

type PendingConnectStore struct {
	path string
	mu   sync.Mutex
}

type pendingConnectState struct {
	ResumeConnect bool      `json:"resumeConnect"`
	CreatedAt     time.Time `json:"createdAt"`
}

func NewPendingConnectStore(appDir string) *PendingConnectStore {
	return &PendingConnectStore{path: filepath.Join(appDir, pendingConnectFileName)}
}

func (s *PendingConnectStore) MarkResumeConnect() error {
	s.mu.Lock()
	defer s.mu.Unlock()

	data, err := json.Marshal(pendingConnectState{
		ResumeConnect: true,
		CreatedAt:     time.Now().UTC(),
	})
	if err != nil {
		return fmt.Errorf("failed to encode pending connect state: %w", err)
	}

	if err := os.WriteFile(s.path, data, 0o600); err != nil {
		return fmt.Errorf("failed to persist pending connect state: %w", err)
	}

	return nil
}

func (s *PendingConnectStore) HasResumeConnect() (bool, error) {
	s.mu.Lock()
	defer s.mu.Unlock()

	data, err := os.ReadFile(s.path)
	if errors.Is(err, os.ErrNotExist) {
		return false, nil
	}
	if err != nil {
		return false, fmt.Errorf("failed to read pending connect state: %w", err)
	}

	var state pendingConnectState
	if err := json.Unmarshal(data, &state); err != nil {
		return false, fmt.Errorf("failed to parse pending connect state: %w", err)
	}

	return state.ResumeConnect, nil
}

func (s *PendingConnectStore) Clear() error {
	s.mu.Lock()
	defer s.mu.Unlock()

	err := os.Remove(s.path)
	if errors.Is(err, os.ErrNotExist) {
		return nil
	}
	if err != nil {
		return fmt.Errorf("failed to clear pending connect state: %w", err)
	}

	return nil
}

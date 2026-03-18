package main

import (
	"errors"
	"os"
	"path/filepath"
)

func ResolveAppDir() (string, error) {
	exePath, err := os.Executable()
	if err != nil {
		return "", err
	}
	if exePath == "" {
		return "", errors.New("executable path is empty")
	}
	return filepath.Dir(exePath), nil
}

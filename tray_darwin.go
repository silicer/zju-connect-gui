//go:build darwin && !bindings

package main

func startTrayImpl(*App) {}

func quitTrayImpl() {}

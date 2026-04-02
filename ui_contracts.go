package main

type DesktopUI interface {
	ShowWindow()
	HideWindow()
	Quit()
	EmitEvent(event string, payload any)
	OpenFileDialog(title string) (string, error)
}

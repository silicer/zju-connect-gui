package main

func (a *App) startTray() {
	startTrayImpl(a)
}

func quitTray() {
	quitTrayImpl()
}

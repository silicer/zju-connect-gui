package main

import (
	"os"
	"time"

	"zju-connect-gui/internal/backend"

	"github.com/gen2brain/iup-go/iup"
)

const singleInstanceID = "ab3d7e1f-f25e-44ee-9c66-d3f71f33c4d3"

func main() {
	relaunchArgs, err := backend.ParseElevatedRelaunchArgs(os.Args[1:])
	if err != nil {
		println("Error:", err.Error())
		return
	}
	if err := backend.WaitForProcessExit(relaunchArgs.WaitParentPID, 15*time.Second); err != nil {
		println("Error:", err.Error())
		return
	}

	iup.Open()
	defer iup.Close()
	iup.SetGlobal("UTF8MODE", "YES")

	app := NewApp(nil)
	if err := app.startup(); err != nil {
		println("Error:", err.Error())
		return
	}

	ui, err := NewIUPUI(app)
	if err != nil {
		println("Error:", err.Error())
		return
	}
	app.SetUI(ui)
	app.startTray()
	if err := ui.Run(); err != nil {
		println("Error:", err.Error())
	}
}

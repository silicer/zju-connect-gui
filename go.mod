module zju-connect-gui

go 1.23

require (
	github.com/containers/winquit v1.1.0
	github.com/energye/systray v1.0.3
	github.com/gen2brain/iup-go/iup v0.32.0
	golang.org/x/sys v0.30.0
)

replace github.com/gen2brain/iup-go/iup => ./third_party/iup-go/iup

require (
	github.com/godbus/dbus/v5 v5.1.0 // indirect
	github.com/google/uuid v1.6.0 // indirect
	github.com/sirupsen/logrus v1.9.3 // indirect
	github.com/stretchr/testify v1.10.0 // indirect
)

//go:build !bindings && !darwin

package main

import (
	"log"
	"runtime"
	"sync"
	"sync/atomic"

	"github.com/energye/systray"
)

var trayOnce sync.Once
var trayQuitOnce sync.Once
var trayStarted atomic.Bool

func startTrayImpl(a *App) {
	trayOnce.Do(func() {
		trayStarted.Store(true)
		systray.Register(func() {
			if len(trayIconBytes) > 0 {
				systray.SetIcon(trayIconBytes)
			}
			if runtime.GOOS != "windows" {
				systray.SetTitle("zju-connect")
			}
			systray.SetTooltip("zju-connect GUI")

			openEIPItem := systray.AddMenuItem("打开 eip", "打开 EIP 主页")
			systray.AddSeparator()
			restoreItem := systray.AddMenuItem("恢复窗口", "显示主窗口")
			systray.AddSeparator()
			quitItem := systray.AddMenuItem("退出程序", "停止连接并退出程序")

			openEIPItem.Click(func() {
				a.OpenEIP()
			})
			restoreItem.Click(func() {
				a.ShowWindow()
			})
			quitItem.Click(func() {
				a.Quit()
			})
		}, func() {
			log.Printf("tray shutdown")
		})
	})
}

func quitTrayImpl() {
	if !trayStarted.Load() {
		return
	}
	trayQuitOnce.Do(func() {
		systray.Quit()
	})
}

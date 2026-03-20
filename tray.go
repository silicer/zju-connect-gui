package main

import (
	"log"
	"runtime"
	"sync"
	"sync/atomic"

	"github.com/getlantern/systray"
)

var trayOnce sync.Once
var trayQuitOnce sync.Once
var trayStarted atomic.Bool

func (a *App) startTray() {
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

			openEIPItem := systray.AddMenuItem("打开 eip", "用默认浏览器打开 http://eip.scmcc.com.cn/")
			systray.AddSeparator()
			restoreItem := systray.AddMenuItem("恢复窗口", "显示主窗口")
			systray.AddSeparator()
			quitItem := systray.AddMenuItem("退出程序", "停止连接并退出程序")

			go func() {
				for {
					select {
					case <-openEIPItem.ClickedCh:
						a.OpenEIP()
					case <-restoreItem.ClickedCh:
						a.ShowWindow()
					case <-quitItem.ClickedCh:
						a.Quit()
						return
					}
				}
			}()
		}, func() {
			log.Printf("tray shutdown")
		})
	})
}

func quitTray() {
	if !trayStarted.Load() {
		return
	}
	trayQuitOnce.Do(func() {
		systray.Quit()
	})
}

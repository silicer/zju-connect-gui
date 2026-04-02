# IUP-Go Implementation Reference Memo

**Purpose:** Practical patterns for replacing Wails/Vue with native Go UI using IUP-Go bindings.

**Library:** `github.com/gen2brain/iup-go/iup` (v0.32.0) - MIT licensed  
**Official Docs:** https://www.tecgraf.puc-rio.br/iup/  
**Examples:** https://github.com/gen2brain/iup-go/tree/main/examples

---

## 1. Initialization & Lifecycle

### Basic Setup
```go
package main

import "github.com/gen2brain/iup-go/iup"

func main() {
    iup.Open()
    defer iup.Close()
    
    dlg := iup.Dialog(childWidget).SetAttributes(`TITLE="My App", SIZE=400x300`)
    iup.Show(dlg)
    iup.MainLoop()
}
```

### Single Instance Lock
```go
iup.SetGlobal("SINGLEINSTANCE", "MyApp")
if iup.GetGlobal("SINGLEINSTANCE") == "" {
    return // Another instance already running
}
```

---

## 2. Thread-Safe UI Updates (CRITICAL)

**IUP is NOT thread-safe.** Goroutines must not directly update UI.

### Option A: PostMessage (Recommended)
```go
label := iup.Label("Waiting...").SetHandle("statusLabel")
label.SetCallback("POSTMESSAGE_CB", iup.PostMessageFunc(func(ih iup.Ihandle, s string, i int, p any) int {
    ih.SetAttribute("TITLE", s)
    return iup.DEFAULT
}))

// From goroutine:
go func() {
    result := doWork()
    iup.PostMessage(label, result, 0, nil)
}()
```

### Option B: Timer
```go
timer := iup.Timer()
timer.SetAttribute("TIME", "100")  // ms
timer.SetCallback("ACTION_CB", iup.TimerActionFunc(func(ih iup.Ihandle) int {
    if newData := checkForUpdates(); newData != nil {
        updateUI(newData)
    }
    return iup.DEFAULT
}))
timer.SetAttribute("RUN", "YES")
```

### Option C: IdleFunc
```go
iup.SetFunction("IDLE_ACTION", iup.IdleFunc(func() int {
    // Called when idle
    return iup.DEFAULT
}))
// To stop: iup.SetFunction("IDLE_ACTION", nil)
```

---

## 3. Layout Containers

### Vbox / Hbox
```go
vbox := iup.Vbox(
    iup.Label("Title"),
    iup.Text().SetAttribute("EXPAND", "HORIZONTAL"),
    iup.Button("Submit"),
).SetAttributes(`GAP=5, MARGIN=10x10, ALIGNMENT=ACENTER`)

hbox := iup.Hbox(btn1, btn2, iup.Fill()).SetAttribute("GAP", "5")
```

### Tabs
```go
tab1 := iup.Vbox(iup.Label("Content A")).SetAttribute("TABTITLE", "Tab A")
tab2 := iup.Vbox(iup.Label("Content B")).SetAttribute("TABTITLE", "Tab B")

tabs := iup.Tabs(tab1, tab2).SetAttributes(`SHOWCLOSE=YES, ALLOWREORDER=YES`)

tabs.SetCallback("TABCHANGE_CB", iup.TabChangeFunc(func(ih, newChild, oldChild iup.Ihandle) int {
    return iup.DEFAULT
}))
tabs.SetCallback("TABCLOSE_CB", iup.TabCloseFunc(func(ih iup.Ihandle, pos int) int {
    return iup.IGNORE  // Prevent close
}))
```

### Grid Box
```go
grid := iup.GridBox(
    iup.Label("Username:"), iup.Text(),
    iup.Label("Password:"), iup.Text(),
).SetAttributes(`ORIENTATION=HORIZONTAL, NUMDIV=2, GAP=5`)
```

---

## 4. Text Input & Multiline Log

### Single-Line Text
```go
txt := iup.Text().SetAttributes(`EXPAND=HORIZONTAL, VALUE="default"`)

txt.SetCallback("ACTION", iup.TextActionFunc(func(ih iup.Ihandle, c int, newValue string) int {
    // c = char code, return iup.IGNORE to reject
    return iup.DEFAULT
}))
```

### Multiline Log
```go
log := iup.MultiLine().SetAttributes(`
    MULTILINE=YES, EXPAND=YES, READONLY=YES,
    SCROLLBAR=YES, VISIBLELINES=10
`)
log.SetHandle("log")

// Append text (use PostMessage from goroutines):
log.SetAttribute("APPEND", fmt.Sprintf("[%s] %s\n", timestamp, msg))

// Get/Set content:
content := log.GetAttribute("VALUE")
log.SetAttribute("VALUE", "New content")
```

---

## 5. Buttons & Actions

```go
btn := iup.Button("Click Me").SetAttributes(`PADDING=5x5, TIP="Tooltip"`)
btn.SetCallback("ACTION", iup.ActionFunc(func(ih iup.Ihandle) int {
    return iup.DEFAULT
}))

// With image:
img := iup.ImageRGBA(width, height, pixels)
img.SetHandle("iconName")
btn := iup.Button("").SetAttribute("IMAGE", "iconName")
```

---

## 6. Canvas & Mouse Events (Captcha Selection)

### Canvas Setup
```go
canvas := iup.Canvas().SetAttributes(`SIZE=300x300, SCROLLBAR=NO`)
canvas.SetCallback("ACTION", iup.ActionFunc(redrawCallback))
canvas.SetCallback("BUTTON_CB", iup.ButtonFunc(buttonCallback))
canvas.SetCallback("MOTION_CB", iup.MotionFunc(motionCallback))
```

### Drawing
```go
func redrawCallback(ih iup.Ihandle) int {
    iup.DrawBegin(ih)
    defer iup.DrawEnd(ih)
    
    ih.SetAttribute("DRAWCOLOR", "255 0 0")
    ih.SetAttribute("DRAWSTYLE", "FILL")
    
    iup.DrawRectangle(ih, x1, y1, x2, y2)
    iup.DrawArc(ih, x1, y1, x2, y2, angle1, angle2)
    iup.DrawLine(ih, x1, y1, x2, y2)
    iup.DrawText(ih, "text", x, y, -1, -1)
    iup.DrawImage(ih, "imageName", x, y, w, h)
    
    return iup.DEFAULT
}
```

### Mouse Clicks
```go
func buttonCallback(ih iup.Ihandle, button, pressed, x, y int, status string) int {
    // button: 1=left, 2=middle, 3=right
    // pressed: 1=down, 0=up
    // iup.IsDouble(status) checks for double-click
    
    if pressed == 1 && button == 1 {
        fmt.Printf("Clicked at (%d, %d)\n", x, y)
    }
    return iup.DEFAULT
}

func motionCallback(ih iup.Ihandle, x, y int, status string) int {
    // iup.IsShift(status), iup.IsControl(status), iup.IsButton1(status)
    return iup.DEFAULT
}
```

### Load Image
```go
file, _ := os.Open("captcha.png")
img, _, _ := image.Decode(file)
iupImg := iup.ImageFromImage(img)
iupImg.SetHandle("captchaImg")

// In redraw: iup.DrawImage(ih, "captchaImg", 0, 0, w, h)
```

---

## 7. File Dialogs

### Open File
```go
filedlg := iup.FileDlg().SetAttributes(`
    DIALOGTYPE=OPEN, TITLE="Open File",
    EXTFILTER="Image Files|*.png;*.jpg|All Files|*.*"
`)
defer filedlg.Destroy()

iup.Popup(filedlg, iup.CENTER, iup.CENTER)

if filedlg.GetInt("STATUS") >= 0 {
    filename := filedlg.GetAttribute("VALUE")
}
```

### Save File / Directory
```go
// Save: DIALOGTYPE=SAVE, FILTER="*.txt", FILTERINFO="Text Files"
// Dir: DIALOGTYPE=DIR
```

---

## 8. System Tray (Hide-to-Tray)

### Enable Tray
```go
icon := iup.ImageRGBA(16, 16, iconPixels)
icon.SetHandle("trayIcon")

dlg := iup.Dialog(child).SetAttributes(`
    TITLE="My App", TRAY=YES, TRAYIMAGE=trayIcon,
    TRAYTIP="My App - click to restore"
`)
```

### Tray Click Callback
```go
dlg.SetCallback("TRAYCLICK_CB", iup.TrayClickFunc(func(ih iup.Ihandle, but, pressed, dclick int) int {
    if but == 1 && pressed == 0 && dclick == 1 {
        dlg.SetAttribute("STATE", "RESTORE")
        iup.Show(dlg)
    }
    return iup.DEFAULT
}))
```

### Hide to Tray
```go
// Hide without closing main loop:
dlg.SetAttribute("HIDETASKBAR", "YES")

// Alternative with LOCKLOOP:
dlg.SetAttribute("LOCKLOOP", "YES")
iup.Hide(dlg)
// Restore: iup.Show(dlg); dlg.SetAttribute("LOCKLOOP", "NO")
```

### Close Confirmation
```go
dlg.SetCallback("CLOSE_CB", iup.CloseFunc(func(ih iup.Ihandle) int {
    dlg.SetAttribute("HIDETASKBAR", "YES")
    return iup.IGNORE  // Prevent actual close
}))
```

---

## 9. Modal Prompts & Notifications

### Simple Messages
```go
iup.Message("Title", "Message text")
iup.MessageError(parent, "Error message")

switch iup.Alarm("Confirm", "Are you sure?", "Yes", "No", "Cancel") {
case 1: // Yes
case 2: // No
case 3: // Cancel
}
```

### Desktop Notifications
```go
notify := iup.Notify()
notify.SetAttribute("TITLE", "Update Available")
notify.SetAttribute("BODY", "New version ready")
notify.SetCallback("NOTIFY_CB", iup.NotifyFunc(func(ih iup.Ihandle, actionId int) int {
    return iup.DEFAULT
}))
notify.SetAttribute("SHOW", "YES")
```

---

## 10. Dialog Callbacks

```go
// State changes: SHOW, RESTORE, MINIMIZE, MAXIMIZE, HIDE
dlg.SetCallback("SHOW_CB", iup.ShowFunc(func(ih iup.Ihandle, state int) int {
    return iup.DEFAULT
}))

// Resize
dlg.SetCallback("RESIZE_CB", iup.ResizeFunc(func(ih iup.Ihandle, w, h int) int {
    return iup.DEFAULT
}))

// Close button
dlg.SetCallback("CLOSE_CB", iup.CloseFunc(func(ih iup.Ihandle) int {
    return iup.DEFAULT  // Allow close
    // return iup.IGNORE  // Prevent close
}))

// File drop
dlg.SetCallback("DROPFILES_CB", iup.DropFilesFunc(func(ih iup.Ihandle, filename string, num, x, y int) int {
    return iup.DEFAULT
}))

// Theme change
dlg.SetCallback("THEMECHANGED_CB", iup.ThemeChangedFunc(func(ih iup.Ihandle, darkMode int) int {
    return iup.DEFAULT
}))
```

---

## 11. Settings Persistence

```go
config := iup.Config()

// Save:
iup.ConfigSetVariableStr(config, "General", "LastPath", "/home/user")
iup.ConfigSetVariableInt(config, "Window", "Width", 800)
iup.ConfigSave(config)

// Load:
iup.ConfigLoad(config)
path := iup.ConfigGetVariableStr(config, "General", "LastPath")
width := iup.ConfigGetVariableIntDef(config, "Window", "Width", 800)
```

---

## 12. Key Patterns for zju-connect-gui Migration

### Log Streaming from Backend
```go
// Backend goroutine:
go func() {
    scanner := bufio.NewScanner(stdout)
    for scanner.Scan() {
        line := scanner.Text()
        iup.PostMessage(logWidget, line, 0, nil)
    }
}()

// UI thread:
logWidget.SetCallback("POSTMESSAGE_CB", iup.PostMessageFunc(func(ih iup.Ihandle, s string, i int, p any) int {
    ih.SetAttribute("APPEND", s+"\n")
    return iup.DEFAULT
}))
```

### State Machine Pattern
```go
type AppState int
const (
    StateDisconnected AppState = iota
    StateConnecting
    StateConnected
)

var state AppState

func updateState(newState AppState) {
    state = newState
    switch state {
    case StateDisconnected:
        btnConnect.SetAttribute("ACTIVE", "YES")
        statusLabel.SetAttribute("TITLE", "Disconnected")
    case StateConnected:
        btnConnect.SetAttribute("ACTIVE", "NO")
        statusLabel.SetAttribute("TITLE", "Connected")
    }
}
```

### Backend Process Management
```go
var cmd *exec.Cmd

func startBackend() error {
    cmd = exec.Command("./zju-connect", args...)
    stdout, _ := cmd.StdoutPipe()
    cmd.Start()
    
    // Stream logs via PostMessage
    go streamLogs(stdout)
    
    return nil
}

func stopBackend() error {
    if cmd != nil && cmd.Process != nil {
        return cmd.Process.Signal(syscall.SIGTERM)
    }
    return nil
}
```

---

## Quick Reference

| Task | Widget/Function | Key Attributes |
|------|-----------------|----------------|
| Window | `iup.Dialog()` | TITLE, SIZE, MENU, TRAY |
| Button | `iup.Button()` | ACTION callback |
| Text input | `iup.Text()` | VALUE, EXPAND |
| Multiline | `iup.MultiLine()` | APPEND, READONLY |
| Log area | `iup.MultiLine()` | SCROLLBAR, VISIBLELINES |
| Tabs | `iup.Tabs()` | TABTITLE on children |
| Canvas | `iup.Canvas()` | ACTION, BUTTON_CB, MOTION_CB |
| File dialog | `iup.FileDlg()` | DIALOGTYPE, VALUE, STATUS |
| Timer | `iup.Timer()` | TIME, RUN, ACTION_CB |
| Tray | Dialog attrs | TRAY, TRAYIMAGE, TRAYTIP, TRAYCLICK_CB |
| Config | `iup.Config()` | ConfigLoad/Save, ConfigGet/SetVariable* |

---

## Common Return Values

- `iup.DEFAULT` - Continue normal processing
- `iup.IGNORE` - Ignore/cancel the action
- `iup.CLOSE` - Close the dialog/window
- `iup.CONTINUE` - Continue (for some specific callbacks)

---

## Important Notes

1. **Thread Safety**: ALWAYS use PostMessage, Timer, or IdleFunc to update UI from goroutines
2. **Handles**: Use `SetHandle("name")` and `iup.GetHandle("name")` for global widget access
3. **Cleanup**: Call `iup.Destroy()` on dialogs/widgets when done
4. **Modal vs Modeless**: `iup.Popup()` for modal, `iup.Show()` for modeless
5. **Platform Differences**: Some features (tray, notifications) are platform-specific
6. **UTF-8**: Set `iup.SetGlobal("UTF8MODE", "YES")` for proper text encoding

---

## Build Commands

```bash
# Development
go run main.go

# Build (Windows)
go build -ldflags "-H=windowsgui" -o app.exe

# With build tags for extra features
go build -tags "gl web plot ctrl"
```

package main

import (
	"bytes"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"image"
	_ "image/jpeg"
	_ "image/png"
	"math"
	"reflect"
	stdRuntime "runtime"
	"strconv"
	"strings"
	"sync"

	"zju-connect-gui/internal/backend"

	"github.com/gen2brain/iup-go/iup"
)

const (
	uiEventTimerMs  = 50
	autosaveTimerMs = 250
	maxLogEntries   = 1000
	captchaHandle   = "zju_connect_gui_captcha"

	captchaPreviewWidth  = 720
	captchaPreviewHeight = 420

	inputDialogGenericRasterSize = "420x250"
	inputDialogGenericMinSize    = "420x220"
	inputDialogSMSRasterSize     = "420x140"
	inputDialogSMSMinSize        = "420x140"
)

type captchaPoint struct {
	X int
	Y int
}

type captchaPreviewRect struct {
	X      int
	Y      int
	Width  int
	Height int
}

type inputDialogMode string

const (
	inputDialogModeGeneric inputDialogMode = "generic"
	inputDialogModeSMS     inputDialogMode = "sms"
)

type iupUI struct {
	app *App

	actions chan func()

	dialog        iup.Ihandle
	eventTimer    iup.Ihandle
	autosaveTimer iup.Ihandle

	statusLabel      iup.Ihandle
	logArea          iup.Ihandle
	autoScrollToggle iup.Ihandle
	startStopButton  iup.Ihandle
	appIcon          iup.Ihandle

	usernameInput     iup.Ihandle
	passwordInput     iup.Ihandle
	socksBindInput    iup.Ihandle
	httpBindInput     iup.Ihandle
	proxyOnlyToggle   iup.Ihandle
	debugDumpToggle   iup.Ihandle
	eipProgramInput   iup.Ihandle
	eipArgsInput      iup.Ihandle
	eipAutoOpenToggle iup.Ihandle

	inputDialog      iup.Ihandle
	inputPromptLabel iup.Ihandle
	inputText        iup.Ihandle
	inputMultiline   iup.Ihandle
	inputSwitcher    iup.Ihandle
	inputSubmit      iup.Ihandle
	inputMode        inputDialogMode

	captchaDialog       iup.Ihandle
	captchaPromptLabel  iup.Ihandle
	captchaCanvas       iup.Ihandle
	captchaPointsLabel  iup.Ihandle
	captchaPoints       []captchaPoint
	captchaImage        image.Image
	captchaImageWidth   int
	captchaImageHeight  int
	captchaImageHandle  iup.Ihandle
	hasCaptchaHandle    bool
	captchaImageHandleM sync.Mutex

	logs        []string
	logValueLen int

	pendingLogM     sync.Mutex
	pendingLogLines []string

	running    bool
	lastSaved  LaunchOptions
	hasSaved   bool
	inputReady bool
}

func NewIUPUI(app *App) (*iupUI, error) {
	ui := &iupUI{
		app:     app,
		actions: make(chan func(), 512),
	}
	if err := ui.build(); err != nil {
		return nil, err
	}
	return ui, nil
}

func (ui *iupUI) Run() error {
	ui.loadInitialState()
	ui.showMainDialog()
	for {
		iup.MainLoop()
		if ui.app.canClose() {
			ui.app.shutdown()
			return nil
		}
	}
}

func (ui *iupUI) ShowWindow() {
	ui.enqueue(func() {
		ui.showMainDialog()
	})
}

func (ui *iupUI) HideWindow() {
	ui.enqueue(func() {
		ui.hideMainDialogToTray()
	})
}

func (ui *iupUI) Quit() {
	ui.enqueue(func() {
		iup.ExitLoop()
	})
}

func (ui *iupUI) EmitEvent(event string, payload any) {
	if event == "log" {
		line, _ := payload.(string)
		ui.bufferLogLine(strings.TrimSpace(line))
		return
	}
	ui.enqueue(func() {
		ui.handleEvent(event, payload)
	})
}

func (ui *iupUI) OpenFileDialog(title string) (string, error) {
	filedlg := iup.FileDlg().SetAttributes(fmt.Sprintf(`DIALOGTYPE=OPEN,TITLE="%s"`, title))
	defer filedlg.Destroy()
	iup.Popup(filedlg, iup.CENTER, iup.CENTER)
	if filedlg.GetInt("STATUS") < 0 {
		return "", nil
	}
	return filedlg.GetAttribute("VALUE"), nil
}

func (ui *iupUI) enqueue(fn func()) {
	select {
	case ui.actions <- fn:
	default:
		go func() { ui.actions <- fn }()
	}
}

func (ui *iupUI) build() error {
	icon, err := loadApplicationIcon()
	if err != nil {
		return fmt.Errorf("load application icon: %w", err)
	}
	ui.appIcon = icon
	iup.SetGlobal("ICON", icon)

	ui.statusLabel = iup.Label("").SetAttributes(`EXPAND=HORIZONTAL, PADDING=8x6`)
	ui.usernameInput = iup.Text().SetAttributes(`EXPAND=HORIZONTAL`)
	ui.passwordInput = iup.Text().SetAttributes(`EXPAND=HORIZONTAL, PASSWORD=YES`)
	ui.socksBindInput = iup.Text().SetAttributes(`EXPAND=HORIZONTAL`)
	ui.httpBindInput = iup.Text().SetAttributes(`EXPAND=HORIZONTAL`)
	ui.proxyOnlyToggle = iup.Toggle("仅代理模式")
	ui.debugDumpToggle = iup.Toggle("调试模式")
	ui.eipProgramInput = iup.Text().SetAttributes(`EXPAND=HORIZONTAL`)
	ui.eipArgsInput = iup.MultiLine().SetAttributes(`EXPAND=HORIZONTAL, VISIBLELINES=4`)
	ui.eipAutoOpenToggle = iup.Toggle("连接后打开浏览器")
	ui.autoScrollToggle = iup.Toggle("自动滚动")
	ui.autoScrollToggle.SetAttribute("VALUE", "ON")
	ui.logArea = iup.MultiLine().SetAttributes(`EXPAND=YES, READONLY=YES, MULTILINE=YES, VISIBLELINES=18, VISIBLECOLUMNS=80, FONT="Monospace, 10"`)
	ui.startStopButton = iup.Button("开始连接")
	ui.startStopButton.SetAttributes(`PADDING=24x12, FONTSTYLE=BOLD`)
	ui.startStopButton.SetCallback("ACTION", iup.ActionFunc(func(iup.Ihandle) int {
		ui.handleStartStop()
		return iup.DEFAULT
	}))

	browseButton := iup.Button("浏览...")
	browseButton.SetCallback("ACTION", iup.ActionFunc(func(iup.Ihandle) int {
		path, err := ui.OpenFileDialog("选择浏览器程序")
		if err != nil {
			ui.setStatus(fmt.Sprintf("选择浏览器程序失败：%v", err))
			return iup.DEFAULT
		}
		if strings.TrimSpace(path) != "" {
			ui.eipProgramInput.SetAttribute("VALUE", path)
		}
		return iup.DEFAULT
	}))

	clearButton := iup.Button("清空")
	clearButton.SetCallback("ACTION", iup.ActionFunc(func(iup.Ihandle) int {
		ui.eipProgramInput.SetAttribute("VALUE", "")
		return iup.DEFAULT
	}))

	clearLogsButton := iup.Button("清空日志")
	clearLogsButton.SetCallback("ACTION", iup.ActionFunc(func(iup.Ihandle) int {
		ui.clearLogs()
		return iup.DEFAULT
	}))

	labeledField := func(title string, field iup.Ihandle) iup.Ihandle {
		return iup.Vbox(
			iup.Label(title),
			field,
		).SetAttributes(`GAP=4, EXPAND=HORIZONTAL`)
	}

	proxyOnlyHelp := iup.Label("开启后，仅提供本机 SOCKS 和 HTTP 代理端口。\n关闭后，还会启用 TUN / 系统路由接管。").SetAttributes(`EXPAND=HORIZONTAL, PADDING=18x0`)
	proxyOnlyField := iup.Vbox(
		ui.proxyOnlyToggle,
		proxyOnlyHelp,
	).SetAttributes(`GAP=2, EXPAND=HORIZONTAL`)

	configFields := iup.Vbox(
		labeledField("用户名", ui.usernameInput),
		labeledField("密码", ui.passwordInput),
		labeledField("SOCKS 监听地址", ui.socksBindInput),
		labeledField("HTTP 监听地址", ui.httpBindInput),
		labeledField("浏览器程序路径", iup.Hbox(ui.eipProgramInput, browseButton, clearButton).SetAttributes(`GAP=6, EXPAND=HORIZONTAL`)),
		labeledField("浏览器参数（每行一个）", ui.eipArgsInput),
		ui.eipAutoOpenToggle,
		proxyOnlyField,
		ui.debugDumpToggle,
	).SetAttributes(`GAP=10, EXPAND=HORIZONTAL`)

	configBody := iup.Vbox(
		iup.Label("按需填写账号、本地代理监听地址和 EIP 打开方式即可。"),
		configFields,
	).SetAttributes(`GAP=10, EXPAND=HORIZONTAL, MARGIN=10x10`)

	configTab := iup.Vbox(
		iup.ScrollBox(configBody).SetAttribute("EXPAND", "YES"),
		iup.Hbox(iup.Fill(), ui.startStopButton).SetAttributes(`GAP=6, MARGIN=10x0`),
	).SetAttributes(`TABTITLE="配置", GAP=8, MARGIN=0x0, EXPAND=YES`)

	logsTab := iup.Vbox(
		iup.Hbox(ui.autoScrollToggle, clearLogsButton, iup.Fill()).SetAttribute("GAP", "6"),
		ui.logArea,
	).SetAttributes(`TABTITLE="日志", GAP=8, MARGIN=10x10`)

	tabs := iup.Tabs(configTab, logsTab).SetAttributes(`EXPAND=YES`)

	root := iup.Vbox(
		ui.statusLabel,
		tabs,
	).SetAttributes(`GAP=6, MARGIN=10x10`)

	ui.dialog = iup.Dialog(root).SetAttributes(`TITLE="ZJU Connect GUI", RASTERSIZE=1248x1160, MINSIZE=1248x1160`)
	iup.SetAttributeHandle(ui.dialog, "ICON", ui.appIcon)
	ui.dialog.SetCallback("CLOSE_CB", iup.CloseFunc(func(iup.Ihandle) int {
		if stdRuntime.GOOS == "darwin" {
			go ui.app.Quit()
			return iup.IGNORE
		}
		ui.hideMainDialogToTray()
		return iup.IGNORE
	}))

	ui.buildInputDialog()
	ui.buildCaptchaDialog()

	ui.eventTimer = iup.Timer().SetAttribute("TIME", strconv.Itoa(uiEventTimerMs))
	ui.eventTimer.SetCallback("ACTION_CB", iup.TimerActionFunc(func(iup.Ihandle) int {
		for {
			select {
			case fn := <-ui.actions:
				fn()
			default:
				ui.flushPendingLogs()
				return iup.DEFAULT
			}
		}
	}))
	ui.eventTimer.SetAttribute("RUN", "YES")

	ui.autosaveTimer = iup.Timer().SetAttribute("TIME", strconv.Itoa(autosaveTimerMs))
	ui.autosaveTimer.SetCallback("ACTION_CB", iup.TimerActionFunc(func(iup.Ihandle) int {
		ui.autosaveIfNeeded()
		return iup.DEFAULT
	}))
	ui.autosaveTimer.SetAttribute("RUN", "YES")
	return nil
}

func (ui *iupUI) buildInputDialog() {
	ui.inputPromptLabel = iup.Label("请输入内容")
	ui.inputText = iup.Text().SetAttributes(`EXPAND=HORIZONTAL`)
	ui.inputMultiline = iup.MultiLine().SetAttributes(`VISIBLELINES=6, EXPAND=YES`)
	ui.inputSwitcher = iup.Zbox(ui.inputText, ui.inputMultiline).SetAttributes(`CHILDSIZEALL=NO, EXPAND=YES`)
	cancel := iup.Button("取消")
	cancel.SetCallback("ACTION", iup.ActionFunc(func(iup.Ihandle) int {
		iup.Hide(ui.inputDialog)
		return iup.DEFAULT
	}))
	ui.inputSubmit = iup.Button("提交")
	ui.inputSubmit.SetCallback("ACTION", iup.ActionFunc(func(iup.Ihandle) int {
		ui.submitInputDialog()
		return iup.DEFAULT
	}))
	body := iup.Vbox(
		ui.inputPromptLabel,
		ui.inputSwitcher,
		iup.Hbox(iup.Fill(), cancel, ui.inputSubmit).SetAttribute("GAP", "6"),
	).SetAttributes(`GAP=8, MARGIN=10x10`)
	ui.inputDialog = iup.Dialog(body).SetAttributes(fmt.Sprintf(`TITLE="输入需求", RASTERSIZE=%s, MINSIZE=%s`, inputDialogGenericRasterSize, inputDialogGenericMinSize))
	if ui.appIcon != 0 {
		iup.SetAttributeHandle(ui.inputDialog, "ICON", ui.appIcon)
	}
	iup.SetAttributeHandle(ui.inputDialog, "DEFAULTESC", cancel)
	ui.applyInputDialogLayout(inputDialogModeGeneric)
	ui.inputDialog.SetCallback("CLOSE_CB", iup.CloseFunc(func(iup.Ihandle) int {
		iup.Hide(ui.inputDialog)
		return iup.IGNORE
	}))
}

func (ui *iupUI) buildCaptchaDialog() {
	ui.captchaPromptLabel = iup.Label("请在图片上按顺序点击对应位置，然后提交").SetAttribute("EXPAND", "HORIZONTAL")
	ui.captchaCanvas = iup.Canvas().SetAttributes(fmt.Sprintf(`RASTERSIZE=%dx%d`, captchaPreviewWidth, captchaPreviewHeight))
	ui.captchaCanvas.SetCallback("ACTION", iup.CanvasActionFunc(func(ih iup.Ihandle, _, _ float64) int {
		ui.drawCaptcha(ih)
		return iup.DEFAULT
	}))
	ui.captchaCanvas.SetCallback("BUTTON_CB", iup.ButtonFunc(func(ih iup.Ihandle, button, pressed, x, y int, _ string) int {
		if button == iup.BUTTON1 && pressed == 1 && ui.captchaImage != nil {
			point, ok := ui.mapCaptchaClickToNatural(ih, x, y)
			if !ok {
				return iup.DEFAULT
			}
			ui.captchaPoints = append(ui.captchaPoints, point)
			ui.updateCaptchaPointsLabel()
			iup.Refresh(ih)
		}
		return iup.DEFAULT
	}))
	ui.captchaPointsLabel = iup.Label("尚未选择坐标").SetAttributes(`EXPAND=HORIZONTAL, PADDING=4x4`)
	undo := iup.Button("撤销上一步")
	undo.SetCallback("ACTION", iup.ActionFunc(func(iup.Ihandle) int {
		if len(ui.captchaPoints) > 0 {
			ui.captchaPoints = ui.captchaPoints[:len(ui.captchaPoints)-1]
			ui.updateCaptchaPointsLabel()
			iup.Refresh(ui.captchaCanvas)
		}
		return iup.DEFAULT
	}))
	clear := iup.Button("清空坐标")
	clear.SetCallback("ACTION", iup.ActionFunc(func(iup.Ihandle) int {
		ui.captchaPoints = nil
		ui.updateCaptchaPointsLabel()
		iup.Refresh(ui.captchaCanvas)
		return iup.DEFAULT
	}))
	cancel := iup.Button("取消")
	cancel.SetCallback("ACTION", iup.ActionFunc(func(iup.Ihandle) int {
		iup.Hide(ui.captchaDialog)
		return iup.DEFAULT
	}))
	submit := iup.Button("提交")
	submit.SetCallback("ACTION", iup.ActionFunc(func(iup.Ihandle) int {
		ui.submitCaptcha()
		return iup.DEFAULT
	}))
	body := iup.Vbox(
		ui.captchaPromptLabel,
		iup.Frame(ui.captchaCanvas).SetAttribute("TITLE", "点击图片选择坐标"),
		ui.captchaPointsLabel,
		iup.Hbox(undo, clear, iup.Fill(), cancel, submit).SetAttribute("GAP", "6"),
	).SetAttributes(`GAP=8, MARGIN=10x10`)
	ui.captchaDialog = iup.Dialog(body).SetAttributes(`TITLE="图形验证码", RASTERSIZE=800x720, MINSIZE=760x680`)
	if ui.appIcon != 0 {
		iup.SetAttributeHandle(ui.captchaDialog, "ICON", ui.appIcon)
	}
	ui.captchaDialog.SetCallback("CLOSE_CB", iup.CloseFunc(func(iup.Ihandle) int {
		iup.Hide(ui.captchaDialog)
		return iup.IGNORE
	}))
}

func (ui *iupUI) loadInitialState() {
	defaults := launchOptionsFromBackend(backend.DefaultLaunchOptions())
	ui.applyOptions(defaults)
	ui.lastSaved = defaults
	ui.hasSaved = true
	ui.inputReady = true

	if running := ui.app.IsRunning(); running {
		ui.setRunning(true)
	}
	if saved, err := ui.app.GetSavedLaunchOptions(); err == nil {
		ui.applyOptions(saved)
		ui.lastSaved = saved
	}
	if resumed, err := ui.app.ResumePendingConnect(); err != nil {
		ui.setStatus(err.Error())
	} else if resumed {
		ui.setStatus("已切换到管理员模式，正在恢复连接...")
	}
}

func (ui *iupUI) handleStartStop() {
	if ui.running {
		if err := ui.app.Stop(); err != nil {
			ui.setStatus(err.Error())
			return
		}
		ui.setStatus("已发送停止信号")
		return
	}
	options := ui.currentOptions()
	if strings.TrimSpace(options.Username) == "" || strings.TrimSpace(options.Password) == "" {
		ui.setStatus("用户名和密码不能为空")
		return
	}
	if err := ui.app.Start(options); err != nil {
		ui.setStatus(err.Error())
		return
	}
	ui.lastSaved = options
	ui.hasSaved = true
	ui.setStatus("正在启动...")
}

func (ui *iupUI) currentOptions() LaunchOptions {
	options := launchOptionsFromBackend(backend.DefaultLaunchOptions())
	options.Username = strings.TrimSpace(ui.usernameInput.GetAttribute("VALUE"))
	options.Password = ui.passwordInput.GetAttribute("VALUE")
	options.SocksBind = strings.TrimSpace(ui.socksBindInput.GetAttribute("VALUE"))
	options.HTTPBind = strings.TrimSpace(ui.httpBindInput.GetAttribute("VALUE"))
	options.EIPBrowserProgram = strings.TrimSpace(ui.eipProgramInput.GetAttribute("VALUE"))
	options.EIPBrowserArgs = parseEIPBrowserArgs(ui.eipArgsInput.GetAttribute("VALUE"))
	options.EIPAutoOpen = ui.eipAutoOpenToggle.GetAttribute("VALUE") == "ON"
	options.TunMode = ui.proxyOnlyToggle.GetAttribute("VALUE") != "ON"
	options.DebugDump = ui.debugDumpToggle.GetAttribute("VALUE") == "ON"
	return options
}

func (ui *iupUI) applyOptions(options LaunchOptions) {
	ui.usernameInput.SetAttribute("VALUE", options.Username)
	ui.passwordInput.SetAttribute("VALUE", options.Password)
	ui.socksBindInput.SetAttribute("VALUE", options.SocksBind)
	ui.httpBindInput.SetAttribute("VALUE", options.HTTPBind)
	ui.eipProgramInput.SetAttribute("VALUE", options.EIPBrowserProgram)
	ui.eipArgsInput.SetAttribute("VALUE", strings.Join(options.EIPBrowserArgs, "\n"))
	if options.EIPAutoOpen {
		ui.eipAutoOpenToggle.SetAttribute("VALUE", "ON")
	} else {
		ui.eipAutoOpenToggle.SetAttribute("VALUE", "OFF")
	}
	if options.TunMode {
		ui.proxyOnlyToggle.SetAttribute("VALUE", "OFF")
	} else {
		ui.proxyOnlyToggle.SetAttribute("VALUE", "ON")
	}
	if options.DebugDump {
		ui.debugDumpToggle.SetAttribute("VALUE", "ON")
	} else {
		ui.debugDumpToggle.SetAttribute("VALUE", "OFF")
	}
}

func (ui *iupUI) autosaveIfNeeded() {
	if !ui.inputReady {
		return
	}
	current := ui.currentOptions()
	if ui.hasSaved && reflect.DeepEqual(current, ui.lastSaved) {
		return
	}
	if err := ui.app.SaveLaunchOptions(current); err == nil {
		ui.lastSaved = current
		ui.hasSaved = true
	}
}

func (ui *iupUI) handleEvent(event string, payload any) {
	switch event {
	case "log":
		line, _ := payload.(string)
		ui.bufferLogLine(strings.TrimSpace(line))
	case "error":
		message, _ := payload.(string)
		ui.setStatus(message)
	case "state":
		ui.handleStatePayload(payload)
	case "need-input":
		ui.handleInputPayload(payload)
	case "need-captcha":
		ui.handleCaptchaPayload(payload)
	}
}

func (ui *iupUI) handleStatePayload(payload any) {
	status, ok := payload.(map[string]any)
	if !ok {
		return
	}
	if running, ok := status["running"].(bool); ok {
		ui.setRunning(running)
	}
	if message, ok := status["message"].(string); ok && strings.TrimSpace(message) != "" {
		ui.setStatus(message)
		return
	}
	if awaiting, ok := status["awaiting"].(string); ok && strings.TrimSpace(awaiting) != "" {
		ui.setStatus("等待输入: " + awaiting)
		return
	}
	if state, ok := status["state"].(string); ok && state == "stopped" && !ui.running {
		ui.setStatus("已断开")
	}
}

func (ui *iupUI) handleInputPayload(payload any) {
	data, ok := payload.(map[string]any)
	if !ok {
		return
	}
	prompt, _ := data["prompt"].(string)
	kind, _ := data["type"].(string)
	if prompt == "" {
		prompt = "请输入内容"
	}
	if kind == "sms" {
		ui.inputMode = inputDialogModeSMS
		ui.inputDialog.SetAttribute("TITLE", "短信验证码")
		ui.inputText.SetAttribute("VALUE", "")
	} else {
		ui.inputMode = inputDialogModeGeneric
		ui.inputDialog.SetAttribute("TITLE", "输入需求")
		ui.inputMultiline.SetAttribute("VALUE", "")
	}
	ui.inputPromptLabel.SetAttribute("TITLE", prompt)
	ui.applyInputDialogLayout(ui.inputMode)
	iup.Show(ui.inputDialog)
	iup.RefreshChildren(ui.inputDialog)
	iup.Refresh(ui.inputDialog)
	if ui.inputMode == inputDialogModeSMS {
		iup.SetFocus(ui.inputText)
	} else {
		iup.SetFocus(ui.inputMultiline)
	}
}

func (ui *iupUI) handleCaptchaPayload(payload any) {
	data, ok := payload.(map[string]any)
	if !ok {
		return
	}
	encoded, _ := data["base64"].(string)
	if encoded == "" {
		return
	}
	decoded, err := base64.StdEncoding.DecodeString(encoded)
	if err != nil {
		ui.setStatus(fmt.Sprintf("解析验证码失败：%v", err))
		return
	}
	img, _, err := image.Decode(bytes.NewReader(decoded))
	if err != nil {
		ui.setStatus(fmt.Sprintf("加载验证码失败：%v", err))
		return
	}
	ui.captchaImage = img
	bounds := img.Bounds()
	ui.captchaImageWidth = bounds.Dx()
	ui.captchaImageHeight = bounds.Dy()
	ui.captchaPoints = nil
	ui.updateCaptchaPointsLabel()
	ui.registerCaptchaHandle(img)
	iup.Show(ui.captchaDialog)
	iup.Refresh(ui.captchaCanvas)
}

func (ui *iupUI) registerCaptchaHandle(img image.Image) {
	ui.captchaImageHandleM.Lock()
	defer ui.captchaImageHandleM.Unlock()
	if ui.hasCaptchaHandle {
		ui.captchaImageHandle.Destroy()
		ui.hasCaptchaHandle = false
	}
	ui.captchaImageHandle = iup.ImageFromImage(img)
	ui.captchaImageHandle.SetHandle(captchaHandle)
	ui.hasCaptchaHandle = true
}

func (ui *iupUI) drawCaptcha(ih iup.Ihandle) {
	iup.DrawBegin(ih)
	defer iup.DrawEnd(ih)
	preview := ui.captchaPreviewRect(ih)
	if ui.captchaImage != nil && preview.Width > 0 && preview.Height > 0 {
		iup.DrawImage(ih, captchaHandle, preview.X, preview.Y, preview.Width, preview.Height)
	}
	for idx, point := range ui.captchaPoints {
		x, y, ok := ui.mapNaturalPointToPreview(ih, point)
		if !ok {
			continue
		}
		iup.DrawArc(ih, x-8, y-8, x+8, y+8, 0, 360)
		iup.DrawText(ih, strconv.Itoa(idx+1), x+10, y-10, -1, -1)
	}
}

func (ui *iupUI) submitInputDialog() {
	var value string
	if ui.inputMode == inputDialogModeSMS {
		value = strings.TrimSpace(ui.inputText.GetAttribute("VALUE"))
	} else {
		value = strings.TrimSpace(ui.inputMultiline.GetAttribute("VALUE"))
	}
	if value == "" {
		ui.setStatus("输入不能为空")
		return
	}
	if err := ui.app.SubmitInput(value); err != nil {
		ui.setStatus(err.Error())
		return
	}
	iup.Hide(ui.inputDialog)
}

func (ui *iupUI) submitCaptcha() {
	if len(ui.captchaPoints) == 0 {
		ui.setStatus("请先选择验证码坐标")
		return
	}
	if ui.captchaImageWidth <= 0 || ui.captchaImageHeight <= 0 {
		ui.setStatus("验证码图片尚未准备好")
		return
	}
	coords := make([][2]int, 0, len(ui.captchaPoints))
	for _, point := range ui.captchaPoints {
		coords = append(coords, [2]int{point.X, point.Y})
	}
	payload := struct {
		Coordinates [][2]int `json:"coordinates"`
		Width       int      `json:"width"`
		Height      int      `json:"height"`
	}{
		Coordinates: coords,
		Width:       ui.captchaImageWidth,
		Height:      ui.captchaImageHeight,
	}
	encoded, err := json.Marshal(payload)
	if err != nil {
		ui.setStatus(err.Error())
		return
	}
	if err := ui.app.SubmitInput(string(encoded)); err != nil {
		ui.setStatus(err.Error())
		return
	}
	iup.Hide(ui.captchaDialog)
}

func (ui *iupUI) updateCaptchaPointsLabel() {
	if len(ui.captchaPoints) == 0 {
		ui.captchaPointsLabel.SetAttribute("TITLE", "尚未选择坐标")
		return
	}
	coords := make([][2]int, 0, len(ui.captchaPoints))
	for _, point := range ui.captchaPoints {
		coords = append(coords, [2]int{point.X, point.Y})
	}
	ui.captchaPointsLabel.SetAttribute("TITLE", string(mustJSON(coords)))
}

func (ui *iupUI) appendLog(line string) {
	ui.appendLogs([]string{line})
}

func (ui *iupUI) appendLogs(lines []string) {
	if len(lines) == 0 {
		return
	}
	filtered := lines[:0]
	for _, line := range lines {
		if line != "" {
			filtered = append(filtered, line)
		}
	}
	if len(filtered) == 0 {
		return
	}
	hadLogs := len(ui.logs) > 0
	ui.logs = append(ui.logs, filtered...)
	if len(ui.logs) > maxLogEntries {
		ui.logs = append([]string(nil), ui.logs[len(ui.logs)-maxLogEntries:]...)
		ui.renderLogs()
		return
	}
	ui.appendLogsToWidget(filtered, hadLogs)
}

func (ui *iupUI) renderLogs() {
	value := strings.Join(ui.logs, "\n")
	ui.logArea.SetAttribute("VALUE", value)
	ui.logValueLen = len(value)
	if ui.autoScrollToggle.GetAttribute("VALUE") == "ON" {
		ui.logArea.SetAttribute("CARETPOS", strconv.Itoa(ui.logValueLen))
	}
}

func (ui *iupUI) appendLogsToWidget(lines []string, hadLogs bool) {
	if len(lines) == 0 {
		return
	}
	appended := strings.Join(lines, "\n")
	if appended == "" {
		return
	}
	if hadLogs {
		appended = "\n" + appended
	}
	ui.logArea.SetAttribute("APPEND", appended)
	ui.logValueLen += len(appended)
	if ui.autoScrollToggle.GetAttribute("VALUE") == "ON" {
		ui.logArea.SetAttribute("CARETPOS", strconv.Itoa(ui.logValueLen))
	}
}

func (ui *iupUI) clearLogs() {
	ui.pendingLogM.Lock()
	ui.pendingLogLines = nil
	ui.pendingLogM.Unlock()
	ui.logs = nil
	ui.logValueLen = 0
	ui.renderLogs()
}

func (ui *iupUI) bufferLogLine(line string) {
	if line == "" {
		return
	}
	ui.pendingLogM.Lock()
	ui.pendingLogLines = append(ui.pendingLogLines, line)
	ui.pendingLogM.Unlock()
}

func (ui *iupUI) flushPendingLogs() {
	ui.pendingLogM.Lock()
	if len(ui.pendingLogLines) == 0 {
		ui.pendingLogM.Unlock()
		return
	}
	lines := append([]string(nil), ui.pendingLogLines...)
	ui.pendingLogLines = nil
	ui.pendingLogM.Unlock()
	ui.appendLogs(lines)
}

func (ui *iupUI) applyInputDialogLayout(mode inputDialogMode) {
	if mode == inputDialogModeSMS {
		iup.SetAttributeHandle(ui.inputSwitcher, "VALUE_HANDLE", ui.inputText)
		iup.SetAttributeHandle(ui.inputDialog, "DEFAULTENTER", ui.inputSubmit)
		ui.inputDialog.SetAttribute("RASTERSIZE", inputDialogSMSRasterSize)
		ui.inputDialog.SetAttribute("MINSIZE", inputDialogSMSMinSize)
	} else {
		iup.SetAttributeHandle(ui.inputSwitcher, "VALUE_HANDLE", ui.inputMultiline)
		ui.inputDialog.SetAttribute("DEFAULTENTER", nil)
		ui.inputDialog.SetAttribute("RASTERSIZE", inputDialogGenericRasterSize)
		ui.inputDialog.SetAttribute("MINSIZE", inputDialogGenericMinSize)
	}
	iup.RefreshChildren(ui.inputDialog)
}

func (ui *iupUI) showMainDialog() {
	ui.dialog.SetAttribute("LOCKLOOP", "NO")
	ui.dialog.SetAttribute("HIDETASKBAR", "NO")
	ui.dialog.SetAttribute("STATE", "RESTORE")
	iup.Show(ui.dialog)
}

func (ui *iupUI) hideMainDialogToTray() {
	ui.dialog.SetAttribute("HIDETASKBAR", "YES")
	ui.dialog.SetAttribute("LOCKLOOP", "YES")
	iup.Hide(ui.dialog)
}

func (ui *iupUI) captchaPreviewRect(ih iup.Ihandle) captchaPreviewRect {
	if ui.captchaImageWidth <= 0 || ui.captchaImageHeight <= 0 {
		return captchaPreviewRect{}
	}
	canvasWidth, canvasHeight := ui.captchaCanvasSize(ih)
	if canvasWidth <= 0 || canvasHeight <= 0 {
		return captchaPreviewRect{}
	}
	scale := math.Min(
		float64(canvasWidth)/float64(ui.captchaImageWidth),
		float64(canvasHeight)/float64(ui.captchaImageHeight),
	)
	if scale <= 0 {
		return captchaPreviewRect{}
	}
	width := maxInt(1, int(math.Round(float64(ui.captchaImageWidth)*scale)))
	height := maxInt(1, int(math.Round(float64(ui.captchaImageHeight)*scale)))
	return captchaPreviewRect{
		X:      (canvasWidth - width) / 2,
		Y:      (canvasHeight - height) / 2,
		Width:  width,
		Height: height,
	}
}

func (ui *iupUI) captchaCanvasSize(ih iup.Ihandle) (int, int) {
	for _, attr := range []string{"DRAWSIZE", "RASTERSIZE"} {
		if count, width, height := iup.GetInt2(ih, attr); count == 2 && width > 0 && height > 0 {
			return width, height
		}
	}
	return captchaPreviewWidth, captchaPreviewHeight
}

func (ui *iupUI) mapCaptchaClickToNatural(ih iup.Ihandle, x, y int) (captchaPoint, bool) {
	preview := ui.captchaPreviewRect(ih)
	if preview.Width <= 0 || preview.Height <= 0 {
		return captchaPoint{}, false
	}
	if x < preview.X || x >= preview.X+preview.Width || y < preview.Y || y >= preview.Y+preview.Height {
		return captchaPoint{}, false
	}
	relX := x - preview.X
	relY := y - preview.Y
	return captchaPoint{
		X: mapScaledCaptchaCoordinate(relX, preview.Width, ui.captchaImageWidth),
		Y: mapScaledCaptchaCoordinate(relY, preview.Height, ui.captchaImageHeight),
	}, true
}

func (ui *iupUI) mapNaturalPointToPreview(ih iup.Ihandle, point captchaPoint) (int, int, bool) {
	preview := ui.captchaPreviewRect(ih)
	if preview.Width <= 0 || preview.Height <= 0 {
		return 0, 0, false
	}
	return preview.X + scaleCaptchaCoordinate(point.X, ui.captchaImageWidth, preview.Width), preview.Y + scaleCaptchaCoordinate(point.Y, ui.captchaImageHeight, preview.Height), true
}

func (ui *iupUI) setRunning(running bool) {
	ui.running = running
	if running {
		ui.startStopButton.SetAttribute("TITLE", "断开连接")
	} else {
		ui.startStopButton.SetAttribute("TITLE", "开始连接")
	}
}

func (ui *iupUI) setStatus(message string) {
	ui.statusLabel.SetAttribute("TITLE", strings.TrimSpace(message))
}

func parseEIPBrowserArgs(raw string) []string {
	lines := strings.Split(raw, "\n")
	args := make([]string, 0, len(lines))
	for _, line := range lines {
		trimmed := strings.TrimSpace(line)
		if trimmed != "" {
			args = append(args, trimmed)
		}
	}
	return args
}

func mustJSON(v any) []byte {
	data, err := json.Marshal(v)
	if err != nil {
		return []byte("[]")
	}
	return data
}

func scaleCaptchaCoordinate(value, naturalSize, previewSize int) int {
	if naturalSize <= 1 || previewSize <= 1 {
		return 0
	}
	return int(math.Round((float64(value) / float64(naturalSize-1)) * float64(previewSize-1)))
}

func mapScaledCaptchaCoordinate(value, previewSize, naturalSize int) int {
	if previewSize <= 1 || naturalSize <= 1 {
		return 0
	}
	return clampInt(int(math.Round((float64(value)/float64(previewSize-1))*float64(naturalSize-1))), 0, naturalSize-1)
}

func clampInt(value, minValue, maxValue int) int {
	if value < minValue {
		return minValue
	}
	if value > maxValue {
		return maxValue
	}
	return value
}

func maxInt(a, b int) int {
	if a > b {
		return a
	}
	return b
}

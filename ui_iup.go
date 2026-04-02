package main

import (
	"bytes"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"image"
	_ "image/jpeg"
	_ "image/png"
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
)

type captchaPoint struct {
	X int
	Y int
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

	usernameInput   iup.Ihandle
	passwordInput   iup.Ihandle
	socksBindInput  iup.Ihandle
	httpBindInput   iup.Ihandle
	proxyOnlyToggle iup.Ihandle
	debugDumpToggle iup.Ihandle
	eipProgramInput iup.Ihandle
	eipArgsInput    iup.Ihandle

	inputDialog      iup.Ihandle
	inputPromptLabel iup.Ihandle
	inputText        iup.Ihandle
	inputMultiline   iup.Ihandle
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

	logs       []string
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
	ui.build()
	return ui, nil
}

func (ui *iupUI) Run() error {
	ui.loadInitialState()
	iup.Show(ui.dialog)
	iup.MainLoop()
	ui.app.shutdown()
	return nil
}

func (ui *iupUI) ShowWindow() {
	ui.enqueue(func() {
		ui.dialog.SetAttribute("STATE", "RESTORE")
		iup.Show(ui.dialog)
	})
}

func (ui *iupUI) HideWindow() {
	ui.enqueue(func() {
		iup.Hide(ui.dialog)
	})
}

func (ui *iupUI) Quit() {
	ui.enqueue(func() {
		iup.ExitLoop()
	})
}

func (ui *iupUI) EmitEvent(event string, payload any) {
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

func (ui *iupUI) build() {
	ui.statusLabel = iup.Label("").SetAttributes(`EXPAND=HORIZONTAL, PADDING=8x6`)
	ui.usernameInput = iup.Text().SetAttributes(`EXPAND=HORIZONTAL`)
	ui.passwordInput = iup.Text().SetAttributes(`EXPAND=HORIZONTAL, PASSWORD=YES`)
	ui.socksBindInput = iup.Text().SetAttributes(`EXPAND=HORIZONTAL`)
	ui.httpBindInput = iup.Text().SetAttributes(`EXPAND=HORIZONTAL`)
	ui.proxyOnlyToggle = iup.Toggle("仅代理模式")
	ui.debugDumpToggle = iup.Toggle("调试模式")
	ui.eipProgramInput = iup.Text().SetAttributes(`EXPAND=HORIZONTAL`)
	ui.eipArgsInput = iup.MultiLine().SetAttributes(`EXPAND=HORIZONTAL, VISIBLELINES=4`)
	ui.autoScrollToggle = iup.Toggle("自动滚动")
	ui.autoScrollToggle.SetAttribute("VALUE", "ON")
	ui.logArea = iup.MultiLine().SetAttributes(`EXPAND=YES, READONLY=YES, MULTILINE=YES, VISIBLELINES=18, VISIBLECOLUMNS=80`)
	ui.startStopButton = iup.Button("开始连接")
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
		ui.logs = nil
		ui.renderLogs()
		return iup.DEFAULT
	}))

	configGrid := iup.GridBox(
		iup.Label("用户名"), ui.usernameInput,
		iup.Label("密码"), ui.passwordInput,
		iup.Label("SOCKS 监听地址"), ui.socksBindInput,
		iup.Label("HTTP 监听地址"), ui.httpBindInput,
		iup.Label("浏览器程序路径"), iup.Hbox(ui.eipProgramInput, browseButton, clearButton).SetAttribute("GAP", "6"),
		iup.Label("浏览器参数（每行一个）"), ui.eipArgsInput,
	).SetAttributes(`NUMDIV=2, ORIENTATION=HORIZONTAL, GAPCOL=10, GAPLIN=8, MARGIN=10x10, EXPAND=HORIZONTAL`)

	configTab := iup.Vbox(
		iup.Label("按需填写账号、本地代理监听地址和 EIP 打开方式即可。"),
		configGrid,
		ui.proxyOnlyToggle,
		ui.debugDumpToggle,
		ui.startStopButton,
	).SetAttributes(`TABTITLE="配置", GAP=8, MARGIN=10x10`)

	logsTab := iup.Vbox(
		iup.Hbox(ui.autoScrollToggle, clearLogsButton, iup.Fill()).SetAttribute("GAP", "6"),
		ui.logArea,
	).SetAttributes(`TABTITLE="日志", GAP=8, MARGIN=10x10`)

	tabs := iup.Tabs(configTab, logsTab).SetAttributes(`EXPAND=YES`)

	root := iup.Vbox(
		iup.Label("ZJU Connect GUI").SetAttributes(`FONTSTYLE=BOLD, PADDING=0x4`),
		ui.statusLabel,
		tabs,
	).SetAttributes(`GAP=6, MARGIN=10x10`)

	ui.dialog = iup.Dialog(root).SetAttributes(`TITLE="ZJU Connect GUI", RASTERSIZE=980x720, MINSIZE=840x620`)
	ui.dialog.SetCallback("CLOSE_CB", iup.CloseFunc(func(iup.Ihandle) int {
		if stdRuntime.GOOS == "darwin" {
			go ui.app.Quit()
			return iup.IGNORE
		}
		iup.Hide(ui.dialog)
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
}

func (ui *iupUI) buildInputDialog() {
	ui.inputPromptLabel = iup.Label("请输入内容")
	ui.inputText = iup.Text().SetAttributes(`EXPAND=HORIZONTAL`)
	ui.inputText.SetCallback("ACTION", iup.TextActionFunc(func(ih iup.Ihandle, c int, _ string) int {
		if ui.inputMode == inputDialogModeSMS && c == 13 {
			ui.submitInputDialog()
			return iup.IGNORE
		}
		return iup.DEFAULT
	}))
	ui.inputMultiline = iup.MultiLine().SetAttributes(`VISIBLELINES=6, EXPAND=YES`)
	cancel := iup.Button("取消")
	cancel.SetCallback("ACTION", iup.ActionFunc(func(iup.Ihandle) int {
		iup.Hide(ui.inputDialog)
		return iup.DEFAULT
	}))
	submit := iup.Button("提交")
	submit.SetCallback("ACTION", iup.ActionFunc(func(iup.Ihandle) int {
		ui.submitInputDialog()
		return iup.DEFAULT
	}))
	body := iup.Vbox(
		ui.inputPromptLabel,
		ui.inputText,
		ui.inputMultiline,
		iup.Hbox(iup.Fill(), cancel, submit).SetAttribute("GAP", "6"),
	).SetAttributes(`GAP=8, MARGIN=10x10`)
	ui.inputDialog = iup.Dialog(body).SetAttributes(`TITLE="输入需求", RASTERSIZE=420x220`)
}

func (ui *iupUI) buildCaptchaDialog() {
	ui.captchaPromptLabel = iup.Label("请在图片上按顺序点击对应位置，然后提交")
	ui.captchaCanvas = iup.Canvas().SetAttributes(`RASTERSIZE=360x240`)
	ui.captchaCanvas.SetCallback("ACTION", iup.ActionFunc(func(ih iup.Ihandle) int {
		ui.drawCaptcha(ih)
		return iup.DEFAULT
	}))
	ui.captchaCanvas.SetCallback("BUTTON_CB", iup.ButtonFunc(func(ih iup.Ihandle, button, pressed, x, y int, _ string) int {
		if button == 1 && pressed == 1 && ui.captchaImage != nil {
			ui.captchaPoints = append(ui.captchaPoints, captchaPoint{X: x, Y: y})
			ui.updateCaptchaPointsLabel()
			iup.Refresh(ih)
		}
		return iup.DEFAULT
	}))
	ui.captchaPointsLabel = iup.Label("尚未选择坐标")
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
		ui.captchaCanvas,
		ui.captchaPointsLabel,
		iup.Hbox(undo, clear, iup.Fill(), cancel, submit).SetAttribute("GAP", "6"),
	).SetAttributes(`GAP=8, MARGIN=10x10`)
	ui.captchaDialog = iup.Dialog(body).SetAttributes(`TITLE="图形验证码", RASTERSIZE=420x380`)
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
		ui.appendLog(strings.TrimSpace(line))
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
		ui.inputText.SetAttribute("VISIBLE", "YES")
		ui.inputMultiline.SetAttribute("VISIBLE", "NO")
		ui.inputText.SetAttribute("VALUE", "")
	} else {
		ui.inputMode = inputDialogModeGeneric
		ui.inputDialog.SetAttribute("TITLE", "输入需求")
		ui.inputText.SetAttribute("VISIBLE", "NO")
		ui.inputMultiline.SetAttribute("VISIBLE", "YES")
		ui.inputMultiline.SetAttribute("VALUE", "")
	}
	ui.inputPromptLabel.SetAttribute("TITLE", prompt)
	iup.Show(ui.inputDialog)
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
	ui.captchaCanvas.SetAttribute("RASTERSIZE", fmt.Sprintf("%dx%d", bounds.Dx(), bounds.Dy()))
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
	if ui.captchaImage != nil {
		iup.DrawImage(ih, captchaHandle, 0, 0, ui.captchaImageWidth, ui.captchaImageHeight)
	}
	for idx, point := range ui.captchaPoints {
		iup.DrawArc(ih, point.X-8, point.Y-8, point.X+8, point.Y+8, 0, 360)
		iup.DrawText(ih, strconv.Itoa(idx+1), point.X+10, point.Y-10, -1, -1)
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
	if line == "" {
		return
	}
	ui.logs = append(ui.logs, line)
	if len(ui.logs) > maxLogEntries {
		ui.logs = append([]string(nil), ui.logs[len(ui.logs)-maxLogEntries:]...)
	}
	ui.renderLogs()
}

func (ui *iupUI) renderLogs() {
	ui.logArea.SetAttribute("VALUE", strings.Join(ui.logs, "\n"))
	if ui.autoScrollToggle.GetAttribute("VALUE") == "ON" {
		ui.logArea.SetAttribute("CARETPOS", strconv.Itoa(len(ui.logArea.GetAttribute("VALUE"))))
	}
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

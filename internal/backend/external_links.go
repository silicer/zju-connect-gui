package backend

import (
	"os/exec"
	"runtime"
)

const EIPURL = "http://eip.scmcc.com.cn/"

func OpenEIP(options LaunchOptions) error {
	options = normalizeLaunchOptions(options)

	if options.EIPBrowserProgram == "" {
		var cmd *exec.Cmd
		switch runtime.GOOS {
		case "windows":
			cmd = exec.Command("rundll32", "url.dll,FileProtocolHandler", EIPURL)
		case "darwin":
			cmd = exec.Command("open", EIPURL)
		default:
			cmd = exec.Command("xdg-open", EIPURL)
		}
		return cmd.Start()
	}

	args := append([]string{}, options.EIPBrowserArgs...)
	args = append(args, EIPURL)
	cmd := exec.Command(options.EIPBrowserProgram, args...)
	return cmd.Start()
}

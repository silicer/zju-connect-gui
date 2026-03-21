package backend

import (
	"context"
	"errors"
	"os/exec"

	wailsRuntime "github.com/wailsapp/wails/v2/pkg/runtime"
)

const EIPURL = "http://eip.scmcc.com.cn/"

func OpenEIP(ctx context.Context, options LaunchOptions) error {
	options = normalizeLaunchOptions(options)

	if options.EIPBrowserProgram == "" {
		if ctx == nil {
			return errors.New("runtime context is not initialized")
		}
		wailsRuntime.BrowserOpenURL(ctx, EIPURL)
		return nil
	}

	args := append([]string{}, options.EIPBrowserArgs...)
	args = append(args, EIPURL)
	cmd := exec.Command(options.EIPBrowserProgram, args...)
	return cmd.Start()
}

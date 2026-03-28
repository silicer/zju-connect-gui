package backend

import (
	"fmt"
	"strconv"
	"strings"
)

const (
	resumePendingConnectArg = "--resume-pending-connect"
	waitParentPIDArgPrefix  = "--wait-parent-pid="
)

type ElevatedRelaunchArgs struct {
	ResumePendingConnect bool
	WaitParentPID        int
}

func BuildElevatedRelaunchArgs(parentPID int) []string {
	args := []string{resumePendingConnectArg}
	if parentPID > 0 {
		args = append(args, fmt.Sprintf("%s%d", waitParentPIDArgPrefix, parentPID))
	}
	return args
}

func ParseElevatedRelaunchArgs(args []string) (ElevatedRelaunchArgs, error) {
	parsed := ElevatedRelaunchArgs{}
	for _, arg := range args {
		switch {
		case arg == resumePendingConnectArg:
			parsed.ResumePendingConnect = true
		case strings.HasPrefix(arg, waitParentPIDArgPrefix):
			value := strings.TrimPrefix(arg, waitParentPIDArgPrefix)
			pid, err := strconv.Atoi(value)
			if err != nil || pid <= 0 {
				return ElevatedRelaunchArgs{}, fmt.Errorf("invalid wait-parent pid %q", value)
			}
			parsed.WaitParentPID = pid
			parsed.ResumePendingConnect = true
		}
	}

	return parsed, nil
}

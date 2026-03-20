package backend

import "testing"

func TestIsVPNClientStartedLine(t *testing.T) {
	tests := []struct {
		name string
		line string
		want bool
	}{
		{name: "exact match", line: "VPN client started", want: true},
		{name: "embedded in log", line: "INFO VPN client started successfully", want: true},
		{name: "trimmed match", line: "  VPN client started  ", want: true},
		{name: "different message", line: "VPN client stopped", want: false},
		{name: "empty", line: "", want: false},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			if got := isVPNClientStartedLine(tt.line); got != tt.want {
				t.Fatalf("isVPNClientStartedLine(%q) = %v, want %v", tt.line, got, tt.want)
			}
		})
	}
}

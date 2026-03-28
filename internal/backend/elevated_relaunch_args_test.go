package backend

import (
	"reflect"
	"testing"
)

func TestBuildElevatedRelaunchArgs(t *testing.T) {
	args := BuildElevatedRelaunchArgs(4321)
	want := []string{"--resume-pending-connect", "--wait-parent-pid=4321"}
	if !reflect.DeepEqual(args, want) {
		t.Fatalf("unexpected relaunch args: got %#v want %#v", args, want)
	}
}

func TestParseElevatedRelaunchArgs(t *testing.T) {
	parsed, err := ParseElevatedRelaunchArgs([]string{"--resume-pending-connect", "--wait-parent-pid=4321", "--other-arg"})
	if err != nil {
		t.Fatalf("parse relaunch args: %v", err)
	}
	if !parsed.ResumePendingConnect {
		t.Fatal("expected resume-pending-connect to be true")
	}
	if parsed.WaitParentPID != 4321 {
		t.Fatalf("expected wait-parent pid 4321, got %d", parsed.WaitParentPID)
	}
}

func TestParseElevatedRelaunchArgs_InvalidParentPID(t *testing.T) {
	_, err := ParseElevatedRelaunchArgs([]string{"--wait-parent-pid=abc"})
	if err == nil {
		t.Fatal("expected invalid pid error")
	}
}

package main

import (
	"reflect"
	"testing"

	"zju-connect-gui/internal/backend"
)

func TestLaunchOptionsToBackend(t *testing.T) {
	options := LaunchOptions{
		Protocol:           "atrust",
		Server:             "sslvpn.scmcc.com.cn",
		Port:               443,
		Username:           "user1",
		Password:           "pass1",
		SocksBind:          "127.0.0.1:1080",
		HTTPBind:           "127.0.0.1:8888",
		SecondaryDNSServer: "223.5.5.5",
		AuthType:           "auth/psw",
		LoginDomain:        "AD",
		ClientDataFile:     "client_data.json",
		EIPAutoOpen:        true,
		EIPBrowserProgram:  "/usr/bin/browser",
		EIPBrowserArgs:     []string{"--new-window", "--profile"},
		TunMode:            true,
		DebugDump:          true,
	}

	converted := options.toBackend()

	want := backend.LaunchOptions{
		Protocol:           options.Protocol,
		Server:             options.Server,
		Port:               options.Port,
		Username:           options.Username,
		Password:           options.Password,
		SocksBind:          options.SocksBind,
		HTTPBind:           options.HTTPBind,
		SecondaryDNSServer: options.SecondaryDNSServer,
		AuthType:           options.AuthType,
		LoginDomain:        options.LoginDomain,
		ClientDataFile:     options.ClientDataFile,
		EIPAutoOpen:        options.EIPAutoOpen,
		EIPBrowserProgram:  options.EIPBrowserProgram,
		EIPBrowserArgs:     []string{"--new-window", "--profile"},
		TunMode:            options.TunMode,
		DebugDump:          options.DebugDump,
	}

	if !reflect.DeepEqual(converted, want) {
		t.Fatalf("unexpected backend launch options:\n got: %#v\nwant: %#v", converted, want)
	}

	converted.EIPBrowserArgs[0] = "--modified"
	if options.EIPBrowserArgs[0] != "--new-window" {
		t.Fatalf("expected toBackend to copy browser args slice, got %#v", options.EIPBrowserArgs)
	}
}

func TestLaunchOptionsFromBackend(t *testing.T) {
	options := backend.LaunchOptions{
		Protocol:           "easyconnect",
		Server:             "vpn.example.com",
		Port:               8443,
		Username:           "user2",
		Password:           "pass2",
		SocksBind:          "127.0.0.1:2080",
		HTTPBind:           "127.0.0.1:2888",
		SecondaryDNSServer: "1.1.1.1",
		AuthType:           "auth/sms",
		LoginDomain:        "EXAMPLE",
		ClientDataFile:     "other_client_data.json",
		EIPAutoOpen:        false,
		EIPBrowserProgram:  "C:/Program Files/Browser/browser.exe",
		EIPBrowserArgs:     []string{"--app", "https://example.com"},
		TunMode:            false,
		DebugDump:          true,
	}

	converted := launchOptionsFromBackend(options)

	want := LaunchOptions{
		Protocol:           options.Protocol,
		Server:             options.Server,
		Port:               options.Port,
		Username:           options.Username,
		Password:           options.Password,
		SocksBind:          options.SocksBind,
		HTTPBind:           options.HTTPBind,
		SecondaryDNSServer: options.SecondaryDNSServer,
		AuthType:           options.AuthType,
		LoginDomain:        options.LoginDomain,
		ClientDataFile:     options.ClientDataFile,
		EIPAutoOpen:        options.EIPAutoOpen,
		EIPBrowserProgram:  options.EIPBrowserProgram,
		EIPBrowserArgs:     []string{"--app", "https://example.com"},
		TunMode:            options.TunMode,
		DebugDump:          options.DebugDump,
	}

	if !reflect.DeepEqual(converted, want) {
		t.Fatalf("unexpected main launch options:\n got: %#v\nwant: %#v", converted, want)
	}

	converted.EIPBrowserArgs[0] = "--modified"
	if options.EIPBrowserArgs[0] != "--app" {
		t.Fatalf("expected launchOptionsFromBackend to copy browser args slice, got %#v", options.EIPBrowserArgs)
	}
}

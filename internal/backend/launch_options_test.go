package backend

import (
	"slices"
	"testing"
)

func TestNormalizeLaunchOptions_Defaults(t *testing.T) {
	options := normalizeLaunchOptions(LaunchOptions{
		Username: "user1",
		Password: "pass1",
	})

	if options.Protocol != defaultProtocol {
		t.Fatalf("expected protocol %q, got %q", defaultProtocol, options.Protocol)
	}
	if options.Server != defaultServer {
		t.Fatalf("expected server %q, got %q", defaultServer, options.Server)
	}
	if options.Port != defaultPort {
		t.Fatalf("expected port %d, got %d", defaultPort, options.Port)
	}
	if options.SocksBind != defaultSocksBind {
		t.Fatalf("expected socks-bind %q, got %q", defaultSocksBind, options.SocksBind)
	}
	if options.HTTPBind != defaultHTTPBind {
		t.Fatalf("expected http-bind %q, got %q", defaultHTTPBind, options.HTTPBind)
	}
	if options.SecondaryDNSServer != defaultSecondaryDNSServer {
		t.Fatalf("expected secondary-dns-server %q, got %q", defaultSecondaryDNSServer, options.SecondaryDNSServer)
	}
	if options.AuthType != defaultAuthType {
		t.Fatalf("expected auth-type %q, got %q", defaultAuthType, options.AuthType)
	}
	if options.LoginDomain != defaultLoginDomain {
		t.Fatalf("expected login-domain %q, got %q", defaultLoginDomain, options.LoginDomain)
	}
	if options.ClientDataFile != defaultClientDataFile {
		t.Fatalf("expected client-data-file %q, got %q", defaultClientDataFile, options.ClientDataFile)
	}
}

func TestNormalizeLaunchOptions_EIPBrowserFields(t *testing.T) {
	options := normalizeLaunchOptions(LaunchOptions{
		Username:          "user1",
		Password:          "pass1",
		EIPBrowserProgram: "  /usr/bin/browser  ",
		EIPBrowserArgs:    []string{" --new-window ", "", "  --profile ", "   "},
	})

	if options.EIPBrowserProgram != "/usr/bin/browser" {
		t.Fatalf("expected trimmed browser program, got %q", options.EIPBrowserProgram)
	}
	if len(options.EIPBrowserArgs) != 2 {
		t.Fatalf("expected 2 normalized browser args, got %#v", options.EIPBrowserArgs)
	}
	if options.EIPBrowserArgs[0] != "--new-window" || options.EIPBrowserArgs[1] != "--profile" {
		t.Fatalf("unexpected normalized browser args: %#v", options.EIPBrowserArgs)
	}
}

func TestLaunchOptionsValidate(t *testing.T) {
	invalid := normalizeLaunchOptions(LaunchOptions{
		Port:     70000,
		Username: "user1",
		Password: "pass1",
	})
	if err := invalid.Validate(); err == nil {
		t.Fatal("expected invalid port error")
	}

	invalid = normalizeLaunchOptions(LaunchOptions{
		Protocol:  "invalid",
		Port:      443,
		Username:  "user1",
		Password:  "pass1",
		SocksBind: "127.0.0.1:1080",
		HTTPBind:  "127.0.0.1:8888",
	})
	if err := invalid.Validate(); err == nil {
		t.Fatal("expected unsupported protocol error")
	}

	valid := normalizeLaunchOptions(LaunchOptions{
		Port:      443,
		Username:  "user1",
		Password:  "pass1",
		SocksBind: "127.0.0.1:1080",
		HTTPBind:  "127.0.0.1:8888",
	})
	if err := valid.Validate(); err != nil {
		t.Fatalf("expected valid options, got error: %v", err)
	}
}

func TestBuildArgs(t *testing.T) {
	options := normalizeLaunchOptions(LaunchOptions{
		Port:      8443,
		Username:  "user1",
		Password:  "pass1",
		SocksBind: "127.0.0.1:2080",
		HTTPBind:  "127.0.0.1:2888",
		TunMode:   true,
		DebugDump: true,
	})

	args := options.BuildArgs("/tmp/gui_captcha.png")

	mustHaveFlagValue(t, args, "-protocol", defaultProtocol)
	mustHaveFlagValue(t, args, "-server", defaultServer)
	mustHaveFlagValue(t, args, "-port", "8443")
	mustHaveFlagValue(t, args, "-username", "user1")
	mustHaveFlagValue(t, args, "-password", "pass1")
	mustHaveFlag(t, args, "-disable-zju-config")
	mustHaveFlagValue(t, args, "-socks-bind", "127.0.0.1:2080")
	mustHaveFlagValue(t, args, "-http-bind", "127.0.0.1:2888")
	mustHaveFlagValue(t, args, "-secondary-dns-server", defaultSecondaryDNSServer)
	mustHaveFlagValue(t, args, "-auth-type", defaultAuthType)
	mustHaveFlagValue(t, args, "-login-domain", defaultLoginDomain)
	mustHaveFlagValue(t, args, "-client-data-file", defaultClientDataFile)
	mustHaveFlagValue(t, args, "-graph-code-file", "/tmp/gui_captcha.png")
	mustHaveFlag(t, args, "-tun-mode")
	mustHaveFlag(t, args, "-add-route")
	mustHaveFlag(t, args, "-dns-hijack")
	mustHaveFlag(t, args, "-fake-ip")
	mustHaveFlag(t, args, "-debug-dump")
}

func TestBuildArgsWithoutTunDebug(t *testing.T) {
	options := normalizeLaunchOptions(LaunchOptions{
		Port:      443,
		Username:  "user1",
		Password:  "pass1",
		SocksBind: "127.0.0.1:1080",
		HTTPBind:  "127.0.0.1:8888",
	})

	args := options.BuildArgs("/tmp/gui_captcha.png")

	mustNotHaveFlag(t, args, "-tun-mode")
	mustNotHaveFlag(t, args, "-add-route")
	mustNotHaveFlag(t, args, "-dns-hijack")
	mustNotHaveFlag(t, args, "-fake-ip")
	mustNotHaveFlag(t, args, "-debug-dump")
}

func mustHaveFlag(t *testing.T, args []string, flag string) {
	t.Helper()
	if slices.Contains(args, flag) {
		return
	}
	t.Fatalf("missing flag %s in args: %#v", flag, args)
}

func mustNotHaveFlag(t *testing.T, args []string, flag string) {
	t.Helper()
	if slices.Contains(args, flag) {
		t.Fatalf("flag %s should not exist in args: %#v", flag, args)
	}
}

func mustHaveFlagValue(t *testing.T, args []string, flag string, value string) {
	t.Helper()
	for i := 0; i < len(args)-1; i++ {
		if args[i] == flag {
			if args[i+1] != value {
				t.Fatalf("flag %s has value %q, want %q", flag, args[i+1], value)
			}
			return
		}
	}
	t.Fatalf("missing flag-value pair %s %q in args: %#v", flag, value, args)
}

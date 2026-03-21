package backend

import (
	"errors"
	"fmt"
	"strconv"
	"strings"
)

const (
	defaultProtocol           = "atrust"
	defaultServer             = "sslvpn.scmcc.com.cn"
	defaultPort               = 443
	defaultSocksBind          = "127.0.0.1:1080"
	defaultHTTPBind           = "127.0.0.1:8888"
	defaultSecondaryDNSServer = "223.5.5.5"
	defaultAuthType           = "auth/psw"
	defaultLoginDomain        = "AD"
	defaultClientDataFile     = "client_data.json"
)

var supportedProtocols = map[string]struct{}{
	"atrust":      {},
	"easyconnect": {},
}

type LaunchOptions struct {
	Protocol           string   `json:"protocol"`
	Server             string   `json:"server"`
	Port               int      `json:"port"`
	Username           string   `json:"username"`
	Password           string   `json:"password"`
	SocksBind          string   `json:"socksBind"`
	HTTPBind           string   `json:"httpBind"`
	SecondaryDNSServer string   `json:"secondaryDnsServer"`
	AuthType           string   `json:"authType"`
	LoginDomain        string   `json:"loginDomain"`
	ClientDataFile     string   `json:"clientDataFile"`
	EIPBrowserProgram  string   `json:"eipBrowserProgram"`
	EIPBrowserArgs     []string `json:"eipBrowserArgs"`
	TunMode            bool     `json:"tunMode"`
	DebugDump          bool     `json:"debugDump"`
}

func normalizeLaunchOptions(options LaunchOptions) LaunchOptions {
	options.Protocol = strings.ToLower(strings.TrimSpace(options.Protocol))
	options.Server = strings.TrimSpace(options.Server)
	options.Username = strings.TrimSpace(options.Username)
	options.SocksBind = strings.TrimSpace(options.SocksBind)
	options.HTTPBind = strings.TrimSpace(options.HTTPBind)
	options.SecondaryDNSServer = strings.TrimSpace(options.SecondaryDNSServer)
	options.AuthType = strings.TrimSpace(options.AuthType)
	options.LoginDomain = strings.TrimSpace(options.LoginDomain)
	options.ClientDataFile = strings.TrimSpace(options.ClientDataFile)
	options.EIPBrowserProgram = strings.TrimSpace(options.EIPBrowserProgram)
	options.EIPBrowserArgs = normalizeStringList(options.EIPBrowserArgs)

	if options.Protocol == "" {
		options.Protocol = defaultProtocol
	}
	if options.Server == "" {
		options.Server = defaultServer
	}
	if options.Port == 0 {
		options.Port = defaultPort
	}
	if options.SocksBind == "" {
		options.SocksBind = defaultSocksBind
	}
	if options.HTTPBind == "" {
		options.HTTPBind = defaultHTTPBind
	}
	if options.SecondaryDNSServer == "" {
		options.SecondaryDNSServer = defaultSecondaryDNSServer
	}
	if options.AuthType == "" {
		options.AuthType = defaultAuthType
	}
	if options.LoginDomain == "" {
		options.LoginDomain = defaultLoginDomain
	}
	if options.ClientDataFile == "" {
		options.ClientDataFile = defaultClientDataFile
	}

	return options
}

func normalizeStringList(values []string) []string {
	if len(values) == 0 {
		return nil
	}

	normalized := make([]string, 0, len(values))
	for _, value := range values {
		trimmed := strings.TrimSpace(value)
		if trimmed == "" {
			continue
		}
		normalized = append(normalized, trimmed)
	}

	if len(normalized) == 0 {
		return nil
	}

	return normalized
}

func (o LaunchOptions) Validate() error {
	if _, ok := supportedProtocols[o.Protocol]; !ok {
		return fmt.Errorf("unsupported protocol: %s", o.Protocol)
	}
	if o.Server == "" {
		return errors.New("server cannot be empty")
	}
	if o.Port < 1 || o.Port > 65535 {
		return errors.New("port must be between 1 and 65535")
	}
	if o.Username == "" {
		return errors.New("username cannot be empty")
	}
	if o.Password == "" {
		return errors.New("password cannot be empty")
	}
	if o.SocksBind == "" {
		return errors.New("socks-bind cannot be empty")
	}
	if o.HTTPBind == "" {
		return errors.New("http-bind cannot be empty")
	}
	if o.SecondaryDNSServer == "" {
		return errors.New("secondary-dns-server cannot be empty")
	}
	if o.AuthType == "" {
		return errors.New("auth-type cannot be empty")
	}
	if o.LoginDomain == "" {
		return errors.New("login-domain cannot be empty")
	}
	if o.ClientDataFile == "" {
		return errors.New("client-data-file cannot be empty")
	}

	return nil
}

func (o LaunchOptions) BuildArgs(captchaPath string) []string {
	args := []string{
		"-protocol", o.Protocol,
		"-server", o.Server,
		"-port", strconv.Itoa(o.Port),
		"-username", o.Username,
		"-password", o.Password,
		"-disable-zju-config",
		"-socks-bind", o.SocksBind,
		"-http-bind", o.HTTPBind,
		"-secondary-dns-server", o.SecondaryDNSServer,
		"-auth-type", o.AuthType,
		"-login-domain", o.LoginDomain,
		"-client-data-file", o.ClientDataFile,
	}

	if captchaPath != "" {
		args = append(args, "-graph-code-file", captchaPath)
	}

	if o.TunMode {
		args = append(args, "-tun-mode", "-add-route", "-dns-hijack", "-fake-ip")
	}

	if o.DebugDump {
		args = append(args, "-debug-dump")
	}

	return args
}

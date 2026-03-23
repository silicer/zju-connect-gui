package main

import "zju-connect-gui/internal/backend"

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

func (o LaunchOptions) toBackend() backend.LaunchOptions {
	return backend.LaunchOptions{
		Protocol:           o.Protocol,
		Server:             o.Server,
		Port:               o.Port,
		Username:           o.Username,
		Password:           o.Password,
		SocksBind:          o.SocksBind,
		HTTPBind:           o.HTTPBind,
		SecondaryDNSServer: o.SecondaryDNSServer,
		AuthType:           o.AuthType,
		LoginDomain:        o.LoginDomain,
		ClientDataFile:     o.ClientDataFile,
		EIPBrowserProgram:  o.EIPBrowserProgram,
		EIPBrowserArgs:     append([]string(nil), o.EIPBrowserArgs...),
		TunMode:            o.TunMode,
		DebugDump:          o.DebugDump,
	}
}

func launchOptionsFromBackend(options backend.LaunchOptions) LaunchOptions {
	return LaunchOptions{
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
		EIPBrowserProgram:  options.EIPBrowserProgram,
		EIPBrowserArgs:     append([]string(nil), options.EIPBrowserArgs...),
		TunMode:            options.TunMode,
		DebugDump:          options.DebugDump,
	}
}

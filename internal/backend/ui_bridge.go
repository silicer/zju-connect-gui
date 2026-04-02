package backend

type UIBridge interface {
	EmitEvent(event string, payload any)
	ShowWindow()
}

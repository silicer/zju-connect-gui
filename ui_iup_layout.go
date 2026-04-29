package main

import (
	"fmt"
	"strconv"
	"strings"
)

func clampDialogSize(dialogRasterSize string, screenRasterSize string, minW, minH int) string {
	dialogW, dialogH := parseSize(dialogRasterSize)
	screenW, screenH := parseSize(screenRasterSize)

	if screenW <= 0 || screenH <= 0 {
		return dialogRasterSize
	}

	targetW := dialogW
	targetH := dialogH

	if targetW > screenW {
		targetW = screenW - 64 // Margin
	}
	if targetH > screenH {
		targetH = screenH - 128 // Margin for taskbar etc
	}
	// We don't artificially expand natural sizes just to hit minW/minH.
	// The min size constraint is enforced by IUP's MINSIZE.
	// We only clamp downward to fit the screen.
	if targetW < minW && dialogW > minW {
		targetW = minW
	}
	if targetH < minH && dialogH > minH {
		targetH = minH
	}
	if targetW == dialogW && targetH == dialogH {
		return dialogRasterSize
	}

	// If parse failed, dialogW/H are 0, return original
	if dialogW <= 0 || dialogH <= 0 {
		return dialogRasterSize
	}

	return fmt.Sprintf("%dx%d", targetW, targetH)
}

func parseSize(s string) (int, int) {
	parts := strings.Split(s, "x")
	if len(parts) != 2 {
		return 0, 0
	}
	w, errW := strconv.Atoi(parts[0])
	h, errH := strconv.Atoi(parts[1])
	if errW != nil || errH != nil {
		return 0, 0
	}
	return w, h
}

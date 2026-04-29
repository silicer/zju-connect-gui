package main

import "testing"

func TestClampDialogSize(t *testing.T) {
	tests := []struct {
		name       string
		dialog     string
		screen     string
		minW, minH int
		expected   string
	}{
		{"normal", "800x600", "1920x1080", 600, 400, "800x600"},
		{"too big w", "2000x600", "1920x1080", 600, 400, "1856x600"},
		{"too big h", "800x1200", "1920x1080", 600, 400, "800x952"},
		{"too big both", "2000x2000", "1920x1080", 600, 400, "1856x952"},
		{"smaller than min", "400x300", "1920x1080", 600, 400, "400x300"}, // clamp doesn't expand natural size unless screen bounds force it
		{"clamp and min conflict", "2000x2000", "800x600", 1000, 800, "1000x800"},
		{"bad dialog size", "bad", "1920x1080", 600, 400, "bad"},
		{"bad screen size", "800x600", "bad", 600, 400, "800x600"},
	}

	for _, tt := range tests {
		t.Run(tt.name, func(t *testing.T) {
			got := clampDialogSize(tt.dialog, tt.screen, tt.minW, tt.minH)
			if got != tt.expected {
				t.Errorf("clampDialogSize(%q, %q, %d, %d) = %q; want %q", tt.dialog, tt.screen, tt.minW, tt.minH, got, tt.expected)
			}
		})
	}
}

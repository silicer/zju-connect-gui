package main

import (
	"bytes"
	_ "embed"
	"image"
	_ "image/png"

	"github.com/gen2brain/iup-go/iup"
)

//go:embed assets/gemini.png
var appIconPNG []byte

func loadApplicationIcon() (iup.Ihandle, error) {
	img, _, err := image.Decode(bytes.NewReader(appIconPNG))
	if err != nil {
		return 0, err
	}
	return iup.ImageFromImage(img), nil
}

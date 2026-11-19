# MX Creative review icon assets

`assets/mx-keypad-icons/` contains seven original, standalone 480 × 480 SVG candidates for an MX Creative Console keypad configuration. The geometry, color choices, and labels were authored for fesReview; no vendor icon, font glyph, or application package is embedded.

These files are **UI assets only**. fesReview does not load them, transfer them to a device, or expose a display-write route. The hardware input protocol and a supported display/icon workflow are not established. Keeping the assets separate avoids implying that the application has qualified either capability.

## Button meaning

The candidate set uses this left-to-right order; it does not define or modify the controller input protocol:

| File | Review action | Visual cue |
| --- | --- | --- |
| `play-pause.svg` | Toggle playback | Play wedge and pause bars |
| `first-frame.svg` | Seek to the first indexed frame | Boundary with inward chevrons |
| `last-frame.svg` | Seek to the last indexed frame | Chevrons toward the end boundary |
| `back-1s.svg` | Move back one second | Left-turning arrow and one |
| `forward-1s.svg` | Move forward one second | Right-turning arrow and one |
| `screenshot.svg` | Save the selected frame | Capture frame corners and target |
| `dictate.svg` | Start or stop dictation | Microphone and voice marks |

All icons use a `#07151b` dark field, `#36d8f5` cyan, and `#f4fbfd` for contrast. The square remains readable independently of host application geometry. Small uppercase labels provide a fallback when a glyph is unfamiliar.

## Preview

Rendered PNG copies and a PNG contact sheet are stored in `assets/mx-keypad-icons/previews/`. The SVG sources remain authoritative; the PNGs are review artifacts, not runtime dependencies.

To regenerate PNG previews on macOS without adding dependencies:

```sh
mkdir -p assets/mx-keypad-icons/previews
qlmanage -t -s 480 -o assets/mx-keypad-icons/previews assets/mx-keypad-icons/*.svg
```

`contact-sheet.png` is the combined review artifact; the seven per-icon PNGs can be regenerated independently.

## Acceptance

- Every source parses as SVG and uses a 480 × 480 square view box.
- Every source remains self-contained and under 8 KiB, with an accessible title, description, label, dark background, and cyan accent.
- At normal preview size, each action is distinguishable by shape alone; labels are secondary.
- At the keypad's intended display scale, both glyph and label retain clear contrast.
- A reviewer confirms that geometry is original and that no vendor asset, trademark treatment, or device-specific display behavior has been introduced.
- The icons neither alter foreground-controller selection nor participate in input, helper launch, or display/image writes.

Run the dependency-free structural check with:

```sh
python3 scripts/test-mx-keypad-icons.py
```

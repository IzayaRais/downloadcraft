# Third-party notices

Download Craft is original Rust code written for this project. Its interface
design was inspired by modern download managers — no third-party source
code is included in this repo.

## Material Icons font — Apache License 2.0

`assets/fonts/MaterialIcons-Regular.ttf` (© Google) is bundled for UI glyphs.
Redistribution of this font requires keeping this attribution.

- Source: https://github.com/google/material-design-icons
- License: Apache-2.0 — https://www.apache.org/licenses/LICENSE-2.0

## Design inspiration

The layout and usability of this app were inspired by modern desktop
download managers (including AB Download Manager and FluxDown). Inspiration
means ideas only: every line of code here was written independently for
this project, and no source files from those projects were copied,
adapted, or embedded.

## Rust crates

- `sha2` (© RustCrypto contributors) — MIT / Apache-2.0, used for optional
  SHA-256 file-integrity verification.
- `egui_extras` (© egui contributors) — MIT / Apache-2.0, used for the
  download table (`TableBuilder`: aligned headers/rows, resizable columns,
  full-row selection).

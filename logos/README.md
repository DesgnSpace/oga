# Oga logo

The ring is the job you are holding — the O of Oga, one whole piece of work. The three blocks beside it are the hands you added: parts of that job, already cut loose and moving in parallel, each its own colour because each is somewhere else. Every colour is one the landing page already uses, so the mark and the site read as one thing.

`logos/export/logo.svg` is the only source. Everything else — the PNG exports, the desktop icons, the landing copy of the SVG — is rendered from it.

## Regenerate

```sh
bun logos/generate.js
```

Needs `rsvg-convert` (`brew install librsvg`) and macOS `iconutil`.

## What it writes

| Path | Contents |
| --- | --- |
| `logos/export/logo-*.png` | 16, 32, 48, 192, 512, 1024, 2048 |
| `landing/logo.svg` | copy of the source |
| `rust/apps/oga-desktop/icons/` | Tauri set: PNGs, `icon.ico`, `icon.icns` |

`icon.icns` is build staging and stays out of git.

## Colours

| Role | Hex | Landing token |
| --- | --- | --- |
| Tile | `#0A0A0A` | — |
| Ring | `#FFFDF5` | `--background` |
| Top block | `#FFD23F` | `--accent` |
| Middle block | `#74B9FF` | `--sidebar` |
| Bottom block | `#88D498` | `--sunken` |

The blocks carry the tile's own corner ratio, `rx` 18 on 76, so they read as miniatures of it.

No shape overlaps another, so the mark also holds as one flat colour on any background.

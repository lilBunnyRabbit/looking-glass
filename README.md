# looking-glass

Transparent, always-on-top image overlay for Windows. Put a reference image over any window (a game's livery editor, a drawing app), make it see-through, lock it so clicks pass through, and trace.

## Use

1. Download `looking-glass-*.exe` from [Releases](https://github.com/lilBunnyRabbit/looking-glass/releases) and run it.
2. Drop an image on the window, or double-click to open one.
3. Drag to move. Scroll or use the corner grip to resize.
4. Set the opacity, then hit **Lock**: clicks go through to the app underneath.

| Key | Action |
| --- | --- |
| `Ctrl+Alt+T` | lock / unlock |
| `Ctrl+Alt+Up` / `Down` | opacity |

Games must run in windowed or borderless mode; exclusive fullscreen hides every overlay.

## Build

```sh
cargo build --release
```

Push a `v*` tag to build and publish a release.

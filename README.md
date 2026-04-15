# macos-wp

A small CLI for managing macOS wallpapers per display — and, unlike the system settings, changes stick to Spaces you haven't created yet.

macOS's built-in wallpaper UI doesn't propagate a display's wallpaper to Spaces created later; any new Space shows the OS default. `macos-wp` writes directly into `com.apple.wallpaper`'s plist so every existing and future Space on a given display uses your chosen image. No background agent, no polling.

## Install

Requires macOS 26 (Tahoe).

```
brew install marek-vybiral/macos-wp/macos-wp
```

Or build from source (requires Rust):

```
git clone https://github.com/marek-vybiral/homebrew-macos-wp
cd homebrew-macos-wp
cargo install --path .
```

## Usage

```
macos-wp list
macos-wp set <image>               [--display <alias|uuid>] [--space <uuid>]
macos-wp random <dir>              [--display <alias|uuid>] [--space <uuid>]
macos-wp reset                     [--display <alias|uuid>]
macos-wp restore
```

### Display aliases

`list` shows each display with a friendly alias:

- `builtin` — the built-in laptop display.
- `ext-1`, `ext-2`, ... — currently-connected external displays.
- `offline-1`, `offline-2`, ... — displays that are in the wallpaper plist but not currently connected. You can still set wallpapers on them; the setting takes effect when they reconnect.

You can always pass a raw UUID instead of an alias.

### Examples

One wallpaper on every display, everywhere:

```
macos-wp set ~/Pictures/summer.png
```

Different wallpaper per display, surviving new Spaces:

```
macos-wp set ~/Pictures/summer.png      --display builtin
macos-wp set ~/Pictures/birkenhain.jpg  --display ext-1
```

One-off wallpaper on a single Space (not sticky to future ones):

```
macos-wp set ~/Pictures/fancy.jpg --space <SPACE_UUID>
```

Space UUIDs come from the `Spaces` keys in the plist — the tool doesn't map them to "Desktop 1 / 2 / 3" because macOS doesn't expose that mapping. Mostly a power-user flag.

Random image from a folder:

```
macos-wp random ~/Pictures/Wallpapers
macos-wp random ~/Pictures/Wallpapers --display ext-1
```

Re-sync all Spaces to match each display's current default (fix drift after using System Settings):

```
macos-wp reset
```

Undo the last change:

```
macos-wp restore
```

## How it works

macOS stores wallpaper state in `~/Library/Application Support/com.apple.wallpaper/Store/Index.plist`. The schema (as of Tahoe) has:

- **`Displays.<DisplayUUID>`** — the per-display default applied to any Space that lacks an override.
- **`Spaces.<SpaceUUID>.Displays.<DisplayUUID>`** — per-Space-per-display override.
- **`SystemDefault`** — fallback template.

A "sticky" `set` writes the top-level `Displays` default *and* every existing per-Space override for that display, so new Spaces inherit it and old Spaces stop disagreeing. A "`--space`" set only writes the one per-Space override.

Before each write, the plist is backed up to `Index.plist.bak` next to the original. `WallpaperAgent` is then signalled to pick up the change.

## Caveats

- Written against macOS Tahoe's `com.apple.wallpaper` plist schema. If Apple changes it, the tool refuses to run. `--force-schema` overrides (a backup is still written first).
- Only handles single still-image wallpapers. Dynamic, video, shuffle, and solid-color wallpapers use different `Configuration` schemas and aren't supported.
- Some notch-masking or wallpaper-rotation utilities (e.g. TopNotch) re-write `Index.plist` themselves and may undo `macos-wp`'s changes.

## License

[Unlicense](./UNLICENSE) — public domain.

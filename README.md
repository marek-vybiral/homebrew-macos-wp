# macos-wp

Tiny CLI for managing macOS wallpapers per display — including on Spaces you haven't created yet.

macOS's built-in wallpaper settings don't propagate to Spaces created later: a new Space always shows the system default. `macos-wp` writes directly into `com.apple.wallpaper`'s plist so every existing and future Space on a given display uses your chosen image.

## Install

Requires macOS 26 (Tahoe).

```
brew install YOUR_GH_USERNAME/macos-wp/macos-wp
```

Or build from source:

```
git clone https://github.com/YOUR_GH_USERNAME/homebrew-macos-wp
cd homebrew-macos-wp
cargo install --path .
```

## Usage

List displays and their current wallpaper:

```
macos-wp list
```

Set a wallpaper for one display (applies to all existing + future Spaces):

```
macos-wp set /path/to/image.jpg --display <DISPLAY_UUID>
```

Display UUIDs come from `macos-wp list`. The plist is backed up to `Index.plist.bak` next to the original before any write.

## Caveats

- Written against macOS Tahoe's `com.apple.wallpaper` plist schema. If Apple changes it, the tool refuses to run; pass `--force-schema` to override (may corrupt your wallpaper settings — a backup is still written).
- Only writes per-display slots; other wallpaper features (shuffle, dynamic, video) aren't supported.

## License

[Unlicense](./UNLICENSE) — public domain.

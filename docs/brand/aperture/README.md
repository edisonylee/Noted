# Noted Aperture icon

Selected icon: a white open frame on a black tile, with citron, blue, pink,
and green accents together at the opening. The icon is a symbol, not an initial.

The canonical raster artwork is [noted-master.png](../../../src-tauri/icons/noted-master.png).
Desktop PNG, ICNS, and ICO assets are derived from that exact approved artwork.
The older `src-tauri/icon-source.svg` is a legacy N concept and must not be used
to regenerate the current icon. The wordmark is a separate brand asset.

The existing Tauri bundle paths already reference the updated desktop assets.
`public/noted-logo.png` is the 256 px browser icon; `public/apple-touch-icon.png`
is the 180 px web home-screen icon. Native iOS and Android assets were not changed.

To regenerate into a temporary directory:

```sh
bunx tauri icon src-tauri/icons/noted-master.png --output /tmp/noted-aperture-export --ios-color '#0A0A0A'
bunx tauri icon src-tauri/icons/noted-master.png --output /tmp/noted-aperture-web --png 180 --png 256
```

Copy only the top-level desktop outputs into `src-tauri/icons/`, and the two
web PNGs into their public paths above. Keep the master. Updating these source
assets does not change the installed app until the standard app is rebuilt.

The raster preserves the selected concept's soft tile depth. At tiny sizes,
the white silhouette remains dominant and the four accents become a small
color signature; they are not status indicators.

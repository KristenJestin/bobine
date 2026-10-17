# Bobine's brand

The maintainer's drawings, copied as delivered: never redrawn here. The mark is a reel: a coral
arc running around a dark-brown ring, a large cream dot where the film leaves, three cream holes.

| File | For |
|---|---|
| `marque.svg` | the mark alone, on a dark background |
| `marque-fond-clair.svg` | the mark alone, on a light background |
| `icone.svg` | the app icon, a rounded square (also `assets/linux/…/scalable/apps/bobine.svg`) |
| `icone-pleine.svg` | the app icon full-bleed, for places that round the corners themselves |
| `icone-android-avant-plan.svg` | the foreground layer of an adaptive (Android) icon |
| `favicon.svg`, `favicon.ico` | the favicon of a web page |
| `png/icone-<16…1024>.png` | the icon rendered square at each size; the pill bar and the login draw the one that fits the screen's scale (`src/brand.rs`), `assets/linux` installs 16 to 512 under hicolor |
| `png/favicon-16.png`, `png/favicon-32.png` | the favicon |
| `png/apple-touch-icon.png` | the web page's touch icon |
| `png/icone-pleine-192.png`, `png/icone-pleine-512.png` | the full-bleed icon |
| `png/android-avant-plan-432.png` | the Android foreground render |
| `couleurs.txt` | the colours below, as delivered |

## Colours

| Name | Value | Where |
|---|---|---|
| Accent on dark | `#f45246` | the app's coral (`theme::brand`): primary button, active pill, progress bars, keyboard focus ring, checked boxes, current hero indicator. Black text on it |
| Accent on light | `#c92420` | the coral on a light background |
| Cream | `#ece5d8` | the dots of the mark |
| Background | `#14120f` | the icon's tile |
| Ring | `#502f16` | the dark ring of the mark |

The app's own greys stay neutral: no cream text, no warm background.

## The desktop

`assets/linux/share/` is laid out as `/usr/share`: `applications/bobine.desktop` (named after the
window's app id, `bobine::APP_ID`) and `icons/hicolor/<size>x<size>/apps/bobine.png` plus
`scalable/apps/bobine.svg`. `tools/package.sh` puts it in the release archive.

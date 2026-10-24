# Brand

Direction **Midnight** with mark **1a (slim band)**, chosen Oct 23, 2026. Licensed CC BY 4.0 ([assets](../README.md#license)).

| File | Use |
| --- | --- |
| `mark.svg` | The mark on dark grounds (48 px and up) |
| `mark-on-light.svg` | The mark on light grounds |
| `mark-mono.svg` | One color, with the window cut out; recolor freely |
| `mark-small.svg` | 32 px and below: no window or bowl, a thicker band |
| `lockup.svg`, `lockup-on-light.svg` | The mark with the wordmark |
| `wordmark.svg` | "WELLFED" alone, outlined |
| `app-icon.svg`, `favicon.svg` | The mark on a navy tile; PNGs in `png/` |
| `android/` | The adaptive launcher icon (foreground, monochrome for themed icons, navy background) |
| `og.svg` | The 1200 × 630 social card for well-fed.dev ([site](../../docs/web/site.md#social-card)) |
| `tokens.json` | The palette, light and dark theme roles (the dark theme is neutral graphite), the contrast pairs every theme must meet, and type, spacing, radii, focus, elevation and motion, shared by the app and the web UI. `pnpm generate` in `src/web/` writes the CSS and refuses a pair under its minimum |

## Rules

- **The mark is the feeder from the front:** hopper with its food window, a slim blue control band, the base, and the bowl in front. Light marks go on navy or night, dark marks on mist or white.
- **Blue is the signal color.** Use `signal` (#2563EB) on light grounds, where white text on it passes contrast, and `signal_on_dark` (#3B82F6) on navy. Keep `kibble` amber for food states only.
- **Type is Inter.** 800 for display and the wordmark, 400–600 for UI.
- **Keep clear space** of at least the band's width plus the window's width around the mark. Don't stretch the mark, recolor its parts separately, or add effects.
- **Device imagery is unbranded** ([renders](../README.md#rendering)). Our name goes in the layout, never on the hardware.

## Regenerating

The marks are hand-drawn on a 64-unit grid. The PNGs come from the SVGs with `rsvg-convert` (for example `rsvg-convert -w 512 -h 512 app-icon.svg -o png/app-icon-512.png`). The wordmark is Inter at weight 800 with 0.03 em tracking and a 30-unit cap height, converted to outlines, so it renders without the font installed.

# Render spec

The images the app, the web UI and the docs need from `plaf103.blend`, and the rules for making them. It is written for whoever extends `render.py`. Brand rules are in [brand](../brand/README.md), licensing in [assets](../README.md#license).

## Ground rules

- **Every image comes from `render.py`.** The one exception is the site's line art, which `lineart.py` traces from the same cameras (see [line art](#line-art)). Extend `render.py` with views, states and stages. Don't hand-edit outputs or render from a saved scene. One command regenerates everything: `blender -b assets/model/plaf103.blend -P assets/model/render.py -- --all`. A single image also works with `--view` and `--state`.
- **Don't edit the product model.** Cameras, lights, backgrounds and state changes (visibility, transforms, emission strength) are applied in the script at render time. If an image needs geometry the model doesn't have (the power jack, the battery door, an inner part), list it in the report rather than inventing it; the owner models it.
- **Unbranded, always.** Keep the `Logo` and `PCB_ChipLabel` objects hidden; `render.py`'s `unbrand()` already does this. No vendor marks, no vendor app UI, no device identifiers, and nothing fictional printed on the hardware.
- **Locked cameras within a set.** Every state of a view uses the same camera, framing and lights, so the app can swap images (food levels, lid open) without the feeder jumping. Name cameras per view in code, not per state.
- **Truthful states.** Show only what the hardware does. The only recorded indicator color is the **red fault light**; the others are unknown, so render them **unlit** and let the UI draw light through [hotspots](#hotspots). Buttons don't move or glow.
- **Color management:** AgX with the Medium High Contrast look, Cycles, denoised, with the GPU when available. The look matches `renders/card.png` and the owner's photographs: tall strip softboxes for key, fill and rim over a world that is dark except overhead, so the near-black satin body carries clean vertical highlights, the ring reads as gloss black and the bowl as mirror-polished steel. Those finishes are applied in `render.py` (`FINISHES`), not saved in the model.

## Outputs

`assets/renders/<view>[-<state>].png` and `<view>[-<state>]-cutout.png`, plus `<view>.json` for views with hotspots. They are committed and listed in `assets/README.md`. Unless a row says otherwise, images are square RGBA, transparent, with a contact shadow (the existing shadow catcher); each `-cutout` image is the same render with the catcher hidden, so it has no shadow, and shares its view's hotspots. A UI image is 1024 px and a marketing image the size given in its row. PNGs are optimized losslessly (`oxipng -o 4`); the app's build makes WebP, not this script.

## The set

### Priority 1: the app

| View / state | Purpose | Framing |
| --- | --- | --- |
| `card` (exists) | Device card on light grounds | Three-quarter front-left, about 14° up |
| `card-dark` | Device card on navy (#0B1424) | Same camera as `card`, with a stronger rim light so the charcoal body separates from navy. Check it on #0B1424 |
| `front` | Large status view and food-level states | Straight on, slightly above, the whole feeder centered |
| `front-food-full`, `-half`, `-low`, `-empty` | Food level in the window | `front`'s camera. Change `HopperFill` (scale, or hide it for empty) so the window reads at a glance; low means kibble at the bottom of the window only |
| `panel` | Close-up of the control ring for setup and button help | Front, tight on the ring, every icon legible at 390 px wide. Needs [hotspots](#hotspots) |
| `top-lid-open` | "Refill the hopper" | High three-quarter with the `Lid` lifted and set aside (or hidden if it can't be placed believably), the food visible |
| `bowl-out` | "Wash the bowl" or "Put the bowl back" | `card`'s camera with the bowl moved forward and down to the surface, the outlet visible |
| `outlet` | Jam and blocked-outlet help | Low, close on the outlet opening above the bowl |
| `back` | "Plug in the adapter" | Rear three-quarter. Report whether the power jack is modeled |
| `bottom` | "Insert batteries" | Underside, tilted. Report whether the battery door is modeled |

### Priority 2: README, releases and store

| View / state | Purpose | Size |
| --- | --- | --- |
| `hero` | README banner and landing page: the feeder on a navy stage (#0B1424 to #13213A falloff, a soft floor reflection), placed right of center, the left 55% empty for type | 3840 × 1920, opaque |
| `social` | GitHub social preview, same stage, the feeder centered right | 1280 × 640, opaque |
| `feature` | Store feature graphic, same stage | 1024 × 500, opaque |
| `turntable` | Optional: a 360° loop for the landing page, 48 frames, on `card`'s elevation | 1080 × 1080 PNG frames; the encode is separate |

Lettering is never rendered. Layouts add the lockup from `assets/brand/` afterwards.

### Priority 3: docs

| View / state | Purpose |
| --- | --- |
| `exploded` | Hardware docs: hopper, ring, PCB and base pulled apart on one axis, the PCB facing camera. Transparent |
| `pcb` | The board alone, top-down. The texture is synthetic; label nothing |

## Line art

`lineart.py` traces the `card` view into `renders/card-lines.json` for the website's hero: the visible silhouettes, creases and open edges as 2D paths grouped by part (body, bowl, window, ring icons, food), and the visible hotspots in the same units. It imports `render.py` for the camera, unbranding and hotspot projection, so the lines and the `card` render agree. It traces with the model's bevel modifiers off, so a crease is the designed sharp edge rather than a strip of shallow bevel facets that would draw as dashes. Edges are chained into lines that run straight on, and visibility is a ray cast per sample along each whole line; a hidden run of 20 or fewer samples between visible ones is treated as noise and drawn. The output is deterministic, so a retrace without a model change leaves no diff.

## Hotspots

For `panel`, `front` and `card`, write `<view>.json` beside the image: each named part's projected center and radius, normalized to the image (origin top left, 0–1). The app draws highlights, pulses and LED colors over the render instead of needing a render per state.

```json
{
  "view": "panel",
  "size": [1024, 1024],
  "hotspots": {
    "button_reset":    { "x": 0.31, "y": 0.52, "r": 0.03, "visible": true },
    "button_mic":      { "x": 0.0, "y": 0.0, "r": 0.0, "visible": true },
    "button_dispense": { "x": 0.0, "y": 0.0, "r": 0.0, "visible": true },
    "button_lock":     { "x": 0.0, "y": 0.0, "r": 0.0, "visible": true },
    "led_connection":  { "x": 0.0, "y": 0.0, "r": 0.0, "visible": true },
    "led_food":        { "x": 0.0, "y": 0.0, "r": 0.0, "visible": true },
    "led_lock":        { "x": 0.0, "y": 0.0, "r": 0.0, "visible": true },
    "led_fault":       { "x": 0.0, "y": 0.0, "r": 0.0, "visible": true },
    "window":          { "x": 0.0, "y": 0.0, "r": 0.0, "visible": true },
    "outlet":          { "x": 0.0, "y": 0.0, "r": 0.0, "visible": true },
    "bowl":            { "x": 0.0, "y": 0.0, "r": 0.0, "visible": true }
  }
}
```

(The values above are placeholders.)

- **Projection:** `bpy_extras.object_utils.world_to_camera_view` on each part's bounding-box center. `r` is half the projected bounding box's larger side. `visible` is a ray cast from the camera that reaches the part unoccluded.
- **Parts:**
  - The **buttons** are Reset, Mic, Dispense and Lock (firmware `buttons.md`, physical).
  - The **indicators** are connection, food warning, lock and fault (hardware README).
  - The model's ring icons are `Icon_Reset`, `Icon_Mic`, `Icon_Portion`, `Icon_Lock`, `Icon_WiFi`, `Icon_Warn`, `Icon_Bowl` and `Icon_LockButton`. Map them by inspecting the model, and record the mapping in `render.py`.
  - If an indicator has no matching object, leave it out of the JSON and say so. Never place one by guesswork.

## Acceptance

- `--all` regenerates every image and JSON from a clean checkout, and a second run produces visually identical images.
- Each set's states line up pixel for pixel outside the part that changes (check with a difference image).
- `card-dark` and `hero` read clearly on #0B1424. No transparent image has halos or a gray fringe on navy or white.
- Hotspot circles drawn over `panel.png` land on the right icons. Commit that overlay check as a scratch image, not under `renders/`.
- The report lists anything the model lacked, any icon-to-part mapping that remains uncertain, and the render time per image.

## Status

| Set | Status | Notes |
| --- | --- | --- |
| Priority 1: the app | Partial | Rendered, but predates the model's current proportions and icon placement; needs regenerating |
| Priority 2: README, releases and store | Not started | |
| Priority 3: docs | Not started | |

Open items for the owner:

- **Model gaps.** The power port is a USB-C receptacle (`PowerPort`) low on the back, placed from photographs. `PCB_USBC` still sits inside the front of the band. There is no battery door (the underside is a flat disc), so `bottom` can't yet show one. `HopperFill` has no material; `render.py` gives it `PL_Kibble`.
- **Lit icons.** The model lights `Icon_Lock` and `Icon_WiFi` blue and prints `Icon_Bowl` and `Icon_Warn` grey. Neither matches the device, whose unlit indicators are dark, so `render.py` gives all four a near-black satin finish (`UNLIT_INDICATOR`). The buttons' icons keep the model's white print.
- **Icon order.** Seen from the front, the buttons run Lock, Dispense, Mic, Reset, and the indicators low food, lock, Wi-Fi, error/warning, each from left to right. The model's ring matches: `Icon_LockButton` and `Icon_Portion` at the left, the four indicators grouped in the middle, `Icon_Mic` and `Icon_Reset` at the right.
- **Uncertain mapping.** `led_food` is `Icon_Bowl` by elimination.
- **Panel legibility.** The icons span 0.17 m of ring, so even at the tightest square frame each is about 10 px wide at 390 px.

## Evidence

- **Modeled.** The Priority 1 images were rendered on October 23, 2026. They are renders of the model, not photographs.
- **Observed.** The button and indicator order comes from the owner, October 24, 2026. The ring's match to it was checked against owner photographs on October 27, 2026.

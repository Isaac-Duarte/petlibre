# Assets

This folder holds the brand and product imagery for the app, the web UI and the docs. Everything here is ours: the renders come from our own 3D model, not vendor photos.

| Path | Contents |
| --- | --- |
| `model/plaf103.blend` | The owner's Blender 5.2 model of the PETLIBRO Granary PLAF103 (meters, Z up, the bowl toward −Y). It holds the product only, with no cameras, lights or stage. Its PCB texture is synthetic and shows no device identifiers |
| `model/render.py` | Builds the stage and renders every image under `renders/` |
| `model/lineart.py` | Traces the `card` view into line art for the website (`renders/card-lines.json`) |
| `model/RENDERS.md` | The spec for every render the app, web and docs need |
| `model/checks/` | Hotspot overlays for checking the hotspot JSON by eye; not for use in the app |
| `renders/` | Generated, committed images, so the app and web build without Blender |
| `brand/` | The logo, app and Android icons, and design tokens ([brand](brand/README.md)) |

## License

The visual assets here (the model, renders, logos and other brand artwork) are licensed under [Creative Commons Attribution 4.0](https://creativecommons.org/licenses/by/4.0/) (CC BY 4.0). Credit "Wellfed contributors". Code, including `model/render.py` and `model/lineart.py`, stays under the repository's MIT license.

PETLIBRO and Granary are the vendor's names, used here only to say which hardware the firmware runs on. The license covers our artwork, not their marks or industrial design.

## Rendering

```sh
blender -b assets/model/plaf103.blend -P assets/model/render.py -- --all --check
blender -b assets/model/plaf103.blend -P assets/model/render.py -- --view front --state food-half
```

`--all` renders every image below, each in a fresh Blender. `--check` also draws the hotspot circles over `card`, `front` and `panel` into `model/checks/`, so you can see that each lands on its part.

Each image is a 1024 px transparent PNG with a contact shadow, plus a `-cutout` copy without the shadow (`card-cutout.png`, `front-food-half-cutout.png`). Each is optimized with `oxipng` when it is on `PATH`. An RTX A6000 on OptiX renders each in about 10 s at the default 512 samples; a CPU takes minutes.

| Image | Use |
| --- | --- |
| `card`, `card-dark` | The device card on light grounds and on navy (#0B1424). One camera; the dark one has a stronger rim |
| `front`, `front-food-full`, `-half`, `-low`, `-empty` | The status view and its food levels. One camera; only the window changes |
| `panel` | The control ring, for setup and button help |
| `top-lid-open` | Refilling the hopper, the lid set down beside it |
| `bowl-out` | Washing or replacing the bowl (the card's camera) |
| `outlet` | Jam and blocked-outlet help |
| `back`, `bottom` | Rear three-quarter, showing the USB-C power port, and underside. The model has no battery door yet |
| `card-lines.json` | The website hero's line drawing: the `card` view traced by `lineart.py` into 2D paths and hotspots, not an image. Retrace with `blender -b assets/model/plaf103.blend -P assets/model/lineart.py` |

`card.json`, `front.json` and `panel.json` hold the hotspots: each button's, indicator's, window's, outlet's and bowl's projected center and radius (normalized, origin top left), and whether it is visible. The app uses them to draw highlights and indicator light over the image ([hotspots](model/RENDERS.md#hotspots)). Every state of a view, and every cutout, shares its JSON. Every indicator is rendered unlit.

**Every render is unbranded.** The script hides the hopper lettering and the PCB label, so no image suggests this project makes the hardware.

- Name the device only as "PETLIBRO Granary PLAF103", in compatibility text.
- Keep the "not affiliated" line wherever the device is shown with our brand.

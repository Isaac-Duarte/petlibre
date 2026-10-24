# Assets

Brand and product imagery for the app, the web UI and the docs.

| Path | Contents |
| --- | --- |
| `model/plaf103.blend` | The owner's Blender 5.2 model of the PETLIBRO Granary PLAF103 (meters, Z up, the bowl toward −Y). It holds the product only, with no cameras, lights or stage. Its PCB texture is synthetic and shows no device identifiers |
| `model/render.py` | Builds the stage and renders every image under `renders/` |
| `model/RENDERS.md` | The spec for every render the app, web and docs need |
| `model/checks/` | Hotspot overlays for checking the hotspot JSON by eye; not for use in the app |
| `renders/` | Generated, committed images, so the app and web build without Blender |
| `brand/` | The logo, app and Android icons, and design tokens ([brand](brand/README.md)) |

## License

The visual assets here (the model, renders, logos and other brand artwork) are licensed under [Creative Commons Attribution 4.0](https://creativecommons.org/licenses/by/4.0/) (CC BY 4.0). Credit "Wellfed contributors". Code, including `model/render.py`, stays under the repository's MIT license.

PETLIBRO and Granary are the vendor's names, used here only to say which hardware the firmware runs on. The license covers our artwork, not their marks or industrial design.

## Rendering

```sh
blender -b assets/model/plaf103.blend -P assets/model/render.py -- --all --check
blender -b assets/model/plaf103.blend -P assets/model/render.py -- --view front --state food-half
```

`--all` renders every image below, each in a fresh Blender. Output is a 1024 px transparent PNG with a contact shadow, optimized with `oxipng` when it is on `PATH`. An RTX A6000 on OptiX renders each in about 10 s at the default 512 samples; a CPU takes minutes. `--check` also draws the hotspot circles over `card`, `front` and `panel` into `model/checks/`, for checking that each lands on its part.

| Image | Use |
| --- | --- |
| `card`, `card-dark` | The device card on light grounds and on navy (#0B1424). One camera; the dark one has a stronger rim |
| `front`, `front-food-full`, `-half`, `-low`, `-empty` | The status view and its food levels. One camera; only the window changes |
| `panel` | The control ring, for setup and button help |
| `top-lid-open` | Refilling the hopper, the lid set down beside it |
| `bowl-out` | Washing or replacing the bowl (the card's camera) |
| `outlet` | Jam and blocked-outlet help |
| `back`, `bottom` | Rear three-quarter and underside. The model has no power jack or battery door yet, so neither image shows one |

`card.json`, `front.json` and `panel.json` give each button's, indicator's, window's, outlet's and bowl's projected center and radius (normalized, origin top left) and whether it is visible, so the app draws highlights and indicator light over the image ([hotspots](model/RENDERS.md#hotspots)). Every state of a view shares its JSON. Every indicator is rendered unlit.

**Every render is unbranded.** The script hides the hopper lettering and the PCB label, so no image suggests this project makes the hardware. Name the device only as "PETLIBRO Granary PLAF103" in compatibility text, and keep the "not affiliated" line wherever the device is shown with our brand.

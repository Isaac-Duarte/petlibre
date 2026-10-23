# Assets

Brand and product imagery for the app, the web UI and the docs.

| Path | Contents |
| --- | --- |
| `model/plaf103.blend` | The owner's Blender 5.2 model of the PETLIBRO Granary PLAF103 (meters, Z up, the bowl toward −Y). It holds the product only, with no cameras, lights or stage. Its PCB texture is synthetic and shows no device identifiers |
| `model/render.py` | Builds the stage and renders every image under `renders/` |
| `renders/` | Generated, committed images, so the app and web build without Blender |
| `brand/` | The logo, palette and type, once a direction is chosen |

## License

The visual assets here (the model, renders, logos and other brand artwork) are licensed under [Creative Commons Attribution 4.0](https://creativecommons.org/licenses/by/4.0/) (CC BY 4.0). Credit "Petlibre contributors". Code, including `model/render.py`, stays under the repository's MIT license.

PETLIBRO and Granary are the vendor's names, used here only to say which hardware the firmware runs on. The license covers our artwork, not their marks or industrial design.

## Rendering

```sh
blender -b assets/model/plaf103.blend -P assets/model/render.py -- --view card --size 1024 --samples 512
```

Views are `card` (the device card's three-quarter shot), `front` and `panel` (the control ring, for setup steps). Output is a transparent PNG with a contact shadow. A GPU renders a 1024 px card in seconds; a CPU takes minutes.

**Every render is unbranded.** The script hides the hopper lettering and the PCB label, so no image suggests this project makes the hardware. Name the device only as "PETLIBRO Granary PLAF103" in compatibility text, and keep the "not affiliated" line wherever the device is shown with our brand.

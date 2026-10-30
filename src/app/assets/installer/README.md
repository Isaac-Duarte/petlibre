# The image the APK installs

The install from stock serves this folder's image to the feeder. Its files are a build-time input and never go into Git:

| File | From |
| --- | --- |
| `review.json`, `build-manifest.json`, `image.bin` | A frozen review of the consumer image |
| `update.manifest` | That build's update manifest, signed with the key its image trusts |

`make app-installer-input REVIEW=<dir> MANIFEST=<file>` copies them here and prints the `WELLFED_INSTALLER_PIN` to build with. Without them, the app builds and says it has no firmware to install. See [the app's README](../../README.md#the-image-it-installs).

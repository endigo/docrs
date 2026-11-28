# Image fixture provenance

Images from real encoders, 48 × 32 pixels, for `crates/docrs/tests/pdf_images.rs`. The PNG, GIF and BMP cases that cover every colour type, depth and layout are generated in that test instead.

All are made from drawings in `tools/pdf/image_fixtures.swift` (a colour gradient with a dark disc and a white bar; a logo; three frames of a moving square), on macOS 27.0 (26A428):

```sh
swift tools/pdf/image_fixtures.swift OUT
cjpeg -quality 90 OUT/photo.pgm > photo-grey.jpg
cjpeg -quality 90 OUT/photo.ppm > photo-rgb.jpg
cjpeg -quality 90 -progressive OUT/photo.ppm > photo-progressive.jpg
```

| File | Encoder | What it is |
| -- | -- | -- |
| `photo-grey.jpg` | libjpeg-turbo 3.2.1 `cjpeg` | baseline, one component, JFIF |
| `photo-rgb.jpg` | libjpeg-turbo 3.2.1 `cjpeg` | baseline, three components, JFIF |
| `photo-progressive.jpg` | libjpeg-turbo 3.2.1 `cjpeg -progressive` | progressive (SOF2) |
| `photo-cmyk.jpg` | ImageIO, Generic CMYK | four components with Adobe's APP14 marker (transform 0); its 55 KB ICC profile (APP2) removed, nothing else changed |
| `photo-p3.jpg` | ImageIO, Display P3 | an embedded ICC profile |
| `logo.png` | ImageIO | 8-bit RGBA with anti-aliased edges and a half-transparent bar |
| `animated.gif` | ImageIO | three frames, looping |

| File | SHA-256 |
| -- | -- |
| `animated.gif` | `a997974ac52b398d3beca02926401e1853b29e80020267697b8fb116fd916840` |
| `logo.png` | `42daad9717edd8da478dda647528fdc98b404e86dce4f464db751237524ea5b8` |
| `photo-cmyk.jpg` | `1c2324382f79c0d176ac90c2b0954a06f870b0c1ff3c0d213f41f38054640252` |
| `photo-grey.jpg` | `c8e3bd558d18c978057eb4fd3e465a7ab55ed026f8b54f6476ea9f587ba9f53e` |
| `photo-p3.jpg` | `c1fbfa127869b0420fe7421f23565a258bfb05c411d2a2b4c9574665b3e05be5` |
| `photo-progressive.jpg` | `5731c5ca44e91008edc192a9c593f97077a260005447260fa021fe5f9d777bd1` |
| `photo-rgb.jpg` | `c472f91eed76e5a870b63c2524b8d211b06c91b999e6ba2bfb1f022ee2a1e763` |

No encoder on this machine writes a YCCK JPEG, so YCCK is refused (`pdf_image_unsupported`) until one is tested.

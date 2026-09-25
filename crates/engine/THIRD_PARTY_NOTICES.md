# Third-party notices

The [MIT license](../../LICENSE) covers Clipward-owned clipping engine source. The following bundled files have separate terms. Keep their license files when redistributing the engine or an app that includes it.

| Component | Included files | Source and terms |
| --- | --- | --- |
| Montserrat | `assets/fonts/Montserrat-Black.ttf`, `Montserrat-ExtraBold.ttf` | [Google Fonts Montserrat](https://github.com/google/fonts/tree/main/ofl/montserrat), SIL Open Font License 1.1; text in `assets/fonts/Montserrat-OFL.txt`. |
| Anton | `assets/fonts/Anton-Regular.ttf` | [Google Fonts Anton](https://github.com/google/fonts/tree/main/ofl/anton), SIL Open Font License 1.1; text in `assets/fonts/Anton-OFL.txt`. |
| Archivo Black | `assets/fonts/ArchivoBlack-Regular.ttf` | [Google Fonts Archivo Black](https://github.com/google/fonts/tree/main/ofl/archivoblack), SIL Open Font License 1.1; text in `assets/fonts/ArchivoBlack-OFL.txt`. |
| Instrument Serif | `assets/fonts/InstrumentSerif-Italic.ttf` | [Google Fonts Instrument Serif](https://github.com/google/fonts/tree/main/ofl/instrumentserif), SIL Open Font License 1.1; text in `assets/fonts/InstrumentSerif-OFL.txt`. |
| Poppins | `assets/fonts/Poppins-Black.ttf`, `Poppins-ExtraBold.ttf` | [Google Fonts Poppins](https://github.com/google/fonts/tree/main/ofl/poppins), SIL Open Font License 1.1; text in `assets/fonts/Poppins-OFL.txt`. |
| Plus Jakarta Sans | `assets/fonts/PlusJakartaSans.ttf` | [Plus Jakarta Sans](https://github.com/tokotype/PlusJakartaSans), SIL Open Font License 1.1; text in `assets/fonts/PlusJakartaSans-OFL.txt`. |
| YuNet face detector | `assets/models/face_detection_yunet_2023mar.onnx` | [OpenCV Zoo YuNet](https://github.com/opencv/opencv_zoo/tree/main/models/face_detection_yunet), MIT; text in `assets/models/YuNet-LICENSE`. The model file was checked byte for byte against the upstream copy during this review. |

Rust dependencies are pinned in the workspace `Cargo.lock`; each crate keeps its own license. The desktop release also includes yt-dlp, FFmpeg and their notices; see the root `THIRD_PARTY_NOTICES.md` and `docs/RELEASING.md`.

No third-party platform logos are bundled. Names and trademarks remain with their owners. Before a public binary release, review the actual built package, dependency license inventory and ownership of original source and artwork.

# reglinux-burner

Slint-based GUI that downloads REG Linux images and flashes them using the embedded
USBImager 1.0.10 engine.

## Notes
- Downloads are handled by `reglinux-fetch` (NDJSON progress parsing).
- Flashing uses the embedded USBImager engine (MIT license).
- USBImager license text: `LICENSES/USBImager-MIT-LICENSE.txt`.

## Build (Linux)
```bash
cargo build -p reglinux-burner
```

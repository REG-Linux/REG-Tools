# reglinux-burner

Slint-based GUI that downloads REG Linux images and flashes them using the embedded
USBImager 1.0.10 engine.

## Notes
- Downloads are handled by `reglinux-fetch` (NDJSON progress parsing).
- Flashing uses the embedded USBImager engine (MIT license).
- USBImager license text: `LICENSES/USBImager-MIT-LICENSE.txt`.

## Smoke test
List devices without flashing:
```bash
reglinux-burner --list-devices
```

## Privileges
Flashing raw disks requires elevated privileges:
- Windows: run as Administrator.
- macOS/Linux: run as root (e.g. `sudo`).
If not elevated, the GUI will show a friendly error and refuse to start flashing.

## Build (Linux/macOS/Windows)
```bash
cargo build -p reglinux-burner
```

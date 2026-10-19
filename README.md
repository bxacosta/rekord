# rekord

Terminal app to monitor and record a microphone in real time, with optional
noise reduction and voice enhancement.

- Noise reduction with [DeepFilterNet](https://github.com/Rikorose/DeepFilterNet).
- Voice enhance: EQ and compression to make speech easier to understand.
- Recording with pause and resume. Files stay playable if the app is killed,
  and interrupted files are repaired on the next start.
- Reconnects automatically when a device is unplugged or the system default
  changes. Losing the output does not interrupt the recording.

## Install

Download `rekord-<version>-windows-x64.exe` from the
[releases](../../releases/latest) page. It is a single file with no
dependencies; rename it to `rekord.exe` and put it in a folder on your `PATH`.

## Usage

```sh
rekord                 # monitor with the saved settings
rekord -r              # start recording immediately
rekord -i anker -o off # microphone matching "anker", no monitoring
rekord -e -n           # voice enhance and noise reduction on
rekord devices         # list microphones and outputs
```

| Key | Action |
|---|---|
| `space` | Record / pause / resume |
| `s` | Stop recording |
| `e` / `n` | Voice enhance / noise reduction |
| `[` `]` | Noise reduction strength |
| `↑` `↓` | Gain |
| `m` | Mute monitoring |
| `i` / `o` | Choose microphone / output |
| `q` | Quit |

Recordings are saved to `Music\Rekord`. See [docs/usage.md](docs/usage.md) for
all options, the config file and details on recording and devices.

Design notes for anyone changing the code are in
[docs/architecture.md](docs/architecture.md).

## Building

```sh
cargo build --release
cargo test
```

Releases are built by GitHub Actions; see [docs/publishing.md](docs/publishing.md).

## License

[MIT](LICENSE)

# rekord

Listen to a microphone live with a clearer voice, and record it without ever losing a take.

rekord was built for Bluetooth microphones and headsets: their audio is quiet,
muddy and noisy. rekord boosts it, removes background noise with a neural
denoiser, shapes the voice so words are easy to understand, and plays it back to
you in real time while (optionally) recording it.

```text
╭ rekord ───────────────────────────────────────────────────────────── ● LIVE ╮
│                                                                             │
│  Mic      Microphone (Anker PowerConf C200)                         48 kHz  │
│  Listen   Speakers (Qudelix-5K USB DAC 96KHz)                           on  │
│                                                                             │
│  Level    ■■■■■■■■■■■■■■■■■■■■■■■■■■■■■■■■■■■■■·····│············   -18 dB  │
│                                                                             │
│  Gain     +12 dB                                                            │
│  Enhance  ● on                                                              │
│  Denoise  ● on    ▰▰▰▱▱▱▱ 18 dB                                             │
│ ─────────────────────────────────────────────────────────────────────────── │
│  ● REC     00:12:34                                               138.3 MB  │
│  2026-10-17_22-58-01.wav  in C:\Users\me\Music\Rekord                       │
│ ─────────────────────────────────────────────────────────────────────────── │
│  ✓ Saved 00:05:12 recording: 2026-10-17_22-40-03.wav                        │
╰─────────────────────────────────────────────────────────────────────────────╯

       space pause   s stop   e enhance   n denoise   [ ] strength
              ↑↓ gain   m mute   i mic   o output   q quit
```

## Features

- **Live monitoring** with low latency, on any output device.
- **Voice enhance**: EQ that removes rumble, boxiness and nasal tone and lifts
  the frequencies where speech is understood, plus gentle compression.
- **Noise reduction** with [DeepFilterNet](https://github.com/Rikorose/DeepFilterNet),
  with adjustable strength.
- **Recording** with start, pause, resume and stop.
- **Never lose a take**: files are kept valid on disk while recording; a crash,
  a killed process or a closed window loses at most half a second, and
  interrupted files are repaired automatically on the next start.
- **Resilient devices**: follows the system default, falls back to it when your
  chosen device is unplugged, and reconnects by itself when it comes back. A
  lost output never interrupts the microphone or the recording.
- **Remembers your setup**: devices, gain and effects chosen in the app are
  restored next time.

## Usage

```sh
rekord                 # start listening with your saved setup
rekord -r              # ...and start recording right away
rekord -i anker -o off # use the mic whose name contains "anker", don't listen
rekord -e -n           # enable voice enhance and noise reduction
rekord -r -t 45m       # record for 45 minutes, then quit
rekord devices         # list microphones and outputs
```

| Option | |
|---|---|
| `-i, --input <NAME>` | Microphone to use: any part of its name |
| `-o, --output <NAME>` | Where to listen: any part of a device name, or `off` |
| `-r, --record` | Start recording right away |
| `-e, --enhance[=on\|off]` | Voice enhance |
| `-n, --denoise[=on\|off]` | Noise reduction |
| `-g, --gain <DB>` | Input gain, from -12 to 30 dB |
| `-d, --dir <FOLDER>` | Folder for recordings (default `Music\Rekord`) |
| `-t, --time <DURATION>` | Quit after this long: `90s`, `45m`, `1h30m` |

Command-line options apply to that run only. Whatever you change inside the app
is remembered.

### Keys

| Key | Action |
|---|---|
| `space` | Start recording, pause, resume |
| `s` | Stop and save the recording |
| `e` | Voice enhance on/off |
| `n` | Noise reduction on/off |
| `[` `]` | Noise reduction strength |
| `↑` `↓` | Gain |
| `m` | Mute/unmute what you hear (the recording is not affected) |
| `i` / `o` | Choose the microphone / the output |
| `q` | Quit (a running recording is saved) |

### Without a terminal

When the output is not a terminal (redirected to a file, run from a script or a
service), rekord prints status lines instead of the interactive screen, and
reads commands from standard input, one per line: `record`, `pause`, `resume`,
`stop`, `enhance`, `denoise`, `mute`, `quit`.

```sh
rekord -r -t 2h > session.log
```

## Recordings

Each take produces two 48 kHz, 16-bit mono WAV files:

| File | Content |
|---|---|
| `2026-10-17_22-58-01.wav` | What you heard: gain, noise reduction and enhance applied |
| `2026-10-17_22-58-01_original.wav` | The untouched microphone signal, to re-process later |

Pauses are left out of the files. Very long takes continue in `_part2`, `_part3`…
files before reaching the 4 GB limit of the WAV format.

## Settings

Saved in `%APPDATA%\rekord\config.toml` (set `REKORD_CONFIG` to use another
file). It is plain TOML; besides what the app saves, you can set
`recordings_dir = 'D:\Audio'` to change the default recordings folder.

## Building

Requires a recent stable Rust toolchain.

```sh
cargo build --release   # target/release/rekord.exe
cargo test
```

## Architecture

```text
src/
├── main.rs        startup: options, settings, recovery, front end selection
├── cli.rs         command-line definition
├── app.rs         application state and actions, shared by both front ends
├── settings.rs    user settings and the config file
├── console.rs     plain console output (device list, exit summary)
├── shutdown.rs    graceful exit when the console window is closed
├── format.rs      human-friendly durations, sizes and paths
├── audio/         everything that talks to hardware
│   ├── devices.rs   enumeration and choosing a concrete device
│   ├── engine.rs    supervisor thread: connects, follows changes, reconnects
│   ├── session.rs   one live connection and its processing thread
│   ├── streams.rs   device callbacks (they only move samples)
│   └── controls.rs  lock-free settings and meters shared with the UI
├── dsp/           voice processing, independent of devices
│   ├── mod.rs       the chain: gain → limiter → denoise → enhance → limiter
│   ├── denoise.rs   DeepFilterNet
│   ├── enhance.rs   voice EQ and compression
│   └── …            filters, dynamics, resampling, click-free ramps
├── recording/     takes on a writer thread; crash-safe WAV; recovery
└── ui/            interactive screen (ratatui) and plain line mode
```

Audio flows through three kinds of threads. Device callbacks only move samples
into and out of lock-free ring buffers. A processing thread per session
resamples to 48 kHz, runs the voice chain, feeds the recorder and the output.
The recorder thread owns the files, so the disk can never stall the audio.

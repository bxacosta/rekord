# Usage

- [Command line](#command-line)
- [Interactive mode](#interactive-mode)
- [Plain mode](#plain-mode)
- [Audio processing](#audio-processing)
- [Recording](#recording)
- [Devices](#devices)
- [Configuration](#configuration)
- [Common cases](#common-cases)

## Command line

```
rekord [OPTIONS]
rekord devices
```

| Option | Description |
|---|---|
| `-i, --input <NAME>` | Microphone to use. Matches any part of the device name, case-insensitive. |
| `-o, --output <NAME>` | Output to monitor on. Same matching as `--input`; `off` disables monitoring. |
| `-r, --record` | Start recording immediately. |
| `-e, --enhance[=on\|off]` | Voice enhance. `-e` alone means `on`. |
| `-n, --denoise[=on\|off]` | Noise reduction. `-n` alone means `on`. |
| `-g, --gain <DB>` | Input gain, from -12 to 30 dB. |
| `-d, --dir <FOLDER>` | Folder for recordings. |
| `-t, --time <DURATION>` | Quit after this time: `90` or `90s`, `45m`, `1h30m`. A running recording is saved. |
| `-h`, `-V` | Help, version. |

`rekord devices` lists microphones and outputs, and marks the one in use and the
system default.

Command-line options apply to the current run only and are not saved. Changes
made inside the app are saved (see [Configuration](#configuration)).

If a name given to `-i` or `-o` matches no device, rekord exits with an error.

## Interactive mode

Used when rekord runs in a terminal.

| Key | Action |
|---|---|
| `space` (or `r`, `p`) | Start recording; pause / resume while recording |
| `s` | Stop and save the recording |
| `e` | Voice enhance on/off |
| `n` | Noise reduction on/off |
| `[` / `]` | Lower / raise noise reduction strength (also turns it on) |
| `↑` / `↓` (or `+` / `-`) | Gain up / down, in 2 dB steps |
| `m` | Mute / unmute monitoring. The recording is not affected. |
| `i` | Choose the microphone |
| `o` | Choose the output |
| `q`, `Esc`, `Ctrl+C` | Quit. A running recording is saved. |

In the device list: `↑` `↓` to move, `Enter` to choose, `1`–`9` to choose
directly, `Esc` to cancel. Choosing an output turns monitoring back on.

The screen shows:

- **Mic / Listen**: devices in use, the microphone sample rate and whether
  monitoring is on, muted or off.
- **Level**: output level with a short peak hold. Green up to -12 dB, yellow up
  to -3 dB, red above.
- **Gain, Enhance, Denoise**: current processing settings.
- **Recording**: state, length (pauses excluded), size and file.
- **Message line**: the most relevant of: errors, recent events (recording
  saved, file recovered), device warnings, dropouts.

Dropouts are counted separately:

- `mic`: microphone audio was dropped because processing fell behind. This
  audio is missing from the recording.
- `output`: the output ran out of audio. Only affects what is heard.

On exit, rekord prints the recordings saved during the session.

## Plain mode

Used when the output is not a terminal (redirected to a file, run from a
script). rekord prints a line on every state change and a status line every
5 seconds:

```
[22:58:01] Live: Microphone (USB Audio) (48000 Hz) → not listening
[22:58:01] Recording to C:\Users\me\Music\Rekord\2026-10-17_22-58-01.wav
[22:58:06] level  -24 dB · gain +12 dB · enhance on · denoise on · dropouts mic 0 / output 0 · REC 00:00:05 (938 KB)
```

Commands are read from standard input, one per line:

| Command | Action |
|---|---|
| `record` (`r`) | Start recording |
| `pause` (`p`) | Pause |
| `resume` | Resume |
| `stop` (`s`) | Stop and save |
| `enhance` (`e`) | Toggle voice enhance |
| `denoise` (`n`) | Toggle noise reduction |
| `mute` (`m`) | Toggle monitoring |
| `quit` (`q`, `exit`) | Quit |

`Ctrl+C` also quits and saves the recording.

Colors are disabled when the output is not a terminal or `NO_COLOR` is set.

## Audio processing

All processing runs at 48 kHz, mono. Stereo microphones are mixed down. The
chain is:

```
gain → limiter → noise reduction → voice enhance → limiter
```

- **Gain** is applied first, so the noise reduction works on the final level.
  Some microphones (Bluetooth headsets in particular) deliver very quiet audio
  that DeepFilterNet would otherwise treat as silence.
- **Noise reduction** uses DeepFilterNet. Its strength is the maximum
  attenuation of noise: 6, 12, 18, 24, 30, 40 dB or `max` (no limit). Lower
  values sound more natural; higher values remove more noise but can affect the
  voice. The model keeps running while disabled, so it is already adapted to the
  room when turned on.
- **Voice enhance** is a fixed EQ (high-pass at 100 Hz, cuts at 300 Hz and
  1 kHz, boost at 3 kHz, high shelf at 5 kHz) followed by a compressor with
  makeup gain.
- **Limiters** prevent clipping.

Toggling an effect, changing the gain or muting fades over 10 ms to avoid clicks.

Monitoring adds about 20–60 ms of latency, plus whatever the devices add. Noise
reduction adds 30 ms while it is on. Turning it on or off causes a short dip in
the sound; turning it off during a recording skips 30 ms of audio.

## Recording

Each recording creates two 16-bit, 48 kHz mono WAV files:

| File | Content |
|---|---|
| `2026-10-17_22-58-01.wav` | Processed audio, as heard |
| `2026-10-17_22-58-01_original.wav` | Microphone audio before processing (resampled to 48 kHz) |

Both files have the same length and are aligned, so the original can be
processed again later.

- **Pause** stops writing; paused time is not in the files.
- **Folder**: `Music\Rekord` by default. Change it per run with `--dir`, or
  permanently with `recordings_dir` in the config file.
- **Names**: the start time. If a file with that name exists, `_2`, `_3`… is
  added. Existing files are never overwritten.
- **Long recordings**: WAV files are limited to 4 GB, so after about 11 hours a
  recording continues in `_part2`, `_part3`… files.
- **Device changes**: the recording continues in the same file when the
  microphone or output changes. While no microphone is connected nothing is
  written, so that time is skipped.

### Crash safety

Files are written on a separate thread, so disk delays never affect the audio.

- The WAV header is updated twice a second, so the file is always playable. If
  the process is killed, at most the last half second is lost.
- Data is forced to disk every 5 seconds and on pause, which also covers power
  loss.
- Closing the console window, logging off or shutting down Windows saves the
  recording before exiting.
- On start, rekord checks the recordings folder and repairs files whose header
  does not match their content (audio written after the last update). Files not
  written by rekord are not touched.
- If writing fails (for example, the disk is full), recording stops, the error
  is shown and repeated on exit, and the audio written so far is kept.

## Devices

The microphone and the output are each either the **system default**, a
**specific device** or, for the output only, **off**.

- **System default** follows Windows: when the default device changes, rekord
  switches to the new one.
- **Specific device**: if it is not connected, rekord uses the system default
  and shows a warning. When the device is connected again, rekord switches back.
- Devices are checked every 2 seconds.

Failures:

- **Microphone stops sending audio** (unplugged, out of range): rekord
  reconnects. A microphone has 15 seconds to deliver its first audio, as
  Bluetooth headsets take a few seconds to switch to hands-free mode.
- **Output fails**: only the output is reopened, every 2 seconds, until it
  works. The microphone and the recording keep running.
- **Microphone and output on the same Bluetooth headset**: monitoring is
  disabled. Using a Bluetooth headset's microphone puts it in call mode, where
  its normal output stops working. USB headsets and built-in audio are not
  affected.

## Configuration

Settings changed in the app are saved to:

```
%APPDATA%\rekord\config.toml
```

The `REKORD_CONFIG` environment variable sets another path.

Example:

```toml
gain_db = 12.0
enhance = true
denoise = true
denoise_db = 18.0
recordings_dir = 'D:\Audio\Recordings'

[input]
use = "device"
id = "wasapi:{0.0.1.00000000}.{8d0d8915-7db1-480c-95dc-0e19e03d94fb}"
name = "Headset (Earphones)"

[output]
use = "default"
```

| Key | Values | Default |
|---|---|---|
| `gain_db` | -12 to 30 | `12.0` |
| `enhance` | `true`, `false` | `false` |
| `denoise` | `true`, `false` | `false` |
| `denoise_db` | `6`, `12`, `18`, `24`, `30`, `40`, `100` (`100` = max) | `18.0` |
| `recordings_dir` | Folder path | `Music\Rekord` |
| `[input]`, `[output]` | See below | System default |

Devices:

| Value | Meaning |
|---|---|
| `use = "default"` | System default |
| `use = "device"` with `id` and `name` | A specific device. `id` identifies it; `name` is shown while it is disconnected. |
| `use = "off"` | No monitoring (output only) |

Device ids are easier to set from the app (`i` / `o`) than by hand.

Notes:

- Missing keys take their default values.
- Out-of-range values are adjusted: gain is clamped, `denoise_db` is rounded up
  to the next strength.
- If the file cannot be parsed, rekord starts with the defaults and shows a
  warning. The file is overwritten the next time a setting is changed in the app.
- `recordings_dir` is only set by hand; the app does not change it.
- Use single quotes for Windows paths (`'D:\Audio'`), or double the
  backslashes inside double quotes (`"D:\\Audio"`).

## Common cases

Record a meeting without monitoring, for at most 2 hours:

```sh
rekord -o off -r -t 2h
```

Monitor a quiet microphone with noise reduction, without changing the saved
settings:

```sh
rekord -i headset -g 20 -n
```

Record into a specific folder:

```sh
rekord -r -d "D:\Audio\Interviews"
```

Run unattended and keep a log:

```sh
rekord -o off -r -t 8h > session.log
```

Control a running session from a script by writing commands to its standard
input:

```sh
(echo record; sleep 600; echo stop; echo quit) | rekord -o off
```

Find the name to pass to `-i` / `-o`:

```sh
rekord devices
```

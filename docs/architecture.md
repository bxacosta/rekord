# Architecture

## Threads

| Thread | Does | Must not |
|---|---|---|
| Device callbacks | Move samples to/from a ring buffer | Allocate, lock, process |
| `processing` | Resample, DSP, feed recorder and output | Touch the disk or wait |
| `recorder` | Write WAV files | — |
| `engine` | Open, replace and watch devices; publish the device lists | — |
| Main | UI loop | Call device APIs (they can block for seconds) |

The processing thread lives as long as the engine. DeepFilterNet's model cannot
move between threads (it uses `Rc`), takes ~150 ms to load and adapts to the
room, so it must survive reconnections. Sessions plug their input and output
rings into it through a `Patcher`.

Threads communicate through `rtrb` rings, `mpsc` channels and atomics, so the
audio side never blocks. The input ring holds 1 s; it only fills if processing
stalls, which is the only case where recorded audio is lost.

## 48 kHz, 10 ms frames

Required by DeepFilterNet. Input is resampled to 48 kHz on entry and to the
output rate on exit. As a result `_original.wav` is resampled too, which keeps
it aligned with the processed file. Using another model with a different frame
size means changing `FRAME_LEN` and the places that assume 10 ms (ramps, monitor
fade, input chunk size).

## Controls

`Controls` and `Meters` are atomics owned by the `Engine`, not the `Session`, so
settings and counters survive reconnections. Processing reads them once per
frame; changes glide over one frame to avoid clicks.

A new parameter goes through `dsp::Params`, `Controls`, `Settings`, `Action` /
`App::update`, then keys, view and plain mode.

## Processing chain

```
gain → limiter → noise reduction → voice enhance → limiter
```

- Gain goes first because DeepFilterNet removes very quiet signals as
  background, and some microphones (Bluetooth headsets) are very quiet.
- The denoiser runs even when disabled. It adapts to the room over a few
  seconds, so starting it on demand would sound wrong at first.
- Its output lags 30 ms (STFT overlap plus lookahead). Crossfading it with the
  undelayed signal would repeat or skip audio, and delaying the dry signal would
  add 30 ms of latency for everyone, although noise reduction is optional. So
  the output has two paths, direct (off) and denoised (on, 30 ms later), and
  switching fades out, changes path and fades in. Turning it on plays the 30 ms
  as silence, marked `is_gap` so it is not recorded; turning it off skips them.
  The original sent to the recorder follows the same delay, so both files stay
  aligned.
- The enhancer is reset when off: its state adapts in milliseconds.
- Denoise strength changes are applied without a glide. Add one there if steps
  become audible.

## Sessions and engine

A `Session` is one microphone plus an optional output.

- Changing or losing the microphone restarts the session. The output is swapped
  inside the running session, so losing headphones never interrupts the
  microphone or a recording.
- Each stream has its own `Health`. Processing errors are reported to the input's
  (restarts the session) or the output's (replaces only the output).
- The `Recorder` belongs to `App`, so a recording continues across session
  restarts.
- Devices are polled every 2 s (`Supervisor::reconcile`) instead of using
  Windows notifications. One code path covers new defaults, reappearing devices,
  silent failures and retries. Each check scans once (`Inventory`) and publishes
  the lists, which the device picker reads instead of enumerating itself.
- A microphone with no data for 3 s is restarted; some drivers stop without an
  error. Before the first data the limit is 15 s, because Bluetooth headsets are
  silent while switching to hands-free mode.

## Monitoring latency

Input and output run on different clocks, so the output queue drifts. Without
adaptive resampling, two limits handle it, affecting only what is heard:

- `MAX_MONITOR_QUEUE` (60 ms): above it, frames are skipped.
- `OUTPUT_PREBUFFER` (20 ms): after running dry, the output waits for this much
  audio. Raise it if a device shows output dropouts.

Glitch-free monitoring would need drift compensation: adjust the output
resampler ratio based on the queue level (rubato's `Async` supports it).

## Devices

- The default device is looked up with `IMMDeviceEnumerator` and opened by id.
  cpal's own default uses `ActivateAudioInterfaceAsync`, which fails with
  `RPC_E_CHANGED_MODE` when COM is initialized in another mode.
- Devices are stored by id; the name is only for display while disconnected.
- `same_hardware` compares the text in parentheses (`Headset (Earphones)` vs
  `Headphones (Earphones)`) to avoid using a headset's mic and output together,
  which forces call quality. It is a heuristic; comparing
  `PKEY_Device_ContainerId` would be exact.

## Dropouts

cpal reports WASAPI's data discontinuity flag as an input xrun. Bluetooth
drivers raise it several times per second without losing samples, so it is
ignored. Counted: input ring overflow (once per episode), output xruns, and
output starvation after playback started.

## Recording

- Only the writer thread changes the recording state; the UI reads it from
  atomics, so it shows the real state. Commands (`Control`) and audio share one
  channel, so they stay in order, and each command is checked against the state
  when it is applied, not when it is sent. `Toggle` is resolved there too.
- Every 500 ms the WAV header is rewritten with the current length, so the file
  is always playable. `sync_data` runs every 5 s and on pause. Raw PCM finalized
  on exit, RF64 or chunk files were discarded: less compatible or not crash-safe.
- On startup `wav::repair` fixes headers behind the real length. It only touches
  files whose header matches exactly what `WavWriter` writes, so **changing the
  header format requires changing `repair`**.
- If finalizing fails, the take still lists its files: the audio is on disk
  and the next start repairs the headers.
- Files are created with `create_new` and never overwritten.
- Parts split at 3.8 GB because WAV sizes are 32-bit.

## Exit

| Event | Handling |
|---|---|
| `q`, `Esc`, `Ctrl+C` (interactive) | Normal exit |
| `Ctrl+C` (plain) | Console handler sets `shutdown::requested()` |
| Console closed, logoff, shutdown | Handler blocks up to 4.5 s until `shutdown::completed()`; Windows kills the process after 5 s |
| Kill, crash, power loss | Repaired on next start |
| Panic | `Recorder::drop` finalizes the take |

`App::finish` drops the engine before the recorder, so no audio arrives while
the take is closed. Console output ignores write errors (`console::say!`), so a
closed pipe does not end a recording.

## Front ends

Interactive and plain mode only map input to `Action` and display
`App::snapshot()`; behavior lives in `App`. The view renders from a `Snapshot`,
so it is tested with ratatui's `TestBackend` without audio devices.

## Settings

`App` keeps the active settings and the saved ones. CLI options change only the
active ones; `App::update` copies to the saved copy only the fields the user
changed, so one-off flags are never persisted. Saves go through a temp file and
rename.

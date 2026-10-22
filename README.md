# work

A minimal terminal focus timer with built-in white, pink and brown noise.

Type `work`, get a 30-minute focus session with soft noise and a calm full-screen
timer. When the time is up, the noise fades out and you get a desktop notification.

```text
╭──────────────────────────────────────────────────╮
│                                                  │
│                     W O R K                      │
│                  FOCUS SESSION                   │
│                                                  │
│       ██████  ██████      ██████  ██  ██         │
│           ██      ██  ██      ██  ██  ██         │
│       ██████  ██████      ██████  ██████         │
│       ██          ██  ██  ██          ██         │
│       ██████  ██████      ██████      ██         │
│                                                  │
│  ███████████████▋░░░░░░░░░░░░░░░░░░░░░░░░    18% │
│                                                  │
│  ◉ Focusing                       30 min session │
│  ◈ White Noise                        Volume 35% │
│                                                  │
│  [Space] Pause          [+/-] Volume             │
│  [N] Noise              [M] Mute                 │
│  [R] Restart            [Q] Quit                 │
│                                                  │
╰─────────────────────────────────────────── ? help ╯
```

- Real-time noise synthesis, no audio files, fully offline.
- Own volume control: the system mixer is never touched.
- Smooth fades on start, stop, pause, mute and noise switch, so there are no clicks.
- Audio runs inside the `work` process. Nothing keeps playing after it exits, even after `kill -9`.
- Monotonic clock (`Instant`): the timer does not drift when the system is busy or the clock changes.
- Low resource use: about 1 MB binary, about 10 MB RAM. It redraws once per second and sleeps between redraws, with no busy-wait. While paused, the audio device is suspended.
- Works in small terminals, without colours (`NO_COLOR`), and without Unicode (`--ascii`, the Linux console).

## Usage

```sh
work                              # 30 minutes, white noise, volume 35%
work 45                           # 45 minutes
work -t 25                        # 25 minutes
work --noise pink                 # white | pink | brown
work --volume 40                  # 0-100, independent of system volume
work 50 --noise brown --volume 25 # flags combine
work --silent                     # no sound (unmute later with M)
work --no-tui                     # one-line text countdown, good for tmux panes
work --no-notify                  # no desktop notification at the end
work --help
work --version
```

### Keys

| Key | Action |
|---|---|
| `Space` | Pause / resume (the timer and the sound stop together) |
| `+` / `-` (or arrows) | Volume up / down by 5% |
| `N` | Next noise: white, pink, brown |
| `M` | Mute / unmute |
| `R` | Restart the session (asks for confirmation while running) |
| `Q`, `Ctrl+C` | Quit |
| `?` | Help |

### Exit codes

`0` normal exit, `2` invalid arguments or config, `128+N` stopped by signal N
(`130` SIGINT, `143` SIGTERM, `129` SIGHUP).

## Configuration

Optional. `work` reads `$XDG_CONFIG_HOME/work/config.toml`
(usually `~/.config/work/config.toml`) and never creates it. Command-line flags take
priority over the file.

```toml
duration = 30          # minutes, 1-1440
noise = "white"        # white | pink | brown
volume = 35            # 0-100
notifications = true
theme = "minimal"      # minimal | mono (no colours)
```

Unknown keys and invalid values are reported with a clear error instead of being ignored.

## Requirements

- Linux with ALSA. PipeWire works through `pipewire-alsa`, and PulseAudio through the ALSA pulse plugin.
- To build: Rust 1.85+ (edition 2024), `pkg-config` and the ALSA headers (`alsa-lib` on Arch, `libasound2-dev` on Debian/Ubuntu).
- Optional: `notify-send` (`libnotify`) for desktop notifications.

On Arch / EndeavourOS:

```sh
sudo pacman -S --needed rust alsa-lib pkgconf pipewire-alsa libnotify
```

## Build and install

```sh
git clone https://github.com/Aibek-S/work.git
cd work
cargo build --release
install -Dm755 target/release/work ~/.local/bin/work
```

Make sure `~/.local/bin` is in your `PATH`. No root access is needed.

Alternatively: `cargo install --path .` installs to `~/.cargo/bin/work`.

## Uninstall

```sh
rm ~/.local/bin/work
rm -r ~/.config/work      # only if you created a config
```

## Development

```sh
cargo test                                    # unit tests: timer, CLI, config, noise, UI rendering
cargo clippy --all-targets
cargo build --release
python3 tests/pty_smoke.py target/release/work  # end-to-end: keys, resize, signals, terminal restore
```

### Architecture

```text
src/
  main.rs    entry point, terminal setup/restore, event loop, signals, --no-tui mode
  cli.rs     argument parsing (lexopt) and help text
  config.rs  XDG config loading, validation, settings merge
  app.rs     session state and actions; no terminal code, unit-tested
  timer.rs   monotonic-clock timer with pause/resume
  audio.rs   cpal output stream, gain smoothing, device suspend on pause
  noise.rs   xorshift PRNG + white / pink (Kellett filter) / brown (leaky integrator)
  ui.rs      ratatui rendering, big digits, palette, layout fallbacks
  notify.rs  notify-send wrapper
```

How it works: the UI thread changes only atomic values: the target gain and the noise type.
The audio callback thread reads them and moves the gain toward the target smoothly
(time constant 40 ms). Pause and mute become a fade to zero. After the fade, the device
is suspended. Ctrl+C, SIGTERM and SIGHUP only set a flag, and the event loop checks it
at least every 250 ms. Then all exit paths go the same way: fade out the audio, close
the device, restore the terminal.

## License

MIT

# Configuration reference

brainfm works without any configuration. It saves your choices as you make
them, so you rarely need to edit anything by hand.

## Files

Paths follow the XDG base directory spec; the table shows the Linux defaults.

| File | Path | Contents |
|---|---|---|
| Preferences | `~/.config/brainfm/config.toml` | Volume, filters per mental state, timer lengths. |
| Session | `~/.local/state/brainfm/session.json` | The login token. Mode `0600`, in a `0700` directory. |
| Log | `~/.local/state/brainfm/brainfm.log` | Warnings and errors from the TUI. |
| Session socket | `$XDG_RUNTIME_DIR/brainfm/tui.sock` | Lets several windows share one session. Removed on exit. |

If `XDG_RUNTIME_DIR` is not set, the socket goes into the state directory
instead.

## config.toml

Every key is optional; missing keys take the defaults shown here.

```toml
# Output volume, 0–100.
volume = 70

# The mental state the player opens on (its id from `brainfm list states`).
last_state = "focus"

[timer]
# Up to three extra countdown lengths offered by the `t` picker, in minutes.
custom_minutes = [45, 90]
# Work and break lengths for Intervals, in minutes.
focus_minutes = 25
break_minutes = 5

# Filters for one mental state, keyed by its id.
[states.focus]
activity_id = "deep-work"           # from `brainfm list activities`
genres = ["Electronic", "Piano"]    # from `brainfm list genres`; empty means all
effects = ["High"]                  # "Low", "Medium", "High"; empty means any
```

Ids such as `focus` and `deep-work` above are examples; use the ones that
`brainfm list` prints for your account.

### Timer

`custom_minutes` adds entries to the timer picker after the built-in 30 min,
1 hr and 2 hrs; brainfm shows at most three. `focus_minutes` and
`break_minutes` set the Intervals mode.

### When the file is invalid

If `config.toml` cannot be parsed, the TUI starts with the defaults, shows the
problem in a banner, and does not overwrite your file until you fix it.
`brainfm play` stops with an error that names the file.

### How brainfm writes it

Changes are saved in the background half a second after the last one, so
dragging a volume slider does not rewrite the file over and over. Each write
goes to a temporary file that is then renamed into place, so the file is never
left half written, even with several brainfm processes running.

## Environment variables

| Variable | Effect |
|---|---|
| `BRAINFM_AUDIO_HOST` | Forces the sound system: `pipewire`, `pulse` or `alsa`. See [Sound output](user-guide.md#sound-output). |
| `BRAINFM_ASCII` | Set to `1` to draw with plain ASCII symbols. |
| `COLORTERM` | `truecolor` or `24bit` enables the full palette; anything else uses 256 colours. |
| `XDG_CONFIG_HOME`, `XDG_STATE_HOME` | Move the configuration and state directories. |
| `XDG_RUNTIME_DIR` | Where the shared-session socket lives. |

## Build features

| Feature | Default | Effect |
|---|---|---|
| `pipewire` | off | Talk to PipeWire natively instead of through `pipewire-pulse`. Needs libpipewire 0.3.53 or newer and clang to build, and libpipewire at runtime. |

```sh
cargo install --git https://github.com/dynamotn/brainfm-tui --features pipewire
```

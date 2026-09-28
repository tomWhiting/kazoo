# kazoo-mix

The kazoo studio desk: an analogue-style mixing console in the terminal that
every kazoo instrument plugs into, all following one tempo and one play/stop.

## Playing with it

```sh
# terminal 1: the desk
cargo run -p kazoo-mix --release

# terminals 2, 3, …: instruments, in any order, before or after the desk
cargo run -p kazoo-808 --release
cargo run -p kazoo-mini --release
cargo run -p kazoo-cs80 --release
cargo run -p kazoo-dx --release
cargo run -p kazoo-arp --release
cargo run -p kazoo-303 --release
cargo run -p kazoo-juno --release
cargo run -p kazoo-prophet --release
cargo run -p kazoo-tui --release   # the voice synth
```

Each instrument plugs into the next free strip, and the desk says so in its
header. While an instrument is plugged in, its own output goes quiet: you hear
it through its strip. Close the desk and the instruments go back to playing
on their own. Start it again and they plug straight back in.

`cargo run -p kazoo-mix --release -- --demo` also plugs in a built-in 808
pattern, so there is something to mix straight away. It joins the desk the
way any instrument does, on the next free strip.

## The desk

Each strip, top to bottom, has:

- source name and health light
- trim
- high, mid and low EQ
- aux send to the reverb
- pan
- mute and solo
- a fader with a peak meter each side

The master section has needle VU meters, the reverb return, and the master
fader.

The header shows play/stop, the tempo, beat lights and the metronome click.

| Keys | What they do |
| --- | --- |
| `←` `→` / `h` `l` | move between strips (past the last strip is the master) |
| `↑` `↓` / `k` `j` | move between controls on a strip |
| `1`–`8`, `9` | jump to a strip, or `9` for the master |
| `+` `-`, `}` `{` | nudge the focused control a little, or a lot |
| `0` | reset the focused control; `R` resets the whole strip |
| `m` `s` | mute, solo |
| `x` | clear the clip lights |
| `space` | play / stop the studio (every instrument follows) |
| `t` | tap tempo |
| `[` `]` | tempo down / up 1 BPM |
| `c` | metronome click on / off |
| `?` | help |
| `q` | quit (also `Esc`, `Ctrl-C`, `Ctrl-Q`, `Ctrl-D`) |

With the mouse, you can:

- drag faders
- scroll over knobs
- click mute, solo, the clip lights, the bank arrows and the play/stop button

## Things to know

- Instruments must run at the desk's sample rate. They do if they all use the
  same audio device. The desk says so if one doesn't.
- The desk takes the hub socket that instruments look for. If another hub is
  already serving, it refuses to start and says which process has the socket.
- Every instrument is locked to the desk's song position to the sample.
  The desk schedules play, stop and tempo changes a few tens of
  milliseconds ahead. It tells each instrument which frame of its own audio
  the change lands on, and the song position there. So every downbeat, the
  desk's click included, comes out of the desk on the same frame.
  - An instrument that joins mid-song, or hears of a change late, catches
    up to the grid rather than starting late.
  - Pressing play or changing tempo on an instrument asks the desk, so the
    whole studio follows.

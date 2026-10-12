# words

A classy cyberpunk terminal typewriter. It writes words and sentences in a big
pixel font in the middle of the screen, blinks a block cursor at the end, holds
them for a beat, then moves on — with glitch bursts tearing through the text.

It is also a countdown/stopwatch and a wall clock. `T` walks between the three,
or open the picker and choose one directly.

Zero dependencies. It is raw ANSI escape codes and a 5×7 bitmap font, so it
builds in seconds and runs anywhere with a terminal.

## Build and run

```sh
cargo build --release
./target/release/words
```

`stty` does the terminal work (raw mode and the size query), so this is
POSIX-only — Linux, macOS, and the BSDs. Nothing else is needed.

## Options

```
-s, --seed <n>      deterministic glitch seed
-w, --words <path>  load phrases from a file, or every .txt in a directory
-g, --lang <code>   start from a bundled language
-i, --stdin         also read phrases from a pipe
-t, --timer [spec]  start on the counter: seconds, or `up` / `down`
-k, --clock         start on the wall clock
-m, --menu          open the picker at startup
-l, --list          print the built-in phrases and exit
-h, --help          usage
```

`--words` replaces the built-in pool, and a `words.txt` in the working
directory is picked up when you don't pass one. Blank lines are skipped and
anything outside the font's glyph set is folded down to ASCII, so accented
prose ("Café naïve", "Zürich", "Straße") survives as words rather than losing
letters. A `--lang` set is a starting point that custom sources add to rather
than replace.

Anything dropped in `$XDG_CONFIG_HOME/words` (or `~/.config/words`) — every
`.txt` file there, by its name — shows up on the picker, so prose can be kept
handy without naming it on the command line.

## Modes

| Mode | |
|---|---|
| `WORDS` | the typewriter |
| `COUNTER` | a countdown or a stopwatch, driven by the keys |
| `CLOCK` | the real time of day and the date, driven by the machine |

`--timer` and `--clock` start straight into the one you asked for; without them
the typewriter is what you get. On the `CLOCK` the date sits under the digits
(`MON 2026-10-12`) and a moving head sweeps the control bar, because a wall
clock has no start or finish to fill toward.

## Keys

| Key | |
|---|---|
| `W` / `↑` | faster |
| `S` / `↓` | slower |
| `SPACE` | next phrase |
| `←` / `→` | previous / next |
| `F` | font size (big ⇄ small) |
| `R` | auto-ramp the speed |
| `G` | force a glitch |
| `B` | pin the control bar |
| `T` | next mode |
| `M` | open the picker |
| `H` / `?` | controls |
| `Q` | quit |

On the counter, `SPACE`/`P` run, pause and resume, `C` clears back to the start,
`U` flips between counting down and counting up, and `+`/`-` lengthen and
shorten the run. The wall clock has no controls of its own: it is always
running.

## How it behaves

**The speed knob moves three things at once.** One control sets typing speed,
how long a finished phrase is held, and the gap before the next one. Raising it
makes all three shorter together, so "faster" never quietly means "same speed,
less pause".

**The control bar times out.** It appears on any keypress and fades out three
seconds after the last one, so the words own the screen while you watch. `B`
pins it open.

**The font is drawn with background colours.** A lit pixel is a space painted
cyan-on-black, which means a whole scanline costs one cursor move and one colour
change, and a 5×7 glyph costs fourteen terminal rows. That is what makes the big
font affordable at 60 fps with no libraries.

**Layout adapts to the window.** Phrases word-wrap, each line is centred
individually so a wrapped sentence reads as one block, and the font drops a size
rather than truncating. On a window too short for the chrome, the chrome gives
way — a line of text always fits.

**Every painter wipes its own footprint.** A terminal keeps whatever the last
frame left in a cell, so a glitch burst, the control bar fading out, and the
help panel closing all repaint what they covered rather than assuming something
else will. Glitch debris gets a padded erase box and a forced repaint on the
following frame, so torn pixels never linger.

## Layout

```
src/main.rs     CLI, frame loop, key reader thread
src/engine.rs   state machine, layout, speed model, tests
src/view.rs     everything that draws
src/style.rs    palette, colour mixing, run-coalescing canvas
src/font.rs     5x7 glyphs
src/phrases.rs  phrase pools and transliteration
src/time.rs     UTC offset, civil-date maths, the clock's strings
src/config.rs   the ~/.config/words directory the picker reads
src/term.rs     raw mode, alt screen, restore-on-drop and on panic
```

## Tests

```sh
cargo test
```

Unit tests cover centring, wrapping, the speed model, the three-second rule,
the phrase transliteration, and the repaint bookkeeping that keeps stale pixels
off the screen — alongside the counter, the wall clock's date arithmetic, and
the picker's navigation and file selection.

## Notes

The terminal is put into raw mode and the alternate screen, and both are
restored on quit, on `Drop`, and from a panic hook — a crash mid-run leaves a
working shell rather than a terminal with echo off. The window size is polled
four times a second rather than per frame, since `stty size` is a subprocess and
asking sixty times a second is enough to starve the render loop.

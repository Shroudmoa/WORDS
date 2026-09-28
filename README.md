# words

A classy cyberpunk terminal typewriter. It writes words and sentences in a big
pixel font in the middle of the screen, blinks a block cursor at the end, holds
them for a beat, then moves on — with glitch bursts tearing through the text.

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
-w, --words <file>  load phrases from a file (one per line)
-l, --list          print the built-in phrases and exit
-h, --help          usage
```

`--words` replaces the built-in pool, and a `words.txt` in the working
directory is picked up when you don't pass one. Blank lines are skipped and
anything outside the font's glyph set is folded down to ASCII, so accented
prose ("Café naïve", "Zürich", "Straße") survives as words rather than losing
letters.

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
| `H` / `?` | controls |
| `Q` | quit |

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
src/term.rs     raw mode, alt screen, restore-on-drop and on panic
```

## Tests

```sh
cargo test
```

Twenty unit tests cover centring, wrapping, the speed model, the three-second
rule, the phrase transliteration, and the repaint bookkeeping that keeps stale
pixels off the screen.

## Notes

The terminal is put into raw mode and the alternate screen, and both are
restored on quit, on `Drop`, and from a panic hook — a crash mid-run leaves a
working shell rather than a terminal with echo off. The window size is polled
four times a second rather than per frame, since `stty size` is a subprocess and
asking sixty times a second is enough to starve the render loop.

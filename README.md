# words

A classy cyberpunk terminal typewriter. It writes words and sentences in a big
pixel font in the middle of the screen, blinks a block cursor at the end, holds
them for a beat, then moves on — with glitch bursts tearing through the text.

It is also a countdown/stopwatch, a wall clock, a deadline tracker, a little
todo list, and a day planner drawn as a small Gantt chart. `T` walks between
those modes, or open the picker and choose one directly.

Reader Mode is a seventh thing: it opens a whole `.txt` or `.md` document in a
plain, left-aligned, top-to-bottom terminal layout with Vim-style movement. It
is an addition to the typewriter, not a replacement for it.

Voice is optional and sits across both: the typewriter can say each new phrase,
and Reader can read a document aloud while tinting the sentence being spoken.
There is no audio crate in the build — WORDS uses whatever speech command the
machine already has, and reports a small message instead if none is available.

Zero dependencies. It is raw ANSI escape codes and a 5×7 bitmap font, so it
builds in seconds and runs anywhere with a terminal.

## Install

```sh
cargo install --path .
```

That builds the release binary and drops a `words` on your `PATH` (in
`~/.cargo/bin` by default), so it runs from any directory. To build without
installing it:

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
-r, --read <path>   read a .txt or .md document
    --todo          start on the todo list
    --plan          start on the day planner
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
handy without naming it on the command line. Saved deadlines share the
directory in a file called `deadlines`; the todo list and the planner use
`todos` and `plan`; voice settings live in `voice`. None of them are `.txt`, so
they stay off the phrase list.

## Modes

| Mode | |
|---|---|
| `WORDS` | the typewriter |
| `COUNTER` | a countdown or a stopwatch, driven by the keys |
| `CLOCK` | the real time of day and the date, driven by the machine |
| `DEADLINE` | a countdown to a moment you named, or to the end of the year |
| `TODO` | a little list of things to do |
| `PLANNER` | a day of blocks with a start and an end |
| `READER` | a whole document in a readable top-to-bottom layout |

`--timer` and `--clock` start straight into the one you asked for; without them
the typewriter is what you get. `words --read notes.md` (or just `words
notes.txt`) opens Reader Mode; so does choosing a row under `DOCUMENTS` in the
picker. `q` or `esc` leaves the reader and hands the keyboard back.

## Reader Mode

Reader Mode shows the document as text, not as words appearing in the middle of
the screen. It starts in `NORMAL` mode. Lines are left-aligned, wrapped to the
terminal width (capped at 100 columns for comfortable reading), and blank
lines are kept as paragraph breaks. Resizing the window rewraps the text and
keeps the current line on screen. Only the visible rows are painted, so a long
document is no more expensive to draw than a short one; files over 16 MiB or
200,000 lines are refused with a message rather than loaded.

`.txt` files are shown as written. `.md` files get a small, readable Markdown
treatment: headings, bold, italic, inline code, links, lists, blockquotes,
fenced code and horizontal rules become styles on the same text, so search,
selection and copying see the words rather than the markers. The original file
content is kept untouched alongside the rendered lines.

The header names the document and the current mode (`NORMAL`, `VISUAL`,
`SEARCH`, `COMMAND`). The footer gives the current line and column and the
percentage read. A thin mark in the left gutter shows the current line; the
cursor itself is only a faint tint on one character.

| Key | Action |
|---|---|
| `h` `j` `k` `l` / arrows | move left, down, up, right |
| `w` / `b` / `e` | next word / previous word / end of word |
| `0` / `$` | beginning / end of the display row |
| `gg` / `G` | first / last line |
| `:{number}` | jump to that line |
| `10j`, `5w`, … | numeric prefixes on motions |
| `ctrl-d` / `ctrl-u` | half page down / up |
| `ctrl-f` / `ctrl-b` | full page down / up |
| `H` / `M` / `L` | top / middle / bottom of the screen |
| `zz` | center the current line |
| `/` / `?` | forward / backward search |
| `n` / `N` | next / previous match |
| `v` / `y` | start visual selection / copy it |
| `yy` | copy the current line |
| `:e PATH` | open another document (`:q` leaves, `:help` shows keys) |
| `ctrl-h` | toggle the in-app key panel |
| `space` | read aloud from the cursor, or pause playback |
| `s` | stop playback |
| `<` / `>` | previous / next sentence |
| `[` / `]` | slower / faster speech |
| `V` | toggle voice from the config |
| `:voice NAME`, `:preview` | switch neural voice, hear a sample |
| `:rate N` | set words per minute |
| `q` / `esc` | leave Reader Mode |

Search is case-insensitive, highlights every match in the visible text and
wraps at the end of the document. `esc` cancels a search or command prompt.
Copying uses `xclip`, `xsel` or `pbcopy` if one is installed; without one the
selection still works and the reader says there is no clipboard available.
Nothing in Reader Mode writes to the document.

The last position in each document is saved in
`$XDG_CONFIG_HOME/words/positions` (or `~/.config/words/positions`) and restored
when the same document is opened again. On the `CLOCK` the date sits under the digits
(`MON 2026-10-12`) and a moving head sweeps the control bar, because a wall
clock has no start or finish to fill toward.

## Voice

The default voice is **Microsoft Edge neural speech**, not a synth that merely
works. WORDS embeds a small Python helper (the same Rust-calls-Python pattern
as `src/v3.py`) that uses `edge-tts` to stream audio and word boundaries. Rust
stays dependency-free: it owns the cache, playback clock and read-along
highlighting; Python only produces sound.

One-time setup:

```sh
python3 -m pip install edge-tts
words --voice-doctor
```

`--voice-doctor` reports Python, the helper, available audio players and the
config path. A player such as `ffplay`, `mpg123`, `paplay`, `aplay` or `afplay`
is still needed to actually hear the MP3. Without one, the reader keeps working
and the prompt explains what is missing.

`$XDG_CONFIG_HOME/words/voice` (or `~/.config/words/voice`) is a plain
`key=value` file. It is created on the first settings change; the defaults are
already listening:

```ini
enabled=true                # master switch; v toggles it while running
reader=true                 # read documents aloud in Reader Mode
words=true                  # say each typewriter phrase as it appears
backend=edge                # edge / piper / say / espeak / spd-say / none
voice=en-US-AvaNeural       # Edge voice name, or a Piper .onnx path
rate=175                    # words per minute
volume=100                  # percentage
pitch=+0Hz                  # Edge pitch, e.g. -3Hz
lang=en-US                  # language hint
```

Older files with `backend=auto` or `backend=gtts` are upgraded to `edge` on
read. `piper`, `say`, `espeak` and `spd-say` are explicit fallbacks, chosen by
setting `backend`; they are not used silently when the neural backend fails.
`backend=none` keeps the interface and highlights working without starting any
audio process. A missing engine reports a message on the prompt line rather
than failing the frame loop.

Audio and its word timings are cached under `voice-cache/`, keyed by text,
voice, rate and pitch, so a repeated phrase or document does not hit the
network again. Edge returns precise word boundaries: the current word is tinted
a brighter green than its sentence, and highlighting comes from the playback
clock rather than a guessed word count.

In Reader Mode, `space` reads from the cursor, pauses at the exact word and
resumes there; `s` stops; `<`/`>` seek a sentence with playback running;
`[`/`]` change speed; `V` or bare `:voice` flips the master switch;
`:voice en-GB-SoniaNeural` switches voice, and `:preview` speaks a sample.
In the typewriter there is nothing to press: a phrase is spoken once when it
changes, whether that came from a key, a live file edit or a mode switch. `v`
outside the reader toggles voice on and off and writes the setting back.

## Deadlines

The picker (`M`) has a `DEADLINES` section. `END OF YEAR` is always there — it
counts down to the turn of the year and shows how much of it has gone
(`END OF YEAR · 48.2% GONE · 137d 11:22`) — and below it are the deadlines
you've saved, soonest first, each with its own countdown. Choosing a row makes
it the `DEADLINE` mode and starts counting.

`+ ADD A DEADLINE` opens a small form: a name, a date, and a time. `TAB` (or
`←`/`→`) moves between the fields, the date and time take only digits, `-` and
`:`, `BACKSPACE` deletes, `ENTER` saves and starts the countdown, and `ESC`
cancels. A date that doesn't parse keeps the form up and says so rather than
saving nonsense; an empty time means midnight, and an empty name becomes
`DEADLINE`.

Saved deadlines live in `~/.config/words/deadlines` (see below) as one
`NAME | YYYY-MM-DD HH:MM` per line, in local time — plain text, hand-editable,
and any line that doesn't parse is skipped rather than refusing the rest.

The `DEADLINE` mode shows `DD:HH:MM:SS` while there is more than a day to go
and `HH:MM:SS` inside the last day. Reaching zero rings and strobes the same
way a countdown does, and the strip under the digits names what it is counting
to. On the `CLOCK` the caption carries the year too (`THU 2026-10-20 · 80.0%
GONE`), so the two answers about the year are always a glance away.

To remove a deadline, highlight it in the picker and press `X`. It is taken off
the list and the file is written back. (`X` on any other row does nothing — it
will not delete a phrase file.)

## Todo

`--todo`, the picker, or `T` gets you to a small todo list. It is one task per
line with a checkbox:

- `UP`/`DOWN` (or `J`/`K`) move through the list.
- `SPACE` ticks or unticks the highlighted task.
- `A` opens an inline line at the bottom; type the task, `ENTER` saves it,
  `ESC` abandons it.
- `X` deletes the highlighted task.
- `C` clears every finished task and leaves the open ones alone.

It is saved in `~/.config/words/todos` as plain text, so it can be edited by
hand:

```
# [ ] open   [x] done
[ ] BUY COFFEE
[x] SHIP IT
```

A plain `- ` bullet is read as an open task too, and any line that is not a
task is skipped rather than refusing the rest.

## Planner

`--plan`, the picker, or `T` opens a day planner. Events have a name, a date, a
start and an end, and the day is drawn as a small Gantt chart: one row per
event, with the 24-hour day running left to right and each event a bar from its
start to its end. A magenta marker shows where "now" is when the day on screen
is today.

- `LEFT`/`RIGHT` (or `P`/`N`) move to the previous or next day.
- `UP`/`DOWN` (or `K`/`J`) move through the day's events.
- `A` opens a form for a new event: name, start (`HH:MM`), end (`HH:MM`).
  `TAB` (or `←`/`→`) moves between fields, `ENTER` saves, `ESC` cancels.
- `E` edits the highlighted event.
- `X` deletes the highlighted event.
- `D` jumps back to today.

An end at or before the start is refused, because an event is a block inside
one day. Events are saved in `~/.config/words/plan`, one per line:

```
# name | local date start end
DESIGN REVIEW | 2026-10-29 09:00 10:30
LUNCH | 2026-10-29 12:30 13:15
```

The planner sorts by day and start time, and only draws the day it is showing.

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
| `V` | voice on / off |
| `M` | open the picker |
| `H` / `?` | controls |
| `Q` | quit |

On the counter, `SPACE`/`P` run, pause and resume, `C` clears back to the start,
`U` flips between counting down and counting up, and `+`/`-` lengthen and
shorten the run. The wall clock has no controls of its own: it is always
running.

In the reader the keys are Vim-like: `hjkl`, `w/b/e`, `0/$`, `gg/G`, `:{n}`,
numeric prefixes, `ctrl-d/u/f/b`, `H/M/L`, `zz`, `/` and `?`, `n/N`, `v` and
`y`, and `q` or `esc` to leave. The full table and the behaviour of each one
are in [Reader Mode](#reader-mode) above.

In the picker, `↑`/`↓` (or `W`/`S`, `K`/`J`) move, `ENTER` chooses, a digit
jumps straight to that row, `X` deletes the highlighted deadline, and `M` or
`ESC` closes it. Section headings (`MODES`, `DEADLINES`, `PHRASES`) are labels,
not rows — navigation steps over them and the digit shortcuts count only the
rows you can act on.

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
src/deadline.rs saved deadlines and the on-disk format
src/todo.rs     saved todo list and the on-disk format
src/plan.rs     saved planner events and the on-disk format
src/time.rs     UTC offset, civil-date maths, the clock's strings
src/config.rs   the ~/.config/words directory the picker reads
src/reader/     Reader Mode: document loading, layout, navigation, rendering
src/voice.rs       voice config, cache, word timings, playback worker, backends
src/voice_helper.py embedded edge-tts helper: synthesis + word boundaries
src/term.rs     raw mode, alt screen, restore-on-drop and on panic
```

## Tests

```sh
cargo test
```

Unit tests cover centring, wrapping, the speed model, the three-second rule,
the phrase transliteration, and the repaint bookkeeping that keeps stale pixels
off the screen — alongside the counter, the wall clock's date arithmetic, the
deadline file's round-trip, the year maths, and the picker's navigation and
file selection. The todo list and the planner get the same treatment: their
file formats round-trip, bad lines are skipped, tasks tick and clear, events
add, edit, delete, sort, and refuse an end before their start. The reader adds
tests for Markdown rendering, wrapping, Unicode widths, every navigation
command, search and selection, empty documents, resizing and saved positions.
Voice tests cover the config round-trip, sentence boundaries, Unicode positions
and the silent backend, plus the reader's sentence jumps, speed keys and stop.

## Notes

The terminal is put into raw mode and the alternate screen, and both are
restored on quit, on `Drop`, and from a panic hook — a crash mid-run leaves a
working shell rather than a terminal with echo off. The window size is polled
four times a second rather than per frame, since `stty size` is a subprocess and
asking sixty times a second is enough to starve the render loop.

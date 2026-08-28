# mctop

[![Package](https://github.com/Kelvinlby/mctop/actions/workflows/package.yml/badge.svg)](https://github.com/Kelvinlby/mctop/actions/workflows/package.yml)

A terminal dashboard for [Folia](https://papermc.io/software/folia) and Paper
Minecraft servers. It watches the numbers that decide whether players are having
a good time — tick rate, tick times, heap pressure, and on Folia the per-region
breakdown that a global average hides.

```
 mctop  survival  ·  Folia 1.21.4  ·  ● connected  ·  up 3d 4h        47 / 120 players
 1 Overview │ 2 Regions │ 3 System │ 4 Worlds │ 5 Log
╭ TPS ───────────────────────╮╭ MSPT ──────────────────────╮╭ CPU ───────────────────────╮
│  ╻  ┏━┓     ┏━┓ ┏━┓        ││ ┏━┓ ┏━┓     ┏━┓            ││ ╻ ╻ ┏━┓ ┏━┓                │
│  ┃  ┗━┫     ┗━┫ ┣━┫        ││ ┏━┛ ┗━┫       ┃            ││ ┗━┫  ━┫ ┣━┫                │
│  ╹  ┗━┛  ▄  ┗━┛ ┗━┛        ││ ┗━╸ ┗━┛  ▄    ╹ ms         ││   ╹ ┗━┛ ┗━┛ %              │
│ 1m 19.7 5m 19.9 15m 20.0   ││ min 4.2  max 46.8          ││ 54.8% of 8 cores           │
╰────────────────────────────╯╰────────────────────────────╯╰────────────────────────────╯
```

## What it shows

| Metric | Where it comes from |
| --- | --- |
| Tick rate, over every window the server reports | `/tps` over RCON |
| Tick times — average, minimum, and **maximum** | `/mspt` over RCON |
| Per-region tick rate, tick time, and load | the Folia region report over RCON |
| Players online, server flavour and version | `/list` and `/version` over RCON |
| CPU use, resident memory, thread count, uptime | the local JVM process |
| Heap occupancy **after garbage collection**, and collector load | `jcmd`, or `jstat` where the JVM allows it |
| World sizes on disk, broken down by `region/`, `entities/`, `poi/` | the local filesystem |
| Free space on the filesystem holding the worlds | the local filesystem |

Two of those deserve a note.

**Heap after collection** is the number that says whether a server is genuinely
running out of memory. Resident set size and current heap usage both include
garbage that is about to be handed back, so a healthy server and a doomed one
look identical in `top`. mctop reports the heap's low-water mark and uses
`jstat`'s collection counters to say whether a collection actually happened
recently — when none has, the figure is labelled as an estimate rather than
presented as fact.

**Per-region detail** exists because Folia ticks each slice of a world on its own
thread. One region grinding to a halt under a mob farm barely moves the global
average, but every player standing in it notices. The Regions tab sorts
worst-first so that region is the top row. On Paper the tab stays empty and says
why: Paper ticks one thread per server, so the global figures already tell the
whole story.

## Layout

Five screens, because a Folia server can hold hundreds of regions and a wall of
them would bury the handful of numbers that matter.

- **Overview** — four headline readings, tick-rate and tick-time history, and a
  summary column. This screen is the same size whether the server ticks one
  region or four hundred.
- **Regions** — every region as a sortable table, worst-first by default, with a
  detail panel for the selected row (including its block coordinates, so you can
  go and look).
- **System** — processor, memory, Java heap, and garbage collector.
- **Worlds** — what each world costs on disk, and how much room is left.
- **Console** — mctop's own notes, the commands you have typed and the server's
  replies, and a command line at the bottom to send more. Polling traffic is
  deliberately kept out of it; `v` shows the raw poll responses when you need
  them for diagnosis.

## Install

### From a .deb

Every push builds a package; download it from the run's artifacts on the
Actions tab (kept for seven days), then:

```sh
sudo apt install ./mctop_0.1.0-1_amd64.deb
```

The binary goes to `/usr/bin/mctop`, so there is no `PATH` to set up. The
installer asks whether to write a sudoers rule letting mctop read the Java heap
when it runs as a different user than the server — see [Reading the heap when
mctop runs as a different user](#reading-the-heap-when-mctop-runs-as-a-different-user).
Declining is safe, and `sudo dpkg-reconfigure mctop` reopens the question later.

The package is built on Ubuntu 24.04, so it needs glibc 2.39 or newer — Ubuntu
24.04+ or Debian 13 (trixie)+. On anything older, build from source.

### From source

```sh
cargo build --release
```

The binary lands at `target/release/mctop`. To build the package yourself:

```sh
cargo install cargo-deb
cargo deb                     # writes target/debian/mctop_<version>_<arch>.deb
```

## Setting up

Enable RCON in `server.properties`:

```properties
enable-rcon=true
rcon.port=25575
rcon.password=something-long
```

Write a starter config and edit it:

```sh
mctop config init     # writes ~/.config/mctop/config.toml
mctop config path     # says where that is on your system
```

At minimum, set `[rcon].address` and give mctop the password. Prefer the
environment variable or a file over an inline password:

```sh
export MCTOP_RCON_PASSWORD=something-long
```

Set `[server].directory` to the server's folder and mctop finds the world folders
under it on its own.

## Usage

```sh
mctop                                  # the dashboard
mctop --address 10.0.0.5:25575         # override the configured address
mctop --interval 0.5                   # override the refresh rate
mctop --directory /srv/minecraft       # override the server folder

mctop status                           # one reading, as plain text, for scripts
mctop probe                            # every command's unparsed response
mctop config show                      # the configuration in force
```

Run it on the server's own host where you can. CPU, memory, and world sizes are
read from the local machine — the game has no idea how much CPU it is using, so
no amount of RCON will produce those. Over RCON alone the tick and region
metrics still work; the System tab says why the rest is missing.

### Reading the heap

Occupancy comes from `jcmd GC.heap_info`, which asks the JVM over its attach
socket. mctop tries `jstat` first, because it is cheaper and it also carries the
collector's counters — how many collections, and how long they took — but
`-XX:+PerfDisableSharedMem` switches off the file `jstat` reads. That flag is
part of [Aikar's flags](https://docs.papermc.io/paper/aikars-flags), so most
Minecraft servers run with it, and on those servers `jstat` can never work no
matter who runs it. mctop falls back to `jcmd` and the System tab says why the
garbage collector panel is empty. Everything else is unaffected.

### When mctop runs as a different user

CPU, memory, and uptime come from `/proc` and work across accounts. The heap does
not: `jstat` reads a file the JVM creates mode `0600`, and `jcmd` attaches over a
socket that checks the caller's user and group. Belonging to the server's group
is not enough, and the file's mode cannot be relaxed — the JVM recreates it on
every start.

Either run mctop as that user:

```sh
sudo -u minecraft mctop --config /etc/mctop/config.toml
```

or leave mctop running as yourself and send only the JDK tools through `sudo`.
The .deb offers to write this rule for you at install time; to do it by hand:

```toml
[jvm]
jcmd  = ["sudo", "-n", "-u", "minecraft", "jcmd"]
jstat = ["sudo", "-n", "-u", "minecraft", "jstat"]
```

with a sudoers rule to match. `-n` matters: mctop gives its children no stdin, so
a `sudo` that wants a password fails immediately rather than hanging.

```
operator ALL=(minecraft) NOPASSWD: /usr/bin/jstat, /usr/bin/jcmd
```

Put that in `/etc/sudoers.d/mctop` with `sudo visudo -f /etc/sudoers.d/mctop`,
which checks the syntax before saving — a malformed file there breaks `sudo`
for everyone.

### Keys

| Key | |
| --- | --- |
| `q` `Esc` `Ctrl-C` | quit |
| `Tab` `⇧Tab` `←` `→` | change tab |
| `1`–`5` | jump to a tab |
| `↑` `↓` `j` `k` | move the selection |
| `PgUp` `PgDn` `Home` `End` | move it further |
| `r` | collect everything now |
| `p` `Space` | pause and resume collection |
| `s` | cycle the region sort (Regions tab) |
| `v` | raw poll responses (Console tab, unfocused) |
| `?` `h` `F1` | help |

On the Console tab the command line takes the keyboard, so `q` types a letter
rather than quitting:

| Key | |
| --- | --- |
| `Enter` | send the command |
| `↑` `↓` | walk the command history |
| `Esc` | clear the line; again to give the keys back |
| `Ctrl-C` | quit, even mid-command |
| `Ctrl-W` `Ctrl-U` | delete a word, delete the line |
| `Ctrl-A` `Ctrl-E` `Home` `End` | move to either end |
| `PgUp` `PgDn` | scroll the output while typing |
| `Tab` `Shift-Tab` | change tab without losing the half-typed line |

Commands go out over the same RCON connection as the metrics, at console
privilege — the same as typing into the server's terminal. They run even while
collection is paused, since pausing stops mctop asking questions, not you.

## Configuration

Everything lives in one TOML file; run `mctop config init` for a commented copy.
Command-line flags override the file, and `MCTOP_RCON_PASSWORD` overrides both.

The parts worth knowing about:

```toml
[refresh]
tick_ms = 1000     # TPS and MSPT
region_ms = 2000   # per-region detail
roster_ms = 5000   # player list and version
process_ms = 2000  # CPU, memory, heap
disk_secs = 120    # world sizes; this one walks the disk
draw_ms = 250      # screen redraw

[commands]
tps = "tps"        # point these elsewhere if your fork words things
mspt = "mspt"      # differently, or to use Spark: tps = "spark tps"
regions = "tps"    # on Folia the region breakdown is part of the tps report
```

Each collector runs on its own clock, so a slow disk scan never holds back the
tick-rate graph.

### When a metric shows as unavailable

Server forks word their console output differently and change it between
releases, so mctop's parsers are deliberately forgiving — and when one still
cannot make sense of a response, it says so rather than inventing a number. Two
places to look:

```sh
mctop probe        # prints every configured command's raw response
```

or the Log tab's raw view (`5` then `v`), which shows the same thing live. Then
point `[commands]` at something your server understands.

## Development

```sh
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test
```

Three examples help when working on it without a server to hand:

```sh
cargo run --example mock_server        # a stand-in Folia server that speaks RCON
cargo run --example render 120 38      # every tab drawn into a text buffer
cargo run --example probe_local        # the local JVM sampled as the System tab does
```

`mock_server` takes `--paper` for a server with no region report, `--broken` for
one whose output nothing recognises, and `--hung` for one that authenticates and
then stops answering. The integration tests drive all four.

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).

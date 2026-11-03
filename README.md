# Signet

A reading framework written in Rust, built for the **Kobo Nia** (N306) and designed
for one thing: reading. No store, no sync, no distractions — instant startup, careful
typography and intelligently refreshed e-ink.

> **Warning.** Signet replaces the stock reader on a real device and hooks into its
> boot process. It is an unofficial project, unaffiliated with Kobo or Rakuten, tested
> on a single device. Use it at your own risk.

## Layout

| Path | Purpose |
| --- | --- |
| `crates/signet` | The reader application |
| `crates/signet-text` | Text shaping, line breaking and hyphenation |
| `crates/signet-epub` | EPUB parsing: container, OPF, table of contents, XHTML and CSS |
| `crates/signet-dict` | Kobo dictionary reader (`dicthtml-*.zip`) |
| `crates/signet-fb` | E-ink framebuffer: 8-bit canvas, blit, waveform refresh |
| `crates/signet-input` | evdev input: multitouch, power button, sleep cover |
| `crates/signetd` | Daemon started at boot by our udev rule |
| `crates/signet-probe` | Hardware probe: runs on the Kobo and dumps /proc, /sys, /etc |
| `crates/signet-fbtest` | Test bench: test pattern and waveform timing |
| `tools/` | Packaging, screenshot conversion, SSH access, experiment scripts |
| `dist/` | udev rule, detach script, init script patch and development `sshd` configuration |
| `assets/` | Embedded fonts and hyphenation patterns, with their licenses |

## Measuring

```sh
cargo run -p signet --release -- --bench book.epub [--greedy] [--no-hint] [--size 0|1|2] [--csv]
```

`--render book.epub <chapter> <dir>` writes a chapter's pages as PGM files, to compare
with WebKit's rendering of the same file (`tools/dev/webkit-snap.swift`). `--bench`
lays out and renders the whole book with the device's configuration and reports
adjustment ratios, actual space widths (in ems), hyphenations, Knuth-Plass passes,
and parse / layout / render times.

Examples: `signet-text` has `lines` (a paragraph line by line), `hyph` (hyphenation
points), `breaks` and `ink` (visual benches); `signet-epub` has `dump` (all chapters).

## Building for the Kobo

```sh
rustup target add armv7-unknown-linux-musleabihf
cargo install cargo-zigbuild   # requires zig (brew install zig)
# `kobo` is an alias defined in .cargo/config.toml
cargo kobo -p signetd -p signet-probe -p signet-fbtest -p signet
./tools/mk-koboroot.sh         # produces dist/KoboRoot.tgz
```

Copy `dist/KoboRoot.tgz` into `.kobo/` on the device, eject, and wait for the reboot.
On the partition, `.signet/mode` is either `signet` (Signet replaces Nickel at boot)
or `probe` (probe only); `.signet/disable` turns everything off.

## Reading gestures

| Gesture | Effect |
| --- | --- |
| Tap the left / right edge | previous / next page |
| Horizontal swipe | next page (leftward) / previous page |
| Tap the center | settings panel (size, brightness, margins, leading, fast pages, table of contents, annotations, mode) |
| Hold a finger for 350 ms | the word lights up; dragging extends the selection within the paragraph; releasing opens "Surligner / Définition / Annuler" |
| Hold a finger on a highlight | selects it again: "Supprimer / Définition / Fermer" |

The interface is currently in French.

## Settings (`.signet/state.txt`)

| Key | Default | Purpose |
| --- | --- | --- |
| `size`, `margins`, `leading` | 1 | index into the three sizes / margins / line spacings |
| `light` | 30 | daytime brightness (0–100) |
| `auto_light` | 1 | time-of-day curve: full 7 am–8 pm, dimming until 11 pm, a quarter at night |
| `fast_pages` | 0 | black-and-white text + DU refresh (375 ms instead of 620) |
| `ink` | 1 | ink: 0 thin (raw), 1 crisp (gamma 1.4 + ¼ px), 2 bold (gamma 1.8 + ½ px) |
| `idle_minutes` | 15 | automatic sleep after inactivity (0 = never) |
| `poweroff_hours` | 72 | automatic power-off after N hours asleep (0 = never) |
| `tz` | 120 | time zone offset in minutes (the clock runs on UTC) |
| `touch` | calibration | touch transform measured on first launch |

Highlights live in `.signet/annotations/<book>.txt`, the shaping cache in
`.signet/shape-cache.bin`, covers and metadata in `.signet/covers/`.

## Two modes on the device

| | Reading | Dev (`.signet/dev` present) |
| --- | --- | --- |
| Wi-Fi | unloaded (required for sleep) | brought up by `.signet/dev.sh` (`.signet/wifi.conf`: `ssid=`, `psk=`), `sshd` |
| Short press | sleep | nothing |
| Long press (≥ 1.5 s) | back to Nickel; ≥ 5 s: reboot | reboot |
| USB cable | disk | disk |

Never hold the button for ten seconds (hard power cut), and never touch the button on
the firmware's error screen: it triggers a factory reset. If an error screen appears,
hold the button for twenty seconds, then power back on. The partition is mounted
read-only at rest; every write goes through a `remount,rw` / `sync` / `remount,ro`
window.

Dev mode ships its own SSH server (static Dropbear, `dist/dropbearmulti`, built by
`tools/dev/build-dropbear.sh`), because the 4.28 recovery firmware has none.

Dev mode tools: `tools/kobo-ssh.sh '<command>'` (key `~/.ssh/signet_kobo`),
`tools/kobo-push.sh` (deploys `dist/KoboRoot.tgz` over SSH without rebooting),
`tools/dev/sleeptest.sh` (self-contained sleep experiment, to run on the device).
SSH logins are key-only: put your public key in `dist/authorized_keys` before running
`tools/mk-koboroot.sh`. Set the device's address with `KOBO_HOST` or in `dist/kobo-host`. Secrets (`dist/authorized_keys`, `wifi.conf`) are never committed.

`tools/kobo-rcs.sh install` patches the device's own init script with `dist/rcS.patch`,
made for firmware 4.38; it stops without changing anything if the patch doesn't apply.

## License

The code is under the [MIT License](LICENSE). Embedded fonts and hyphenation patterns
keep their own licenses, next to each file in `assets/`.

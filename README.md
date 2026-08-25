# Rainbowave CLI

Rainbowave turns your terminal into a full-screen field of spectral light. Five
independently flowing ribbons braid through a deep, shifting haze with luminous
cores, bloom, internal filaments, drifting motes, and a subtle star field.

The scene runs at 60 frames per second and renders two independent truecolor
pixels in every terminal cell using Unicode half blocks. Frames are built in a
reused buffer, sent in one write, and wrapped in synchronized-update markers to
keep supporting GPU-accelerated terminals smooth and tear-free.

## Install

Download the archive for your platform from the
[latest GitHub release](https://github.com/yummydirtx/rainbowave-cli/releases/latest),
extract it, and place `rainbowave` somewhere on your `PATH`.

Prebuilt releases are available for Intel and Apple Silicon macOS and for x86-64
and ARM64 Linux.

To build from source, install Rust 1.85 or newer and run:

```sh
cargo install --path .
```

## Use

Start the animation:

```sh
rainbowave
```

Press `q`, Escape, or Ctrl-C to quit. Rainbowave uses the terminal's alternate
screen and restores the previous screen, cursor, colors, and input mode when it
exits.

The first release intentionally has one polished visual style and no tuning
flags. Run `rainbowave --help` or `rainbowave --version` for command metadata.

## Terminal support

Rainbowave targets modern macOS and Linux terminals with UTF-8 and 24-bit ANSI
color. It does not reject terminals based on advertised capabilities, so terminals
with limited color support may display an approximation. Terminals that understand
DEC synchronized output receive each frame atomically; other terminals safely
ignore those markers. Because color is the program's primary output, `NO_COLOR` is
intentionally ignored. If the terminal cannot enter raw or alternate-screen mode,
Rainbowave exits with an error instead of leaving a partial animation behind.

## Development

```sh
cargo fmt --all --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-targets --locked
cargo build --release --locked
```

GitHub Actions runs these checks on macOS and Linux. Pushing a tag such as
`v0.1.0` builds all supported binaries, creates SHA-256 checksums, and publishes
them to a GitHub release.

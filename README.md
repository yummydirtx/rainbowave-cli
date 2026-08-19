# Rainbowave CLI

Rainbowave fills your terminal with layered ASCII waves and a constantly flowing
truecolor rainbow. It is a small compiled utility designed to look especially
smooth in modern, GPU-accelerated terminal emulators.

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

Rainbowave targets modern macOS and Linux terminals. It emits 24-bit ANSI color
without rejecting terminals based on advertised capabilities, so terminals with
limited color support may display an approximation. Because color is the program's
primary output, `NO_COLOR` is intentionally ignored. If the terminal cannot enter
raw or alternate-screen mode, Rainbowave exits with an error instead of leaving a
partial animation behind.

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

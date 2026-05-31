# building and installing

inshellah is a rust crate. it builds with stock cargo on any platform
rust supports.

## with nix

```sh
nix build
```

binary is at `./result/bin/inshellah`.

development shell:

```sh
nix develop
cargo build --release
cargo test
```

## with cargo

requires rust >= 1.85 (edition 2024).

```sh
cargo build --release
cargo test
sudo install -Dm755 target/release/inshellah /usr/local/bin/inshellah
```

## arch linux

```sh
sudo pacman -S rust
cargo build --release
sudo install -Dm755 target/release/inshellah /usr/local/bin/inshellah
```

## debian / ubuntu

```sh
sudo apt install cargo rustc
# or: rustup install stable
cargo build --release
sudo install -Dm755 target/release/inshellah /usr/local/bin/inshellah
```

## fedora

```sh
sudo dnf install cargo rust
cargo build --release
sudo install -Dm755 target/release/inshellah /usr/local/bin/inshellah
```

## post-install setup

index completions from your system prefix(es):

```sh
# typical linux system
inshellah index /usr /usr/local

# more workers / different timeout
inshellah index /usr /usr/local --workers 16 --timeout-ms 500

# check what was indexed
inshellah dump
```

wire up the nushell completer in `~/.config/nushell/config.nu`:

```nu
source /path/to/inshellah-completer.nu
```

see [nushell-integration.md](nushell-integration.md) for full
completer details and [runtime-completions.md](runtime-completions.md)
for on-the-fly resolution of commands not covered by the upfront
index.

## re-indexing after package changes

```sh
inshellah index /usr /usr/local
```

on nixos, the system index regenerates on every `nixos-rebuild`. see
[nixos.md](nixos.md).

## development

```sh
cargo build           # debug build, faster compile
cargo test            # full test suite
cargo clippy --release
```

a `man` binary is useful at runtime as a fallback for locating
manpages outside the indexed prefixes — not required for indexing
itself.

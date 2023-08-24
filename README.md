cargo gc
--------------------

[![Crates.io](https://img.shields.io/crates/v/cargo-gc-bin)](https://crates.io/crates/cargo-gc-bin)
[![License](https://img.shields.io/badge/license-Apache%202.0-blue)](LICENSE-APACHE)

Cargo extension to recycle outdated build artifacts. And try the best to avoid recompilation.

# Usage

Install it with cargo:
```shell
cargo install cargo-gc-bin
```

The executable is `cargo-gc`. You can invoke it with `cargo gc` command:
```shell
cargo gc
```

It will check and remove all outdated build artifacts in the current project. See `cargo gc --help` for more information.

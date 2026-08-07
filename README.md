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

# Which artifacts to collect

To keep live artifacts alive, `cargo gc` runs a few `cargo` invocations of the
current toolchain and collects the produced artifact hashes. By default it
probes the target directory and only collects the intents that were actually
used there. If the directory holds no artifacts at all, gc warns and does
nothing instead of inventing artifacts to keep. The set can be overridden,
in decreasing precedence:

- CLI: `cargo gc --collect build --collect check`
- Env var: `CARGO_GC_COLLECT=build,check cargo gc`
- Manifest: `[package.metadata.cargo-gc] collect = ["build", "check"]`

Valid values are `build`, `check` and `test`. Collecting an intent that was
never used will first produce those artifacts and then keep them, so a
minimal set keeps the cache lean while a full set avoids recompilation.

# Limitations / Known issues
- [x] Invokes `cargo build`, `cargo check` and `cargo test --no-run` once to
	reconcile the cache with the current toolchain.
- [ ] Artifacts produced by a *running* `cargo test` (the lib-test harness of
	weird binaries) may be collected on the next run; recompilation there is
	cheap and limited.
- [ ] Some unknown entries are kept conservatively.

# Explaination

`cargo gc` invokes the *real* `cargo` of the current toolchain (the `CARGO`
environment variable set by the `cargo` proxy) with
`--message-format=json` and collects the file-name hashes of every produced
artifact. Any file or fingerprint directory whose hash is not part of that
collection is stale and gets removed. Because the collection always comes
from the same `cargo` that would build the project, results never drift with
toolchain updates.

Compare to other utils like `cargo sweep`, this one is based on the
informations provided by cargo itself rather than filesystem timestamp. So it
can be more accurate and still avoiding recompilation as much as possible.

# Next steps
Technically, it's possible to implement a "perfect" GC that can remove all outdated artifacts without any recompilation. And done this in a totally static way (i.e., without invoking `cargo build`).

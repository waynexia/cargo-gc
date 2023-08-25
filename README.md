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

# Limitations / Known issues
- [ ] It needs to invoke `cargo build` that takes lots of time.
- [ ] Need to re-link after GC
- [ ] `cargo check` will re-check from scratch

# Explaination

`cargo gc` uses the output information from `cargo build` to help recognize build artifacts in use, and removes all others. In the current implementation, top-level arficats are not recognized and leads to re-link after GC.

Compare to other utils like `cargo sweep`, this one is based on the informations provided by cargo itself rather than filesystem timestamp. So it can be more accurate and still avoiding recompilation as much as possible.

# Next steps
Technically, it's possible to implement a "perfect" GC that can remove all outdated artifacts without any recompilation. And done this in a totally static way (i.e., without invoking `cargo build`). As the tuple ("crate name", "fingreprint") can be computed outside of `cargo`.

//! Link options of the `erd` binary, and the `fd_walk` cfg.

/// On macOS, drops the dylibs the binary uses no symbol of: `libc` always links `libiconv`, and
/// `ctrlc` reaches `objc2`, which links Foundation and `libobjc`. dyld loads every linked
/// library at startup, which costs about as much as the rest of a small run.
///
/// Sets `cfg(fd_walk)` on Linux and macOS, where the walk works relative to directory file
/// descriptors (`src/fs/at/`); elsewhere it uses `std::fs`.
fn main() {
    println!("cargo::rerun-if-changed=src/build.rs");
    println!("cargo::rustc-check-cfg=cfg(fd_walk)");

    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();

    if os == "linux" || os == "macos" {
        println!("cargo::rustc-cfg=fd_walk");
    }

    if os == "macos" {
        println!("cargo::rustc-link-arg-bins=-Wl,-dead_strip_dylibs");
    }
}

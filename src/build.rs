//! Link options of the `erd` binary.

/// On macOS, drops the dylibs the binary uses no symbol of: `libc` always links `libiconv`, and
/// `ctrlc` reaches `objc2`, which links Foundation and `libobjc`. dyld loads every linked
/// library at startup, which costs about as much as the rest of a small run.
fn main() {
    println!("cargo:rerun-if-changed=src/build.rs");

    if std::env::var_os("CARGO_CFG_TARGET_OS").is_some_and(|os| os == "macos") {
        println!("cargo:rustc-link-arg-bins=-Wl,-dead_strip_dylibs");
    }
}

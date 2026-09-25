//! Embeds `packaging/identity/fsh-shell.manifest` into fsh-shell.exe. Its `<msix>` element
//! gives the exe package identity once the identity package is registered (needed for
//! reading notifications); without the package it changes nothing.

fn main() {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packaging/identity/fsh-shell.manifest");
    println!("cargo:rerun-if-changed={}", manifest.display());
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
        println!("cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}", manifest.display());
    }
}

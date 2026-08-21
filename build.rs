//! Build script: embed the Windows executable icon.
//!
//! `embed-resource` compiles `assets/app.rc` (which references
//! `assets/gemini.ico` via a standard `ICON` statement) into a
//! machine-appropriate resource object and links it into the binary via
//! `cargo:rustc-link-arg`. The rc file is used instead of passing the .ico
//! directly because the underlying compilers (rc.exe / windres) accept .ico
//! inputs inconsistently, while `ICON` statements are universally supported.
//!
//! Unlike a committed `.syso` file in the crate root — which rustc links
//! unconditionally for *every* Windows target and would break aarch64 builds
//! with an x64 machine-type mismatch — this generates the object for the
//! exact target being built.

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("assets/app.rc", embed_resource::NONE);
    }
    println!("cargo:rerun-if-changed=assets/app.rc");
    println!("cargo:rerun-if-changed=assets/gemini.ico");
}

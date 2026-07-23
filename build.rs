fn main() {
    if std::env::var("PROXYBRIDGE_BUNDLE").is_ok() {
        println!("cargo:rustc-cfg=proxybridge_bundled");
    }
    println!("cargo:rerun-if-env-changed=PROXYBRIDGE_BUNDLE");
}

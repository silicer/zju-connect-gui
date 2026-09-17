//! Build script.
//!
//! Two jobs:
//!
//! 1. Embed the Windows executable icon (`embed-resource`).
//!
//!    `embed-resource` compiles `assets/app.rc` (which references
//!    `assets/gemini.ico` via a standard `ICON` statement) into a
//!    machine-appropriate resource object and links it into the binary via
//!    `cargo:rustc-link-arg`. The rc file is used instead of passing the .ico
//!    directly because the underlying compilers (rc.exe / windres) accept .ico
//!    inputs inconsistently, while `ICON` statements are universally supported.
//!
//!    Unlike a committed `.syso` file in the crate root — which rustc links
//!    unconditionally for *every* Windows target and would break aarch64 builds
//!    with an x64 machine-type mismatch — this generates the object for the
//!    exact target being built.
//!
//! 2. On Linux, compile the vendored ProxyBridge / netfilter C stack (see
//!    `vendor/`) into a static archive and link it into the binary.
//!
//!    This is what makes a *static musl* release possible: a fully static musl
//!    binary has no dynamic loader at all, and musl's `dlopen` is a stub that
//!    always fails with "Dynamic loading not supported", so the shared library
//!    upstream ships can never be loaded there. Linking the stack in at build
//!    time removes the `dlopen` dependency entirely.
//!
//! ## Compiling the C for a musl target
//!
//! The C objects are linked into the same binary as the Rust code, so they must
//! be compiled against the *same* libc. The default host compiler is not good
//! enough for `*-linux-musl`: glibc's `<ctype.h>` alone expands `isspace()` into
//! a `__ctype_b_loc()` call that musl does not provide, and the final link then
//! fails with an undefined symbol. Point `CC_<target-triple>` at a musl
//! compiler instead:
//!
//! ```sh
//! CC_x86_64_unknown_linux_musl=musl-gcc cargo build --target x86_64-unknown-linux-musl
//! ```
//!
//! (`musl-gcc` is in the `musl-tools` package on Debian/Ubuntu. `zig cc` works
//! too, but not directly: its triples differ from Rust's — it wants
//! `x86_64-linux-musl` — while `cc` appends Rust's own `--target=...` to every
//! clang-family compiler it detects, and zig rejects that spelling. Wrap zig
//! and drop the extra flag if you want to use it. Bear in mind that `zig cc`
//! also turns on UBSan for its Debug mode, so a *debug* profile needs an
//! explicit `CFLAGS_<target>=-O2` or the link fails on `__ubsan_handle_*`.)

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Vendored libraries, pinned by directory name. See `vendor/README.md`.
const VENDOR_LIBS: [&str; 3] = [
    "libmnl-1.0.5",
    "libnfnetlink-1.0.2",
    "libnetfilter_queue-1.0.5",
];

/// Vendored ProxyBridge core, pinned by directory name.
const PROXYBRIDGE_DIR: &str = "proxybridge-3.2.0";

/// Vendored ProxyBridge core for Windows, pinned by directory name.
const PROXYBRIDGE_WIN_DIR: &str = "proxybridge-win-4.0.0";

/// Vendored WinDivert headers + export list (the DLL itself is shipped next to
/// the executable, exactly as before).
const WINDIVERT_DIR: &str = "windivert-2.2.2-A";

fn main() {
    println!("cargo:rerun-if-changed=assets/app.rc");
    println!("cargo:rerun-if-changed=assets/gemini.ico");

    // `proxybridge_native` marks the targets where the vendored core is compiled
    // into this binary: Linux, and Windows x86_64. Everywhere else — macOS, and
    // Windows arm64, for which upstream ships no WinDivert build — the
    // integration is stubbed out in Rust.
    println!("cargo::rustc-check-cfg=cfg(proxybridge_native)");
    let proxybridge_native = match env::var("CARGO_CFG_TARGET_OS").as_deref() {
        Ok("linux") => true,
        Ok("windows") => env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("x86_64"),
        _ => false,
    };
    if proxybridge_native {
        println!("cargo:rustc-cfg=proxybridge_native");
    }

    // Windows/MSVC only (a no-op on every other target): link the VCRuntime
    // statically while leaving the Universal CRT dynamic. The UCRT is part of
    // Windows 10+, but `vcruntime140.dll` ships with the VC++ Redistributable —
    // this keeps the release zip independent of it without paying the size cost
    // (and the UCRT pinning) of a fully static CRT.
    static_vcruntime::metabuild();

    match env::var("CARGO_CFG_TARGET_OS").as_deref() {
        Ok("windows") => {
            embed_resource::compile("assets/app.rc", embed_resource::NONE);
            // Windows arm64 has no WinDivert build, so ProxyBridge stays a stub
            // there and nothing is compiled.
            if env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("x86_64") {
                compile_windows_proxybridge();
            }
        }
        // macOS: upstream ships no reusable core library, so the integration is
        // stubbed out in Rust and nothing is compiled here.
        Ok("linux") => compile_proxybridge_stack(),
        _ => {}
    }
}

/// Compile the vendored Windows ProxyBridge core into a static archive.
///
/// Upstream ships `ProxyBridgeCore.dll`, which the app used to `dlopen`. The
/// DLL is built from exactly this source, carrying one local patch (the DNS
/// redirect, see `vendor/README.md`), so the app compiles it in instead: the
/// `ProxyBridge_*` symbols then resolve at link time and no DLL has to be
/// shipped next to the executable. WinDivert is untouched — it stays the
/// upstream DLL, loaded at run time by the code below.
fn compile_windows_proxybridge() {
    let manifest_dir =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set"));
    let vendor = manifest_dir.join("vendor");
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR is set"));
    let windivert = vendor.join(WINDIVERT_DIR);

    let mut build = cc::Build::new();
    build.include(&windivert);
    build.define("_WIN32_WINNT", "0x0601");
    build.define("PROXYBRIDGE_EXPORTS", None);
    // Pinned third-party source: not ours to police for warnings.
    build.warnings(false);
    build.file(vendor.join(PROXYBRIDGE_WIN_DIR).join("ProxyBridge.c"));

    // WinDivert is distributed as a DLL plus an MSVC import library. Neither is
    // usable as-is by every toolchain (and committing a binary blob is not the
    // point either), so the import library is generated from the vendored
    // export list with the tool that matches the compiler.
    generate_import_library(
        &windivert.join("windivert.def"),
        &out_dir,
        &build.get_compiler(),
    );

    build.compile("proxybridge");

    println!(
        "cargo:rustc-link-search=native={}",
        out_dir.to_string_lossy()
    );
    println!("cargo:rustc-link-lib=dylib=WinDivert");
    println!("cargo:rustc-link-lib=dylib=ws2_32");
    println!("cargo:rustc-link-lib=dylib=iphlpapi");

    // Headers and the export list are not tracked by `cc`.
    println!("cargo:rerun-if-changed={}", vendor.display());
}

/// Build an import library for `WinDivert.dll` from a `.def` export list.
///
/// `lib.exe`/`llvm-lib` produce the MSVC flavour that `link.exe` expects, and
/// `dlltool`/`llvm-dlltool` the GNU flavour for mingw. The first tool of each
/// family that works wins, so both supported Windows toolchains build offline
/// with no SDK download.
fn generate_import_library(def: &Path, out_dir: &Path, compiler: &cc::Tool) {
    let mut errors = Vec::new();

    if compiler.is_like_msvc() {
        // `lib.exe` sits next to the `cl.exe` that `cc` resolved, which is how
        // this works without a Visual Studio developer prompt.
        let lib_tool = compiler
            .path()
            .parent()
            .map(|dir| dir.join("lib.exe"))
            .filter(|path| path.is_file());
        let archive = out_dir.join("WinDivert.lib");
        let mut cmd = match &lib_tool {
            Some(tool) => Command::new(tool),
            None => Command::new("lib.exe"),
        };
        match cmd
            .arg(format!("/def:{}", def.display()))
            .arg(format!("/out:{}", archive.display()))
            .arg("/machine:x64")
            .status()
        {
            Ok(status) if status.success() => return,
            Ok(status) => errors.push(format!("lib.exe exited with {status}")),
            Err(e) => errors.push(format!("lib.exe: {e}")),
        }
    }

    let archive = out_dir.join("libWinDivert.a");
    let attempts: [(&str, Option<&str>); 3] = [
        ("llvm-dlltool", Some("i386:x86-64")),
        ("dlltool", None),
        ("x86_64-w64-mingw32-dlltool", None),
    ];
    for (tool, machine) in attempts {
        let mut cmd = Command::new(tool);
        if let Some(machine) = machine {
            cmd.arg("-m").arg(machine);
        }
        match cmd.arg("-d").arg(def).arg("-l").arg(&archive).status() {
            Ok(status) if status.success() => return,
            Ok(status) => errors.push(format!("{tool} exited with {status}")),
            Err(e) => errors.push(format!("{tool}: {e}")),
        }
    }

    panic!(
        "cannot build the WinDivert import library from {}: {}",
        def.display(),
        errors.join("; ")
    );
}

/// Compile the vendored ProxyBridge / netfilter stack into a static archive.
///
/// The result is emitted as `libproxybridge.a` plus the matching
/// `cargo:rustc-link-lib` / `cargo:rustc-link-search` directives, so the Rust
/// side only has to declare the `ProxyBridge_*` functions it calls.
fn compile_proxybridge_stack() {
    let manifest_dir =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set"));
    let vendor = manifest_dir.join("vendor");
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR is set"));

    warn_if_no_musl_cc();

    // Upstream's `src/internal.h` includes the autotools-generated `config.h`.
    // Nothing in it is needed here (its only interesting macro,
    // HAVE_VISIBILITY_HIDDEN, controls dynamic symbol export, which is moot for
    // statically linked code), but the include has to resolve.
    fs::write(
        out_dir.join("config.h"),
        "/* Substitutes the autotools-generated config.h. Intentionally empty. */\n",
    )
    .expect("failed to write config.h");

    let mut build = cc::Build::new();
    build.include(&out_dir);
    build.define("_GNU_SOURCE", None);
    build.flag_if_supported("-fPIC");
    // Pinned third-party sources: not ours to police for warnings.
    build.warnings(false);

    for lib in VENDOR_LIBS {
        let dir = vendor.join(lib);
        build.include(dir.join("include"));
        for src in c_sources(&dir.join("src")) {
            build.file(src);
        }
    }

    add_kernel_uapi_headers(&mut build);

    let proxybridge = vendor.join(PROXYBRIDGE_DIR);
    build.include(&proxybridge);
    build.file(proxybridge.join("ProxyBridge.c"));

    build.compile("proxybridge");

    // Headers are not tracked by `cc`, so watch the whole vendored tree.
    println!("cargo:rerun-if-changed={}", vendor.display());
}

/// Make the host's kernel UAPI headers reachable for musl builds.
///
/// `musl-gcc` is a `gcc -specs ...` wrapper whose specs replace the include
/// path wholesale (`-nostdinc -isystem /usr/include/<triple>-linux-musl`), so
/// the host's `/usr/include` is never searched. The vendored sources include
/// `<linux/netlink.h>` and friends, and libmnl's own copy of that header pulls
/// in `<linux/types.h>` -> `<asm/types.h>`, none of which musl ships: without
/// these directories the build dies on
/// `linux/types.h: No such file or directory`.
///
/// `-idirafter` puts them *after* every other directory, so musl's headers
/// always win and only the kernel UAPI headers (which are libc-agnostic) fall
/// through to the host. Debian and Ubuntu split those across `/usr/include` and
/// a multiarch directory, hence the second entry. Both are optional, so this
/// stays a no-op on hosts that lay them out differently — and on musl-native
/// hosts, where `/usr/include` is already on the default path.
fn add_kernel_uapi_headers(build: &mut cc::Build) {
    if env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("musl") {
        return;
    }

    // These are the *host's* headers, so they only make sense for a native
    // build — handing them to a cross build would describe the wrong
    // architecture. A real musl cross toolchain brings its own sysroot and
    // needs none of this.
    let Ok(arch) = env::var("CARGO_CFG_TARGET_ARCH") else {
        return;
    };
    if arch != env::consts::ARCH {
        return;
    }

    let candidates = [
        PathBuf::from("/usr/include"),
        PathBuf::from(format!("/usr/include/{arch}-linux-gnu")),
    ];

    for dir in candidates {
        if dir.is_dir() {
            build.flag("-idirafter").flag(dir);
        }
    }
}

/// Every `.c` file directly inside `dir`, in a stable order.
fn c_sources(dir: &Path) -> Vec<PathBuf> {
    let mut sources: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
        .map(|entry| entry.expect("readable directory entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "c"))
        .collect();
    sources.sort();
    sources
}

/// Warn when a musl target is being built with the default host compiler.
///
/// Cargo/`cc` fall back to `cc` from `PATH`, which on a glibc host produces
/// objects that cannot link into a musl binary. On a musl-native host (Alpine)
/// the default compiler is already correct, so this stays a warning rather than
/// an error.
fn warn_if_no_musl_cc() {
    if env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("musl") {
        return;
    }

    let target = env::var("TARGET").unwrap_or_default();
    let cc_var = format!("CC_{}", target.replace('-', "_"));
    let explicit = env::var_os(&cc_var).is_some()
        || env::var_os("TARGET_CC").is_some()
        || env::var_os("CC").is_some();

    if !explicit {
        println!(
            "cargo:warning=target `{target}` uses musl but no `{cc_var}`/`TARGET_CC`/`CC` is set, \
             so the vendored C in vendor/ will be compiled by the default host compiler. If that \
             compiler targets glibc the link will fail on symbols such as `__ctype_b_loc`. Set \
             `{cc_var}` to a musl compiler (e.g. `musl-gcc`, or `zig cc -target {target}`)."
        );
    }
}

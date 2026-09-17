# Vendored dependencies

Third-party C sources that are compiled into the Linux binary by `build.rs`
(see the "Compiling the C for a musl target" section there). They are committed
rather than fetched at build time so that a plain `cargo build` works offline,
and so the exact bytes that go into a release are reviewable in the repository.

Nothing here is used on Windows or macOS: Windows loads the prebuilt
`ProxyBridgeCore.dll`, and macOS is not supported at all.

## Why this is vendored instead of loaded at run time

Upstream ships `libproxybridge.so`, which the app used to load with `dlopen`.
That cannot work in the Linux release build: it is a **fully static musl**
binary, which has no dynamic loader at all — musl's `dlopen` is a stub that
always fails with `Dynamic loading not supported`. The whole stack is therefore
compiled and linked in at build time, and there is no `dlopen` left in the
binary.

## Contents

| Directory | Upstream | Version | License |
| --- | --- | --- | --- |
| `proxybridge-3.2.0/` | https://github.com/InterceptSuite/ProxyBridge (`Linux/src/`) | v3.2.0 | MIT |
| `libnetfilter_queue-1.0.5/` | https://www.netfilter.org/projects/libnetfilter_queue/ | 1.0.5 | GPL-2.0 |
| `libnfnetlink-1.0.2/` | https://www.netfilter.org/projects/libnfnetlink/ | 1.0.2 | GPL-2.0 |
| `libmnl-1.0.5/` | https://www.netfilter.org/projects/libmnl/ | 1.0.5 | LGPL-2.1 |

Each directory keeps upstream's `COPYING`/`LICENSE` next to the sources. Only
the files this build needs are kept (the `.c` sources and the headers they
include); autotools files, tests and documentation were dropped.

`libnetfilter_queue` needs `libnfnetlink` and `libmnl`; `libmnl` needs none of
them. `libnfnetlink` is included because 1.0.5 genuinely calls `nfnl_*`
symbols, even though upstream's example Makefile looks like it could drop it.

## Licensing

`libnetfilter_queue` and `libnfnetlink` are **GPL-2.0** and are linked
statically into the binary, so the **released Linux binary as a whole is
GPL-2.0** (this is a combined work, not mere aggregation as the previously
bundled separate `.so` was). The rest of this repository remains MIT; the MIT
terms simply cannot cover a binary that contains GPL-2.0 object code.

Anyone redistributing a Linux build must therefore ship the corresponding
source — which is precisely what this directory is — along with the license
texts.

## Local patches to upstream code

`proxybridge-3.2.0/ProxyBridge.c` carries two changes, each marked with a comment
at the site.

### 1. Portable `struct msghdr` initialization

Upstream initializes `struct msghdr` positionally:

```c
struct msghdr msg = {&sa, sizeof(sa), &iov, 1, NULL, 0, 0};
```

On 64-bit musl that does not compile: musl declares an anonymous `int __pad1`
member between `msg_iovlen` and `msg_control`, so the `NULL` ends up targeting an
`int` field (clang: "incompatible pointer to integer conversion"). Rewriting it
with designated initializers is portable across both libcs — glibc's
`struct msghdr` simply has no pad member to skip.

### 2. Exit-time cleanup only when the library was actually used

`library_cleanup` is an ELF destructor. Upstream builds this file as a *shared*
library, so it runs when a host unloads the library, and with `running` false it
fires four `iptables -D` invocations to clear rules left behind by an earlier
crash.

Linking it in statically changes the meaning: the destructor is registered in
`.fini_array` and now runs on **every** process exit — including for users who
never enabled ProxyBridge, each exit spawning four `iptables` child processes
that fail noisily for a non-root user.

The patch sets a `g_api_touched` flag in the six entry points this application
uses and makes the destructor return early while it is unset, so the cleanup
still happens for anyone who actually drove the library:

| | `iptables` spawns at process exit |
| --- | --- |
| upstream, linked in statically, API unused | 4 |
| patched, API unused | 0 |
| patched, API used | 4 |

Rules are only ever added by `ProxyBridge_Start` and removed by
`ProxyBridge_Stop`, so skipping the fallback when nothing called into the library
cannot leave anything behind.

## Updating

1. Download the new upstream release, drop in the sources and license, and
   delete the version you are replacing (keep the directory names in `build.rs`
   in sync).
2. Re-apply the two patches described above if upstream has not fixed them.
3. Rebuild for Linux and make sure the release binary is still static:
   `file` must report `statically linked`, with no `PT_INTERP`.

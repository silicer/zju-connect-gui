# Vendored dependencies

Third-party C sources that are compiled into the Linux binary by `build.rs`
(see the "Compiling the C for a musl target" section there). They are committed
rather than fetched at build time so that a plain `cargo build` works offline,
and so the exact bytes that go into a release are reviewable in the repository.

Everything here is compiled into the shipped binaries at build time: the Linux
tree on Linux, the Windows tree on Windows x86_64. macOS is not supported at
all, and Windows arm64 has no ProxyBridge core either — upstream ships no
WinDivert build for it.

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
| `proxybridge-win-4.0.0/` | https://github.com/InterceptSuite/ProxyBridge (`Windows/src/`) | v4.0.0 | MIT |
| `windivert-2.2.2-A/` | https://reqrypt.org/windivert.html | 2.2.2-A | LGPL-3.0 / GPL-2.0 |
| `libnetfilter_queue-1.0.5/` | https://www.netfilter.org/projects/libnetfilter_queue/ | 1.0.5 | GPL-2.0 |
| `libnfnetlink-1.0.2/` | https://www.netfilter.org/projects/libnfnetlink/ | 1.0.2 | GPL-2.0 |
| `libmnl-1.0.5/` | https://www.netfilter.org/projects/libmnl/ | 1.0.5 | LGPL-2.1 |

Each directory keeps upstream's `COPYING`/`LICENSE` next to the sources. Only
the files this build needs are kept (the `.c` sources and the headers they
include); autotools files, tests and documentation were dropped. The Windows
tree also keeps upstream's `compile.ps1`, as the reference for the compiler and
linker flags its DLL is normally built with; we do not run it.

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

`windivert-2.2.2-A/` is a different case: WinDivert is dual-licensed
**LGPL-3.0 or GPL-2.0**, and only its header (`windivert.h`) and its export
list (`windivert.def`) are vendored — enough to compile and link against the
DLL. `WinDivert.dll` and the signed `WinDivert64.sys` driver are still the
upstream binaries, redistributed untouched in the Windows package and loaded
dynamically, which is what the LGPL asks for.

## Local patches to upstream code

Two vendored trees are patched locally, each change marked with a comment at the
site: the Linux core `proxybridge-3.2.0/ProxyBridge.c`, and the Windows core
`proxybridge-win-4.0.0/`. The sections below keep their numbering per tree.

### Linux core: `proxybridge-3.2.0/ProxyBridge.c`

It carries three changes.

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

The patch sets a `g_api_touched` flag in every entry point this application
calls and makes the destructor return early while it is unset, so the cleanup
still happens for anyone who actually drove the library:

| | `iptables` spawns at process exit |
| --- | --- |
| upstream, linked in statically, API unused | 4 |
| patched, API unused | 0 |
| patched, API used | 4 |

Rules are only ever added by `ProxyBridge_Start` and removed by
`ProxyBridge_Stop`, so skipping the fallback when nothing called into the library
cannot leave anything behind.

### 3. Tunnel-resolved DNS for the listed processes

Proxying DNS as upstream does cannot put a listed process's lookup into the
tunnel. The SOCKS5 UDP relay puts the *original* destination in the SOCKS5
request, and zju-connect dials a destination it cannot reach through the tunnel
with a plain local socket (`dial/dialer_proxy.go`), so the query is answered on
the local network — and the unconditional `127.0.0.0/8` exemption in
`is_broadcast_or_multicast` sends a query aimed at a loopback stub resolver
(systemd-resolved's `127.0.0.53`, a local `dnsmasq`) direct before the proxy path
is even considered.

The patch adds `ProxyBridge_SetDnsRedirect(const char *ip, int port)`. For a
rule-matched **UDP port 53** packet, the connection table records that address
instead of the original destination, so the relay asks the SOCKS5 server for it
and the reply is matched back to the client — which still sees an answer from
the resolver it originally queried (the iptables `REDIRECT` keeps the conntrack
reverse mapping). Every destination is replaced, not just loopback ones: the
point is that a listed process resolves through the proxy whatever resolver it
was configured with. With no redirect configured, which is the default, nothing
changes.

Two deliberate limits: TCP port 53 is left alone (the DNS server this targets —
the core's `-dns-server-bind` listener — is UDP-only, and redirecting the TCP
retry after a truncated answer would turn a slow lookup into a failure), and
IPv6 DNS never reaches ProxyBridge at all, because it only installs IPv4
netfilter rules.

Only the application decides when to arm it: the switch next to the ProxyBridge
process list (off by default, so a stock launch behaves exactly like upstream),
and only after probing the listener — it logs `Starting DNS server at ...`
*before* binding, keeps running when the bind fails, and answers `NOERROR` with
an empty answer section while the tunnel is still coming up — an answer a stub
resolver caches as "no such name". With no redirect configured, which is the
default, the C patch is inert: `relay_dest_*` simply equals the original
destination on every path.

One thing to watch when updating upstream: PR #165 adds
`-t mangle -A OUTPUT -o lo -j ACCEPT` so that loopback traffic never reaches
NFQUEUE. That shortcut would make this patch a no-op (the packet path only runs
for queued packets), so it must not be re-applied ahead of it.

### Windows core: `proxybridge-win-4.0.0/`

It carries two changes. Upstream ships a prebuilt `ProxyBridgeCore.dll`; this
repository compiles the same source into the executable instead (see
`build.rs`), so the patches below are simply available at link time. WinDivert
is untouched and stays the upstream DLL.

#### 1. `__forceinline` under GCC and clang

mingw-w64 defines `__forceinline` as
`extern __inline__ __attribute__((__always_inline__,__gnu_inline__))`, which
cannot be combined with `static` ("multiple storage classes in declaration
specifiers"). MSVC, which upstream builds the DLL with, treats it as a
qualifier that does combine. The patch redefines it to plain `__inline__` for
GCC/clang only, so both toolchains compile the file.

#### 2. Tunnel-resolved DNS for the listed processes

Same problem as on Linux, with one difference: Windows has no `conntrack`, so
the reply cannot be left to a NAT layer.

Upstream records the client's destination in its connection table and asks the
SOCKS5 server for exactly that address, while zju-connect dials a destination it
cannot reach through the tunnel on the local machine — a lookup the local
resolver cannot answer therefore never reaches the tunnel.

The patch adds `ProxyBridge_SetDnsRedirect(const char *ip, int port)`. A
rule-matched **UDP port 53** flow gets a second destination
(`relay_dest_ip/port`) which is what the relay asks the SOCKS5 server for, while
`orig_dest_ip/port` keeps labelling the reply injected back to the client — that
is the address the client's socket expects to see. IPv6 flows are not
redirected: the SOCKS5 request is `ATYP_IPV4`.

One Windows-specific consequence: the resolver does not run inside the
requesting process, so a browser's rule never matches its own lookups — they are
emitted by the shared DNS Client service (`svchost.exe`). When the hijack is
enabled the application therefore adds a port-53-only rule for `svchost.exe`,
which makes DNS system-wide while ProxyBridge runs. That is unavoidable with a
shared resolver, and it is still scoped: only port 53 of that host process is
captured.

## Updating

1. Download the new upstream release, drop in the sources and license, and
   delete the version you are replacing (keep the directory names in `build.rs`
   in sync). The two cores live in separate upstream directories: `Linux/src/`
   and `Windows/src/`.
2. Re-apply the patches described above if upstream has not fixed them.
3. Rebuild and check both artefacts: on Linux `file` must still report
   `statically linked` with no `PT_INTERP`, and on Windows the executable must
   import `WinDivert.dll` but **not** `ProxyBridgeCore.dll` — the core is inside
   it. `x86_64-w64-mingw32-objdump -p zju-connect-gui.exe | grep "DLL Name"`
   shows this in one line.

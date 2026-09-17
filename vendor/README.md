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

`proxybridge-3.2.0/ProxyBridge.c` carries three changes, each marked with a
comment at the site.

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

Only the application decides when to arm it, and it does so only after probing
that listener: it logs `Starting DNS server at ...` *before* binding, keeps
running when the bind fails, and answers `NOERROR` with an empty answer section
while the tunnel is still coming up — an answer a stub resolver caches as "no
such name".

One thing to watch when updating upstream: PR #165 adds
`-t mangle -A OUTPUT -o lo -j ACCEPT` so that loopback traffic never reaches
NFQUEUE. That shortcut would make this patch a no-op (the packet path only runs
for queued packets), so it must not be re-applied ahead of it.

## Updating

1. Download the new upstream release, drop in the sources and license, and
   delete the version you are replacing (keep the directory names in `build.rs`
   in sync).
2. Re-apply the three patches described above if upstream has not fixed them.
3. Rebuild for Linux and make sure the release binary is still static:
   `file` must report `statically linked`, with no `PT_INTERP`.

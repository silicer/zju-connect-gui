/*
 * Added by this repository - not part of upstream ProxyBridge.
 * See vendor/README.md.
 *
 * Why this exists: WinDivert ships as a DLL that the package keeps in the
 * executable's `proxybridge/` directory (together with the signed
 * `WinDivert64.sys` driver). The Windows loader only searches the executable's
 * own directory, the system directories and PATH for a static import table, so
 * linking `WinDivert.lib` would make the *whole application* refuse to start
 * whenever the DLL is not next to the exe - the package layout would have to
 * change to accommodate it.
 *
 * Upstream's ProxyBridge.c is compiled with `WINDIVERTEXPORT=extern` (see
 * build.rs), which turns its WinDivert calls into ordinary extern references.
 * This file defines them and resolves the DLL on first use, preferring the
 * packaged location `<exe dir>\proxybridge\`, then `<exe dir>` (the layout the
 * app used to copy to), then the default search order. A missing DLL therefore
 * fails the ProxyBridge start with a log line instead of preventing the
 * process from launching at all.
 */

#include <windows.h>
#include "windivert.h"

static HMODULE g_windivert;

typedef HANDLE(WINAPI *wd_open_fn)(const char *, WINDIVERT_LAYER, INT16, UINT64);
typedef BOOL(WINAPI *wd_recv_fn)(HANDLE, VOID *, UINT, UINT *, WINDIVERT_ADDRESS *);
typedef BOOL(WINAPI *wd_send_fn)(HANDLE, const VOID *, UINT, UINT *, const WINDIVERT_ADDRESS *);
typedef BOOL(WINAPI *wd_shutdown_fn)(HANDLE, WINDIVERT_SHUTDOWN);
typedef BOOL(WINAPI *wd_close_fn)(HANDLE);
typedef BOOL(WINAPI *wd_set_param_fn)(HANDLE, WINDIVERT_PARAM, UINT64);
typedef BOOL(WINAPI *wd_calc_checksums_fn)(VOID *, UINT, WINDIVERT_ADDRESS *, UINT64);
typedef BOOL(WINAPI *wd_parse_packet_fn)(const VOID *, UINT, PWINDIVERT_IPHDR *,
                                         PWINDIVERT_IPV6HDR *, UINT8 *,
                                         PWINDIVERT_ICMPHDR *, PWINDIVERT_ICMPV6HDR *,
                                         PWINDIVERT_TCPHDR *, PWINDIVERT_UDPHDR *, PVOID *,
                                         UINT *, PVOID *, UINT *);

static wd_open_fn g_open;
static wd_recv_fn g_recv;
static wd_send_fn g_send;
static wd_shutdown_fn g_shutdown;
static wd_close_fn g_close;
static wd_set_param_fn g_set_param;
static wd_calc_checksums_fn g_calc_checksums;
static wd_parse_packet_fn g_parse_packet;

/* `dir` already ends with a separator. */
static BOOL join_path(wchar_t *out, size_t out_len, const wchar_t *dir, const wchar_t *name)
{
    size_t dir_len = wcslen(dir);
    size_t name_len = wcslen(name);
    if (dir_len + name_len + 1 > out_len)
        return FALSE;
    memcpy(out, dir, dir_len * sizeof(wchar_t));
    memcpy(out + dir_len, name, (name_len + 1) * sizeof(wchar_t));
    return TRUE;
}

static HMODULE load_windivert(void)
{
    wchar_t exe[MAX_PATH];
    DWORD len = GetModuleFileNameW(NULL, exe, MAX_PATH);

    if (len > 0 && len < MAX_PATH)
    {
        wchar_t *sep = wcsrchr(exe, L'\\');
        wchar_t candidate[MAX_PATH];
        if (sep != NULL)
            *(sep + 1) = L'\0'; /* keep the trailing separator */

        /* The packaged layout: <exe dir>\proxybridge\WinDivert.dll */
        if (join_path(candidate, MAX_PATH, exe, L"proxybridge\\WinDivert.dll") &&
            GetFileAttributesW(candidate) != INVALID_FILE_ATTRIBUTES)
        {
            HMODULE dll = LoadLibraryExW(candidate, NULL, LOAD_WITH_ALTERED_SEARCH_PATH);
            if (dll != NULL)
                return dll;
        }

        /* A copy next to the executable, from older releases. */
        if (join_path(candidate, MAX_PATH, exe, L"WinDivert.dll") &&
            GetFileAttributesW(candidate) != INVALID_FILE_ATTRIBUTES)
        {
            HMODULE dll = LoadLibraryExW(candidate, NULL, LOAD_WITH_ALTERED_SEARCH_PATH);
            if (dll != NULL)
                return dll;
        }
    }

    /* PATH / system directories, for a system-wide WinDivert install. */
    return LoadLibraryW(L"WinDivert.dll");
}

static BOOL ensure_loaded(void)
{
    if (g_windivert != NULL)
        return TRUE;

    HMODULE dll = load_windivert();
    if (dll == NULL)
    {
        /* The caller reports GetLastError(); say "file not found" so the
         * message names the missing piece rather than an unknown error. */
        SetLastError(ERROR_FILE_NOT_FOUND);
        return FALSE;
    }

    g_open = (wd_open_fn)GetProcAddress(dll, "WinDivertOpen");
    g_recv = (wd_recv_fn)GetProcAddress(dll, "WinDivertRecv");
    g_send = (wd_send_fn)GetProcAddress(dll, "WinDivertSend");
    g_shutdown = (wd_shutdown_fn)GetProcAddress(dll, "WinDivertShutdown");
    g_close = (wd_close_fn)GetProcAddress(dll, "WinDivertClose");
    g_set_param = (wd_set_param_fn)GetProcAddress(dll, "WinDivertSetParam");
    g_calc_checksums = (wd_calc_checksums_fn)GetProcAddress(dll, "WinDivertHelperCalcChecksums");
    g_parse_packet = (wd_parse_packet_fn)GetProcAddress(dll, "WinDivertHelperParsePacket");

    if (g_open == NULL || g_recv == NULL || g_send == NULL || g_shutdown == NULL ||
        g_close == NULL || g_set_param == NULL || g_calc_checksums == NULL ||
        g_parse_packet == NULL)
    {
        FreeLibrary(dll);
        SetLastError(ERROR_PROC_NOT_FOUND);
        return FALSE;
    }

    g_windivert = dll;
    return TRUE;
}

HANDLE WinDivertOpen(const char *filter, WINDIVERT_LAYER layer, INT16 priority, UINT64 flags)
{
    if (!ensure_loaded())
        return INVALID_HANDLE_VALUE;
    return g_open(filter, layer, priority, flags);
}

BOOL WinDivertRecv(HANDLE handle, VOID *pPacket, UINT packetLen, UINT *pRecvLen,
                   WINDIVERT_ADDRESS *pAddr)
{
    if (!ensure_loaded())
        return FALSE;
    return g_recv(handle, pPacket, packetLen, pRecvLen, pAddr);
}

BOOL WinDivertSend(HANDLE handle, const VOID *pPacket, UINT packetLen, UINT *pSendLen,
                   const WINDIVERT_ADDRESS *pAddr)
{
    if (!ensure_loaded())
        return FALSE;
    return g_send(handle, pPacket, packetLen, pSendLen, pAddr);
}

BOOL WinDivertShutdown(HANDLE handle, WINDIVERT_SHUTDOWN how)
{
    if (!ensure_loaded())
        return FALSE;
    return g_shutdown(handle, how);
}

BOOL WinDivertClose(HANDLE handle)
{
    if (!ensure_loaded())
        return FALSE;
    return g_close(handle);
}

BOOL WinDivertSetParam(HANDLE handle, WINDIVERT_PARAM param, UINT64 value)
{
    if (!ensure_loaded())
        return FALSE;
    return g_set_param(handle, param, value);
}

BOOL WinDivertHelperCalcChecksums(VOID *pPacket, UINT packetLen, WINDIVERT_ADDRESS *pAddr,
                                 UINT64 flags)
{
    if (!ensure_loaded())
        return FALSE;
    return g_calc_checksums(pPacket, packetLen, pAddr, flags);
}

BOOL WinDivertHelperParsePacket(const VOID *pPacket, UINT packetLen, PWINDIVERT_IPHDR *ppIpHdr,
                                PWINDIVERT_IPV6HDR *ppIpv6Hdr, UINT8 *pProtocol,
                                PWINDIVERT_ICMPHDR *ppIcmpHdr,
                                PWINDIVERT_ICMPV6HDR *ppIcmpv6Hdr,
                                PWINDIVERT_TCPHDR *ppTcpHdr, PWINDIVERT_UDPHDR *ppUdpHdr,
                                PVOID *ppData, UINT *pDataLen, PVOID *ppNext, UINT *pNextLen)
{
    if (!ensure_loaded())
        return FALSE;
    return g_parse_packet(pPacket, packetLen, ppIpHdr, ppIpv6Hdr, pProtocol, ppIcmpHdr,
                          ppIcmpv6Hdr, ppTcpHdr, ppUdpHdr, ppData, pDataLen, ppNext,
                          pNextLen);
}

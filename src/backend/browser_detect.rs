//! Detection of locally installed browsers + native executable picker.
//!
//! Two capabilities live here:
//!
//! 1. [`detect_installed_browsers`] scans well-known install locations (and
//!    the Windows registry's `StartMenuInternet` clients) so the UI can offer
//!    a dropdown of installed browsers instead of asking the user to paste a
//!    full path.
//! 2. [`pick_browser_file_dialog`] opens a **native** file-picker dialog from
//!    the backend process. This cannot be delegated to the web UI: browsers
//!    deliberately hide real filesystem paths (`C:\fakepath\...`), while the
//!    backend needs the absolute executable path to persist and later spawn.
//!    Each platform uses its stock tooling (PowerShell + WinForms on Windows,
//!    zenity/kdialog/yad on Linux, osascript on macOS) so no extra crate
//!    dependency is required.

use std::collections::HashSet;
use std::path::PathBuf;

use serde::Serialize;

/// Browser engine families we know how to steer from the command line.
///
/// The distinction matters in proxy-only mode: Chrome-family browsers accept
/// a `--proxy-server` switch, Firefox-family browsers need a temporary
/// profile with proxy prefs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BrowserKind {
    Chrome,
    Firefox,
}

impl BrowserKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            BrowserKind::Chrome => "chrome",
            BrowserKind::Firefox => "firefox",
        }
    }

    /// Command-line switch that opens a fresh browser window.
    pub fn new_window_flag(&self) -> &'static str {
        match self {
            BrowserKind::Chrome => "--new-window",
            BrowserKind::Firefox => "-new-window",
        }
    }
}

/// One detected browser installation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectedBrowser {
    /// Human-readable name, e.g. "Google Chrome".
    pub name: String,
    /// Absolute executable path.
    pub path: String,
    /// Engine family derived from the executable name.
    pub kind: BrowserKind,
}

// ── Classification ────────────────────────────────────────────────────

/// Substring tokens identifying Firefox-family binaries. Checked before the
/// Chrome tokens.
const FIREFOX_TOKENS: &[&str] = &["firefox", "librewolf", "waterfox", "tor-browser", "floorp"];
/// Short Firefox-family names that must match exactly ("zen" is too generic
/// to substring-match safely).
const FIREFOX_EXACT_NAMES: &[&str] = &["zen", "zen-alpha", "zen-browser"];
/// Substring tokens identifying Chrome/Chromium-family binaries.
const CHROME_TOKENS: &[&str] = &[
    "chrome",
    "chromium",
    "msedge",
    "microsoft-edge",
    "brave",
    "vivaldi",
    "opera",
];

/// Classify an executable path (or bare binary name) into a browser family.
///
/// Returns `None` for anything we do not recognize; callers decide on a
/// sensible fallback (the launcher assumes Chrome-family flags).
pub fn classify_browser(path: &str) -> Option<BrowserKind> {
    let name = file_stem_of(path);
    let lower = name.to_lowercase();
    if FIREFOX_EXACT_NAMES.contains(&lower.as_str()) {
        return Some(BrowserKind::Firefox);
    }
    if FIREFOX_TOKENS.iter().any(|t| lower.contains(t)) {
        return Some(BrowserKind::Firefox);
    }
    if CHROME_TOKENS.iter().any(|t| lower.contains(t)) {
        return Some(BrowserKind::Chrome);
    }
    None
}

/// File stem of a path, tolerating both separators and stripping `.exe`.
fn file_stem_of(path: &str) -> &str {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    name.strip_suffix(".exe").unwrap_or(name)
}

/// Pretty display name for a known binary name / install path.
fn display_name(path: &str) -> String {
    let lower = path.to_lowercase();
    let table: &[(&str, &str)] = &[
        ("google-chrome", "Google Chrome"),
        ("chrome", "Chrome"),
        ("chromium", "Chromium"),
        ("microsoft-edge", "Microsoft Edge"),
        ("msedge", "Microsoft Edge"),
        ("brave", "Brave"),
        ("vivaldi", "Vivaldi"),
        ("opera", "Opera"),
        ("firefox", "Firefox"),
        ("librewolf", "LibreWolf"),
        ("waterfox", "Waterfox"),
        ("zen", "Zen Browser"),
        ("tor-browser", "Tor Browser"),
        ("floorp", "Floorp"),
    ];
    for (token, label) in table {
        if lower.contains(token) {
            return (*label).to_string();
        }
    }
    file_stem_of(path).to_string()
}

// ── Detection entry point ─────────────────────────────────────────────

/// Detect installed browsers on this machine.
///
/// Results are deduplicated by path and ordered Chrome-family first (its
/// proxy handling is the most predictable), then alphabetically, so the
/// auto-pick in proxy-only mode is deterministic.
pub fn detect_installed_browsers() -> Vec<DetectedBrowser> {
    let mut out: Vec<DetectedBrowser> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    #[cfg(target_os = "windows")]
    {
        windows_registry_browsers(&mut out, &mut seen);
        windows_known_path_browsers(&mut out, &mut seen);
    }
    #[cfg(target_os = "macos")]
    {
        macos_known_path_browsers(&mut out, &mut seen);
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        unix_path_browsers(&mut out, &mut seen);
    }

    out.sort_by(|a, b| {
        let rank = |k: BrowserKind| match k {
            BrowserKind::Chrome => 0,
            BrowserKind::Firefox => 1,
        };
        rank(a.kind)
            .cmp(&rank(b.kind))
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.path.cmp(&b.path))
    });
    out
}

fn push_browser(
    out: &mut Vec<DetectedBrowser>,
    seen: &mut HashSet<String>,
    path: impl Into<PathBuf>,
) {
    let path = path.into();
    if !path.is_file() {
        return;
    }
    let text = path.to_string_lossy().into_owned();
    let key = text.to_lowercase();
    if !seen.insert(key) {
        return;
    }
    let Some(kind) = classify_browser(&text) else {
        return;
    };
    out.push(DetectedBrowser {
        name: display_name(&text),
        path: text,
        kind,
    });
}

// ── Windows: registry + known install dirs ────────────────────────────

#[cfg(target_os = "windows")]
mod win {
    use super::*;

    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::Foundation::{ERROR_SUCCESS, FILETIME};
    use windows::Win32::System::Registry::{
        RegCloseKey, RegEnumKeyExW, RegOpenKeyExW, RegQueryInfoKeyW, RegQueryValueExW, HKEY,
        HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, REG_SZ, REG_VALUE_TYPE,
    };

    const START_MENU_INTERNET: PCWSTR = windows::core::w!("SOFTWARE\\Clients\\StartMenuInternet");

    pub(super) fn registry_browsers(out: &mut Vec<DetectedBrowser>, seen: &mut HashSet<String>) {
        for hive in [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER] {
            collect_hive(hive, out, seen);
        }
    }

    fn collect_hive(hive: HKEY, out: &mut Vec<DetectedBrowser>, seen: &mut HashSet<String>) {
        unsafe {
            let mut root = HKEY::default();
            if RegOpenKeyExW(hive, START_MENU_INTERNET, 0, KEY_READ, &mut root) != ERROR_SUCCESS {
                return;
            }
            let mut subkeys = 0u32;
            let mut max_subkey_len = 0u32;
            let ok = RegQueryInfoKeyW(
                root,
                PWSTR::null(),
                None,
                None,
                Some(&mut subkeys),
                Some(&mut max_subkey_len),
                None,
                None,
                None,
                None,
                None,
                None,
            ) == ERROR_SUCCESS;
            if ok {
                for index in 0..subkeys {
                    collect_client(root, index, max_subkey_len, out, seen);
                }
            }
            let _ = RegCloseKey(root);
        }
    }

    /// Read one `StartMenuInternet\<client>` subkey: its default value is the
    /// display name and `shell\open\command` holds the launch command whose
    /// first token is the executable.
    fn collect_client(
        root: HKEY,
        index: u32,
        max_subkey_len: u32,
        out: &mut Vec<DetectedBrowser>,
        seen: &mut HashSet<String>,
    ) {
        unsafe {
            let mut name_buf = vec![0u16; max_subkey_len as usize + 1];
            let mut name_len = name_buf.len() as u32;
            if RegEnumKeyExW(
                root,
                index,
                PWSTR(name_buf.as_mut_ptr()),
                &mut name_len,
                None,
                PWSTR::null(),
                None,
                Some(std::ptr::null_mut::<FILETIME>()),
            ) != ERROR_SUCCESS
            {
                return;
            }
            let sub_name = String::from_utf16_lossy(&name_buf[..name_len as usize]);
            let wide: Vec<u16> = sub_name.encode_utf16().chain(Some(0)).collect();

            let mut client = HKEY::default();
            if RegOpenKeyExW(root, PCWSTR(wide.as_ptr()), 0, KEY_READ, &mut client) != ERROR_SUCCESS
            {
                return;
            }
            let command = open_subkey(client, "shell\\open\\command")
                .and_then(|k| read_sz(k, PCWSTR::null()));
            let _ = RegCloseKey(client);

            let Some(command) = command else { return };
            let Some(exe) = first_exe_from_command(&command) else {
                return;
            };
            push_browser(out, seen, PathBuf::from(exe));
        }
    }

    fn open_subkey(parent: HKEY, sub: &str) -> Option<HKEY> {
        unsafe {
            let wide: Vec<u16> = sub.encode_utf16().chain(Some(0)).collect();
            let mut key = HKEY::default();
            if RegOpenKeyExW(parent, PCWSTR(wide.as_ptr()), 0, KEY_READ, &mut key) != ERROR_SUCCESS
            {
                return None;
            }
            Some(key)
        }
    }

    /// Read a REG_SZ value (`value_name = null` reads the default value).
    fn read_sz(key: HKEY, value_name: PCWSTR) -> Option<String> {
        unsafe {
            let mut ty = REG_VALUE_TYPE::default();
            let mut size = 0u32;
            if RegQueryValueExW(key, value_name, None, Some(&mut ty), None, Some(&mut size))
                != ERROR_SUCCESS
            {
                return None;
            }
            if ty != REG_SZ || size == 0 || size > 8192 {
                return None;
            }
            let mut buf = vec![0u8; size as usize];
            let mut got = size;
            if RegQueryValueExW(
                key,
                value_name,
                None,
                None,
                Some(buf.as_mut_ptr()),
                Some(&mut got),
            ) != ERROR_SUCCESS
            {
                return None;
            }
            let units = &buf[..got.min(size) as usize];
            let pairs: &[u16] =
                std::slice::from_raw_parts(units.as_ptr().cast::<u16>(), units.len() / 2);
            Some(
                String::from_utf16_lossy(pairs)
                    .trim_end_matches('\0')
                    .to_string(),
            )
        }
    }

    /// `"C:\...\firefox.exe" -osint -default-browser` → the quoted exe; bare
    /// commands fall back to the first whitespace-separated token.
    fn first_exe_from_command(command: &str) -> Option<String> {
        let trimmed = command.trim();
        if let Some(rest) = trimmed.strip_prefix('"') {
            if let Some(end) = rest.find('"') {
                return Some(rest[..end].to_string());
            }
        }
        trimmed.split_whitespace().next().map(str::to_string)
    }

    /// Fallback scan covering portable installs the registry may not list.
    pub(super) fn known_path_browsers(out: &mut Vec<DetectedBrowser>, seen: &mut HashSet<String>) {
        let pf = std::env::var("ProgramFiles").unwrap_or_else(|_| r"C:\Program Files".into());
        let pf86 =
            std::env::var("ProgramFiles(x86)").unwrap_or_else(|_| r"C:\Program Files (x86)".into());
        let local = std::env::var("LocalAppData").unwrap_or_default();
        let combos: Vec<(String, String)> = vec![
            (pf.clone(), r"Google\Chrome\Application\chrome.exe".into()),
            (pf86.clone(), r"Google\Chrome\Application\chrome.exe".into()),
            (
                local.clone(),
                r"Google\Chrome\Application\chrome.exe".into(),
            ),
            (
                pf86.clone(),
                r"Microsoft\Edge\Application\msedge.exe".into(),
            ),
            (pf.clone(), r"Microsoft\Edge\Application\msedge.exe".into()),
            (pf.clone(), r"Mozilla Firefox\firefox.exe".into()),
            (pf86.clone(), r"Mozilla Firefox\firefox.exe".into()),
            (local.clone(), r"Mozilla Firefox\firefox.exe".into()),
            (
                pf.clone(),
                r"BraveSoftware\Brave-Browser\Application\brave.exe".into(),
            ),
            (
                local.clone(),
                r"BraveSoftware\Brave-Browser\Application\brave.exe".into(),
            ),
            (pf.clone(), r"Vivaldi\Application\vivaldi.exe".into()),
            (local.clone(), r"Vivaldi\Application\vivaldi.exe".into()),
            (pf86.clone(), r"Opera\opera.exe".into()),
            (local.clone(), r"Programs\Opera\opera.exe".into()),
            (pf.clone(), r"Chromium\Application\chrome.exe".into()),
        ];
        for (base, rel) in combos {
            push_browser(out, seen, PathBuf::from(base).join(rel));
        }
    }
}

#[cfg(target_os = "windows")]
use win::{
    known_path_browsers as windows_known_path_browsers,
    registry_browsers as windows_registry_browsers,
};

// ── macOS: /Applications scan ─────────────────────────────────────────

#[cfg(target_os = "macos")]
fn macos_known_path_browsers(out: &mut Vec<DetectedBrowser>, seen: &mut HashSet<String>) {
    let applications = ["/Applications", "~/Applications"];
    let apps: &[(&str, &str)] = &[
        ("Google Chrome.app", "Google Chrome"),
        ("Microsoft Edge.app", "Microsoft Edge"),
        ("Chromium.app", "Chromium"),
        ("Brave Browser.app", "Brave Browser"),
        ("Vivaldi.app", "Vivaldi"),
        ("Opera.app", "Opera"),
        ("Firefox.app", "firefox"),
        ("Firefox Developer Edition.app", "firefox"),
        ("LibreWolf.app", "librewolf"),
        ("Waterfox.app", "waterfox"),
        ("Zen Browser.app", "zen"),
    ];
    for base in applications {
        let base = shellexpand_home(base);
        for (dir, exe) in apps {
            push_browser(
                out,
                seen,
                PathBuf::from(&base)
                    .join(dir)
                    .join("Contents/MacOS")
                    .join(exe),
            );
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn shellexpand_home(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(path)
}

// ── Linux/BSD: PATH + well-known directories ──────────────────────────

#[cfg(all(unix, not(target_os = "macos")))]
const UNIX_CHROME_CANDIDATES: &[&str] = &[
    "google-chrome",
    "google-chrome-stable",
    "google-chrome-beta",
    "google-chrome-unstable",
    "chromium",
    "chromium-browser",
    "microsoft-edge",
    "microsoft-edge-stable",
    "microsoft-edge-beta",
    "brave-browser",
    "brave-browser-stable",
    "brave",
    "vivaldi",
    "vivaldi-stable",
    "opera",
];

#[cfg(all(unix, not(target_os = "macos")))]
const UNIX_FIREFOX_CANDIDATES: &[&str] = &[
    "firefox",
    "firefox-esr",
    "firefox-nightly",
    "firefox-developer-edition",
    "librewolf",
    "waterfox",
    "zen-browser",
    "zen",
    "tor-browser",
];

#[cfg(all(unix, not(target_os = "macos")))]
fn unix_path_browsers(out: &mut Vec<DetectedBrowser>, seen: &mut HashSet<String>) {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(paths) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&paths));
    }
    dirs.extend([
        PathBuf::from("/usr/local/bin"),
        PathBuf::from("/snap/bin"),
        PathBuf::from("/opt/google/chrome"),
        PathBuf::from("/opt/microsoft/msedge"),
        PathBuf::from("/opt/vivaldi"),
        PathBuf::from("/opt/BraveSoftware"),
        shellexpand_home("~/bin"),
    ]);

    for name in UNIX_CHROME_CANDIDATES.iter().chain(UNIX_FIREFOX_CANDIDATES) {
        for dir in &dirs {
            let candidate = dir.join(name);
            if is_executable_file(&candidate) {
                push_browser(out, seen, candidate);
                break;
            }
        }
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn is_executable_file(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    match std::fs::metadata(path) {
        Ok(meta) => meta.is_file() && meta.permissions().mode() & 0o111 != 0,
        Err(_) => false,
    }
}

// ── Native file-picker dialog ─────────────────────────────────────────

/// Outcome of one native picker attempt.
#[cfg(not(target_os = "windows"))]
enum PickerOutcome {
    /// The helper program does not exist on this machine.
    Missing,
    /// The helper ran but the user cancelled (or dismissed) the dialog.
    Cancelled,
    /// The user picked a file.
    Selected(String),
}

#[cfg(not(target_os = "windows"))]
impl PickerOutcome {
    fn from_output(result: std::io::Result<std::process::Output>) -> Self {
        match result {
            Err(_) => PickerOutcome::Missing,
            Ok(output) => {
                let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if output.status.success() && !text.is_empty() {
                    PickerOutcome::Selected(text)
                } else {
                    PickerOutcome::Cancelled
                }
            }
        }
    }
}

/// Open a native file-picker and let the user choose a browser executable.
///
/// Returns `Ok(None)` when the user cancelled the dialog, and an `Err` only
/// when no picker mechanism is available at all. Must be invoked off the main
/// async runtime (it blocks until the dialog closes).
pub fn pick_browser_file_dialog() -> Result<Option<String>, String> {
    #[cfg(target_os = "windows")]
    {
        windows_picker()
    }
    #[cfg(target_os = "macos")]
    {
        macos_picker()
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        linux_picker()
    }
}

#[cfg(target_os = "windows")]
fn windows_picker() -> Result<Option<String>, String> {
    // WinForms OpenFileDialog via PowerShell: present on every supported
    // Windows install, no extra dependencies. UTF-8 console output keeps
    // non-ASCII install paths intact.
    let script = concat!(
        "[Console]::OutputEncoding=[System.Text.Encoding]::UTF8;",
        "Add-Type -AssemblyName System.Windows.Forms | Out-Null;",
        "$d = New-Object System.Windows.Forms.OpenFileDialog;",
        "$d.Title = '选择浏览器可执行文件';",
        "$d.Filter = '可执行程序 (*.exe)|*.exe|所有文件 (*.*)|*.*';",
        "if ($d.ShowDialog() -eq [System.Windows.Forms.DialogResult]::OK)",
        "{ [Console]::Out.Write($d.FileName) }"
    );
    let output = std::process::Command::new("powershell")
        .args(["-NoProfile", "-STA", "-NonInteractive", "-Command", script])
        .output()
        .map_err(|e| format!("无法启动 PowerShell 文件对话框: {e}"))?;
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if output.status.success() && !text.is_empty() {
        Ok(Some(text))
    } else {
        Ok(None) // user cancelled the dialog
    }
}

#[cfg(target_os = "macos")]
fn macos_picker() -> Result<Option<String>, String> {
    let output = std::process::Command::new("osascript")
        .args([
            "-e",
            r#"POSIX path of (choose file with prompt "选择浏览器可执行文件")"#,
        ])
        .output()
        .map_err(|e| format!("无法启动 macOS 文件对话框: {e}"))?;
    match PickerOutcome::from_output(Ok(output)) {
        PickerOutcome::Selected(path) => Ok(Some(path)),
        _ => Ok(None),
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn linux_picker() -> Result<Option<String>, String> {
    const TITLE: &str = "选择浏览器可执行文件";

    // zenity (GNOME et al.)
    let outcome = PickerOutcome::from_output(
        std::process::Command::new("zenity")
            .args(["--file-selection", "--title", TITLE])
            .output(),
    );
    if let PickerOutcome::Selected(path) = outcome {
        return Ok(Some(path));
    }
    // kdialog (KDE)
    let outcome = PickerOutcome::from_output(
        std::process::Command::new("kdialog")
            .args(["--getopenfilename", ".", "--title", TITLE])
            .output(),
    );
    if let PickerOutcome::Selected(path) = outcome {
        return Ok(Some(path));
    }
    // yad (lightweight GTK fork of zenity)
    let outcome = PickerOutcome::from_output(
        std::process::Command::new("yad")
            .args(["--file", "--title", TITLE])
            .output(),
    );
    if let PickerOutcome::Selected(path) = outcome {
        return Ok(Some(path));
    }

    let missing = ["zenity", "kdialog", "yad"]
        .iter()
        .all(|tool| which_missing(tool));
    if missing {
        Err(
            "未找到可用的文件选择工具（zenity/kdialog/yad），请改用下方检测列表或手动填写路径"
                .into(),
        )
    } else {
        Ok(None) // a helper existed but the user cancelled
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn which_missing(tool: &str) -> bool {
    std::process::Command::new(tool)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_err()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_recognizes_chrome_family() {
        assert_eq!(classify_browser("chrome.exe"), Some(BrowserKind::Chrome));
        assert_eq!(
            classify_browser(r"C:\Program Files\Google\Chrome\Application\chrome.exe"),
            Some(BrowserKind::Chrome)
        );
        assert_eq!(classify_browser("chromium"), Some(BrowserKind::Chrome));
        assert_eq!(
            classify_browser(r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe"),
            Some(BrowserKind::Chrome)
        );
        assert_eq!(
            classify_browser("/usr/bin/microsoft-edge-stable"),
            Some(BrowserKind::Chrome)
        );
        assert_eq!(classify_browser("brave-browser"), Some(BrowserKind::Chrome));
        assert_eq!(
            classify_browser("/usr/bin/vivaldi-stable"),
            Some(BrowserKind::Chrome)
        );
        assert_eq!(classify_browser("opera"), Some(BrowserKind::Chrome));
    }

    #[test]
    fn classify_recognizes_firefox_family() {
        assert_eq!(classify_browser("firefox"), Some(BrowserKind::Firefox));
        assert_eq!(
            classify_browser(r"C:\Program Files\Mozilla Firefox\firefox.exe"),
            Some(BrowserKind::Firefox)
        );
        assert_eq!(classify_browser("firefox-esr"), Some(BrowserKind::Firefox));
        assert_eq!(
            classify_browser("/usr/bin/librewolf"),
            Some(BrowserKind::Firefox)
        );
        assert_eq!(classify_browser("waterfox"), Some(BrowserKind::Firefox));
        assert_eq!(
            classify_browser("/usr/bin/tor-browser"),
            Some(BrowserKind::Firefox)
        );
        // Short name must match exactly, not by substring.
        assert_eq!(classify_browser("/usr/bin/zen"), Some(BrowserKind::Firefox));
        assert_eq!(
            classify_browser("/usr/bin/zen-browser"),
            Some(BrowserKind::Firefox)
        );
    }

    #[test]
    fn classify_rejects_unknown_binaries() {
        assert_eq!(classify_browser(""), None);
        assert_eq!(classify_browser("/usr/bin/notepad"), None);
        assert_eq!(classify_browser(r"C:\Windows\System32\cmd.exe"), None);
        assert_eq!(classify_browser("/usr/bin/python3"), None);
    }

    #[test]
    fn display_name_maps_known_binaries() {
        assert_eq!(
            display_name("/usr/bin/google-chrome-stable"),
            "Google Chrome"
        );
        assert_eq!(
            display_name(r"C:\Program Files\Mozilla Firefox\firefox.exe"),
            "Firefox"
        );
        assert_eq!(display_name("/opt/somethingweird/bin/custom"), "custom");
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn detect_returns_sorted_results_without_panicking() {
        use std::path::Path;
        let browsers = detect_installed_browsers();
        for pair in browsers.windows(2) {
            let rank = |k: BrowserKind| match k {
                BrowserKind::Chrome => 0,
                BrowserKind::Firefox => 1,
            };
            assert!(
                (rank(pair[0].kind), pair[0].name.as_str())
                    <= (rank(pair[1].kind), pair[1].name.as_str())
            );
        }
        // Every reported path must exist and be classifiable.
        for b in &browsers {
            assert!(Path::new(&b.path).is_file(), "{}", b.path);
        }
    }
}

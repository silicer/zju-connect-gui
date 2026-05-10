//! Linux tray-icon controller built on `ksni`. ksni implements the
//! StatusNotifierItem D-Bus protocol natively in Rust (no GTK dependency),
//! and the `blocking` feature gives us a thread-spawning API that integrates
//! cleanly without an explicit async runtime.
//!
//! Per the StatusNotifierItem spec, the `Activate` signal corresponds to a
//! left-click and the `ContextMenu` request to a right-click. We wire
//! `activate` to "show window" and provide the explicit show/hide/quit menu
//! items so the user always has predictable controls regardless of how the
//! desktop shell renders click intents.

use ksni::blocking::{Handle, TrayMethods};
use ksni::menu::{MenuItem, StandardItem};
use ksni::{Icon, ToolTip, Tray};
use slint::Weak;

use crate::tray::{dispatch_hide, dispatch_quit, dispatch_show, TrayError, ICON_BYTES};
use crate::AppWindow;

pub struct TrayController {
    handle: Handle<ZjuTray>,
}

impl Drop for TrayController {
    fn drop(&mut self) {
        // Don't block on the awaiter — process tear-down doesn't need to wait
        // for the dbus exchange to finish. Issuing the request is enough.
        let _ = self.handle.shutdown();
    }
}

impl TrayController {
    pub fn new(weak: Weak<AppWindow>) -> Result<Self, TrayError> {
        let icon = build_icon()?;
        let tray = ZjuTray { weak, icon };
        let handle = tray
            .spawn()
            .map_err(|err| TrayError::Build(format!("ksni spawn: {err}")))?;
        Ok(Self { handle })
    }
}

struct ZjuTray {
    weak: Weak<AppWindow>,
    icon: Icon,
}

impl Tray for ZjuTray {
    fn id(&self) -> String {
        "zju-connect-gui".into()
    }

    fn title(&self) -> String {
        "ZJU Connect GUI".into()
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        vec![self.icon.clone()]
    }

    fn tool_tip(&self) -> ToolTip {
        ToolTip {
            title: "ZJU Connect GUI".into(),
            description: String::new(),
            icon_name: String::new(),
            icon_pixmap: vec![],
        }
    }

    // Left-click on the tray icon. Always show — hiding is reserved for the
    // explicit menu entry below to avoid surprising disappear-on-click cases.
    fn activate(&mut self, _x: i32, _y: i32) {
        dispatch_show(self.weak.clone());
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        vec![
            StandardItem {
                label: "显示主窗口".into(),
                activate: Box::new(|tray: &mut ZjuTray| dispatch_show(tray.weak.clone())),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "隐藏到托盘".into(),
                activate: Box::new(|tray: &mut ZjuTray| dispatch_hide(tray.weak.clone())),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "退出".into(),
                activate: Box::new(|tray: &mut ZjuTray| dispatch_quit(tray.weak.clone())),
                ..Default::default()
            }
            .into(),
        ]
    }
}

fn build_icon() -> Result<Icon, TrayError> {
    let img = image::load_from_memory(ICON_BYTES)
        .map_err(|err| TrayError::Icon(err.to_string()))?
        .to_rgba8();
    let (width, height) = img.dimensions();
    let mut data = img.into_raw();
    // ksni::Icon expects ARGB32 in network byte order (i.e. memory layout
    // [A, R, G, B]). image gives us [R, G, B, A]; rotating right by one byte
    // moves alpha from the tail to the head. (Trick lifted from the ksni docs.)
    for pixel in data.chunks_exact_mut(4) {
        pixel.rotate_right(1);
    }
    Ok(Icon {
        width: width as i32,
        height: height as i32,
        data,
    })
}

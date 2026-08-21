//! Linux tray-icon controller built on `ksni`. Pure-Rust StatusNotifierItem
//! over D-Bus — no GTK dependency.
//!
//! Left-click (Activate) opens the web UI. The context menu provides explicit
//! "打开网页" and "退出" actions.

use ksni::blocking::{Handle, TrayMethods};
use ksni::menu::{MenuItem, StandardItem};
use ksni::{Icon, ToolTip, Tray};
use tokio::sync::oneshot;

use crate::tray::{open_web_ui, TrayError, ICON_BYTES};

pub struct TrayController {
    handle: Handle<ZjuTray>,
    _quit_tx: Option<oneshot::Sender<()>>,
}

impl Drop for TrayController {
    fn drop(&mut self) {
        let _ = self.handle.shutdown();
    }
}

impl TrayController {
    /// Create the tray icon. Returns the controller plus a oneshot receiver
    /// that fires when the user clicks "退出".
    pub fn new(port: u16, token: &str) -> Result<(Self, oneshot::Receiver<()>), TrayError> {
        let (quit_tx, quit_rx) = oneshot::channel();
        let icon = build_icon()?;
        let tray = ZjuTray {
            port,
            token: token.to_string(),
            quit_tx: Some(quit_tx),
            icon,
        };
        let handle = tray
            .spawn()
            .map_err(|err| TrayError::Build(format!("ksni spawn: {err}")))?;
        Ok((
            Self {
                handle,
                _quit_tx: None,
            },
            quit_rx,
        ))
    }
}

struct ZjuTray {
    port: u16,
    token: String,
    quit_tx: Option<oneshot::Sender<()>>,
    icon: Icon,
}

impl Tray for ZjuTray {
    fn id(&self) -> String {
        "zju-connect-gui".into()
    }

    fn title(&self) -> String {
        "ZJU Connect".into()
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        vec![self.icon.clone()]
    }

    fn tool_tip(&self) -> ToolTip {
        ToolTip {
            title: format!("ZJU Connect - http://localhost:{}", self.port),
            description: String::new(),
            icon_name: String::new(),
            icon_pixmap: vec![],
        }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        open_web_ui(self.port, &self.token);
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        vec![
            StandardItem {
                label: "打开网页".into(),
                activate: Box::new(|tray: &mut ZjuTray| open_web_ui(tray.port, &tray.token)),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "退出".into(),
                activate: Box::new(|tray: &mut ZjuTray| {
                    if let Some(tx) = tray.quit_tx.take() {
                        let _ = tx.send(());
                    }
                }),
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
    for pixel in data.as_chunks_mut::<4>().0 {
        pixel.rotate_right(1);
    }
    Ok(Icon {
        width: width as i32,
        height: height as i32,
        data,
    })
}

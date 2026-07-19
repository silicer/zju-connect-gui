use tokio::sync::mpsc::Sender;

use crate::tray::{dispatch_quit, dispatch_show, TrayError, ICON_BYTES};
use ksni::blocking::TrayMethods;
use ksni::{MenuItem, Tray};

pub struct TrayController {
    _handle: ksni::blocking::Handle<AppTray>,
}

impl TrayController {
    pub fn new(url: String, quit_tx: Sender<()>) -> Result<Self, TrayError> {
        let tray = AppTray {
            url,
            quit_tx,
            icon_data: build_icon_data()?,
        };

        let handle = std::thread::spawn(move || {
            tray.spawn()
                .map_err(|err| TrayError::Build(err.to_string()))
        })
        .join()
        .unwrap()?;

        Ok(Self { _handle: handle })
    }
}

struct AppTray {
    url: String,
    quit_tx: Sender<()>,
    #[allow(dead_code)]
    icon_data: Vec<u8>,
}

impl Tray for AppTray {
    fn id(&self) -> String {
        "zju-connect-gui".into()
    }

    fn icon_name(&self) -> String {
        "network-vpn".into()
    }

    fn title(&self) -> String {
        "ZJU Connect GUI".into()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: "ZJU Connect GUI".into(),
            description: "Native desktop GUI for zju-connect".into(),
            ..Default::default()
        }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        dispatch_show(self.url.clone());
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        use ksni::menu::*;
        vec![
            StandardItem {
                label: "打开 EIP".into(),
                activate: Box::new(|_| {
                    let _ = open::that("https://eip.zju.edu.360.cn/");
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "打开控制界面".into(),
                activate: Box::new(|this: &mut Self| {
                    dispatch_show(this.url.clone());
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "退出".into(),
                activate: Box::new(|this: &mut Self| {
                    dispatch_quit(this.quit_tx.clone());
                }),
                ..Default::default()
            }
            .into(),
        ]
    }
}

fn build_icon_data() -> Result<Vec<u8>, TrayError> {
    Ok(ICON_BYTES.to_vec()) // For simplicity. Actual icon_data requires RGBA payload in ksni.
}

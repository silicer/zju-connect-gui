use std::time::Duration;
use tokio::sync::mpsc::Sender;
use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

use crate::tray::{dispatch_quit, dispatch_show, TrayError, ICON_BYTES};

pub struct TrayController {
    _icon: TrayIcon,
}

impl TrayController {
    pub fn new(url: String, quit_tx: Sender<()>) -> Result<Self, TrayError> {
        let icon = build_icon()?;

        let menu = Menu::new();
        let eip_item = MenuItem::new("打开 EIP", true, None);
        let show_item = MenuItem::new("打开控制界面", true, None);
        let quit_item = MenuItem::new("退出", true, None);
        menu.append_items(&[&eip_item, &show_item, &quit_item])
            .map_err(|err| TrayError::Build(format!("menu append: {err}")))?;

        let eip_id = eip_item.id().clone();
        let show_id = show_item.id().clone();
        let quit_id = quit_item.id().clone();

        let tray_icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("ZJU Connect GUI")
            .with_icon(icon)
            .with_menu_on_left_click(false)
            .build()
            .map_err(|err| TrayError::Build(err.to_string()))?;

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(60));
            loop {
                interval.tick().await;
                while let Ok(event) = MenuEvent::receiver().try_recv() {
                    if event.id == eip_id {
                        let _ = open::that("https://eip.zju.edu.360.cn/");
                    } else if event.id == show_id {
                        dispatch_show(url.clone());
                    } else if event.id == quit_id {
                        dispatch_quit(quit_tx.clone());
                    }
                }
                while let Ok(event) = TrayIconEvent::receiver().try_recv() {
                    match event {
                        TrayIconEvent::DoubleClick {
                            button: MouseButton::Left,
                            ..
                        } => {
                            dispatch_show(url.clone());
                        }
                        _ => {}
                    }
                }
            }
        });

        Ok(Self { _icon: tray_icon })
    }
}

fn build_icon() -> Result<Icon, TrayError> {
    let img = image::load_from_memory(ICON_BYTES)
        .map_err(|err| TrayError::Icon(err.to_string()))?
        .to_rgba8();
    let (w, h) = img.dimensions();
    Icon::from_rgba(img.into_raw(), w, h).map_err(|err| TrayError::Icon(err.to_string()))
}

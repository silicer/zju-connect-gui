//! Tray-icon-based controller used on Windows and macOS. Owns the OS tray icon
//! plus a Slint timer that pumps the global `MenuEvent` and `TrayIconEvent`
//! receivers (60 ms cadence) and forwards them to the AppWindow weak handle
//! via `slint::invoke_from_event_loop`. Left single- or double-click restores
//! the window; the context menu opens on right-click.

use std::time::Duration;

use slint::Weak;
use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

use crate::tray::{dispatch_hide, dispatch_quit, dispatch_show, TrayError, ICON_BYTES};
use crate::AppWindow;

const POLL_INTERVAL: Duration = Duration::from_millis(60);

pub struct TrayController {
    _icon: TrayIcon,
    _timer: slint::Timer,
}

impl TrayController {
    pub fn new(weak: Weak<AppWindow>) -> Result<Self, TrayError> {
        let icon = build_icon()?;

        let menu = Menu::new();
        let show_item = MenuItem::new("显示主窗口", true, None);
        let hide_item = MenuItem::new("隐藏到托盘", true, None);
        let quit_item = MenuItem::new("退出", true, None);
        menu.append_items(&[&show_item, &hide_item, &quit_item])
            .map_err(|err| TrayError::Build(format!("menu append: {err}")))?;
        let show_id = show_item.id().clone();
        let hide_id = hide_item.id().clone();
        let quit_id = quit_item.id().clone();

        let tray_icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("ZJU Connect GUI")
            .with_icon(icon)
            // We want the menu strictly on right-click; the left-click delivers
            // a Click event we handle below to show the main window. (Without
            // this, macOS would open the menu on every left click.)
            .with_menu_on_left_click(false)
            .build()
            .map_err(|err| TrayError::Build(err.to_string()))?;

        let weak_for_timer = weak.clone();

        let timer = slint::Timer::default();
        timer.start(slint::TimerMode::Repeated, POLL_INTERVAL, move || {
            while let Ok(event) = MenuEvent::receiver().try_recv() {
                if event.id == show_id {
                    dispatch_show(weak_for_timer.clone());
                } else if event.id == hide_id {
                    dispatch_hide(weak_for_timer.clone());
                } else if event.id == quit_id {
                    dispatch_quit(weak_for_timer.clone());
                }
            }
            while let Ok(event) = TrayIconEvent::receiver().try_recv() {
                match event {
                    TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    }
                    | TrayIconEvent::DoubleClick {
                        button: MouseButton::Left,
                        ..
                    } => {
                        dispatch_show(weak_for_timer.clone());
                    }
                    _ => {}
                }
            }
        });

        Ok(Self {
            _icon: tray_icon,
            _timer: timer,
        })
    }
}

fn build_icon() -> Result<Icon, TrayError> {
    let img = image::load_from_memory(ICON_BYTES)
        .map_err(|err| TrayError::Icon(err.to_string()))?
        .to_rgba8();
    let (w, h) = img.dimensions();
    Icon::from_rgba(img.into_raw(), w, h).map_err(|err| TrayError::Icon(err.to_string()))
}

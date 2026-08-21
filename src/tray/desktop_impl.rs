//! Tray-icon-based controller used on Windows and macOS.
//!
//! On Windows, `tray-icon` requires an active Win32 message pump on the
//! thread that creates the icon. This module spawns a dedicated thread that
//! owns the icon AND pumps messages, polling tray/menu events at 60 ms
//! cadence.

use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use tokio::sync::oneshot;

use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

use crate::tray::{open_web_ui, TrayError, ICON_BYTES};

const POLL_INTERVAL: Duration = Duration::from_millis(60);

pub struct TrayController {
    _thread: Option<thread::JoinHandle<()>>,
    _stop_tx: mpsc::SyncSender<()>,
}

impl TrayController {
    /// Spawn a background thread that creates the tray icon and runs the
    /// Win32 / Cocoa message pump.  Returns a oneshot receiver that fires
    /// when the user selects "退出".
    pub fn new(port: u16, token: &str) -> Result<(Self, oneshot::Receiver<()>), TrayError> {
        let (quit_tx, quit_rx) = oneshot::channel();
        let (stop_tx, stop_rx) = mpsc::sync_channel::<()>(1);

        // Decode the icon once, then move into the thread.
        let icon = build_icon()?;
        let token = token.to_string();

        let thread_handle = thread::spawn(move || {
            let menu = Menu::new();
            let open_item = MenuItem::new("打开网页", true, None);
            let quit_item = MenuItem::new("退出", true, None);
            menu.append_items(&[&open_item, &quit_item])
                .expect("append tray menu items");
            let open_id = open_item.id().clone();
            let quit_id = quit_item.id().clone();

            #[cfg(target_os = "windows")]
            pump_windows_messages(); // prime the message pump before icon creation

            let _tray_icon = TrayIconBuilder::new()
                .with_menu(Box::new(menu))
                .with_tooltip(format!("ZJU Connect - http://localhost:{port}"))
                .with_icon(icon)
                .with_menu_on_left_click(false)
                .build()
                .expect("create tray icon");

            let mut quit_tx = Some(quit_tx);

            loop {
                // Stop signal
                if stop_rx.try_recv().is_ok() {
                    break;
                }

                // ── Menu events ──────────────────────────────
                while let Ok(event) = MenuEvent::receiver().try_recv() {
                    if event.id == open_id {
                        open_web_ui(port, &token);
                    } else if event.id == quit_id {
                        if let Some(tx) = quit_tx.take() {
                            let _ = tx.send(());
                        }
                        return; // thread exits – tray icon dropped
                    }
                }

                // ── Tray icon click events ──────────────────
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
                            open_web_ui(port, &token);
                        }
                        _ => {}
                    }
                }

                // ── Windows message pump ────────────────────
                #[cfg(target_os = "windows")]
                pump_windows_messages();

                thread::sleep(POLL_INTERVAL);
            }
        });

        Ok((
            Self {
                _thread: Some(thread_handle),
                _stop_tx: stop_tx,
            },
            quit_rx,
        ))
    }
}

impl Drop for TrayController {
    fn drop(&mut self) {
        let _ = self._stop_tx.send(());
    }
}

/// Pump all pending Windows messages on the current thread.
#[cfg(target_os = "windows")]
fn pump_windows_messages() {
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE,
    };

    let mut msg = MSG::default();
    loop {
        let has_msg = unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE) };
        if has_msg.as_bool() {
            unsafe {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        } else {
            break;
        }
    }
}

fn build_icon() -> Result<Icon, TrayError> {
    let img = image::load_from_memory(ICON_BYTES)
        .map_err(|err| TrayError::Icon(err.to_string()))?
        .to_rgba8();
    let (w, h) = img.dimensions();
    Icon::from_rgba(img.into_raw(), w, h).map_err(|err| TrayError::Icon(err.to_string()))
}

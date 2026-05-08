//! UiBridge implementation that forwards ProxyEvents from manager threads onto
//! the Slint event loop, then mutates the AppWindow's reactive properties.

use slint::{ComponentHandle, Image, Model, SharedString, VecModel, Weak};
use zju_connect_gui::backend::proxy::{ProxyEvent, ProxyState, UiBridge};

use crate::AppWindow;

pub struct CapturingBridge {
    weak: Weak<AppWindow>,
    max_log_entries: usize,
}

impl CapturingBridge {
    pub fn new(weak: Weak<AppWindow>, max_log_entries: usize) -> Self {
        Self {
            weak,
            max_log_entries,
        }
    }
}

impl UiBridge for CapturingBridge {
    fn emit_event(&self, event: ProxyEvent) {
        let weak = self.weak.clone();
        let cap = self.max_log_entries;
        let _ = slint::invoke_from_event_loop(move || {
            let Some(w) = weak.upgrade() else { return };
            apply_event(&w, cap, event);
        });
    }

    fn show_window(&self) {
        let weak = self.weak.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(w) = weak.upgrade() {
                w.window().set_minimized(false);
                w.show().ok();
            }
        });
    }
}

fn apply_event(window: &AppWindow, cap: usize, event: ProxyEvent) {
    match event {
        ProxyEvent::Log(line) => {
            // The log model is owned by app.rs as a VecModel<SharedString> set on
            // the window. Downcast back to it so we can mutate in place.
            let model = window.get_logs();
            if let Some(vec) = model.as_any().downcast_ref::<VecModel<SharedString>>() {
                vec.push(SharedString::from(line));
                while vec.row_count() > cap {
                    vec.remove(0);
                }
            }
        }
        ProxyEvent::State {
            state,
            message,
            running,
            awaiting,
            retry_attempt: _,
            retry_delay_ms: _,
        } => {
            window.set_running(running);
            if let Some(msg) = message {
                window.set_status_message(SharedString::from(msg));
            } else if let Some(reason) = awaiting {
                window.set_status_message(SharedString::from(format!("等待输入: {reason}")));
            } else if state == ProxyState::Stopped && !running {
                window.set_status_message(SharedString::from("已断开"));
            } else if state == ProxyState::Connected {
                window.set_status_message(SharedString::from("已连接"));
            }
        }
        ProxyEvent::NeedInput { kind, prompt } => {
            window.set_modal_type(0);
            let kind_int = match kind {
                zju_connect_gui::backend::proxy::InputKind::Sms => 1,
                _ => 0,
            };
            window.set_modal_input_kind(kind_int);
            window.set_modal_title(SharedString::from(if kind_int == 1 {
                "短信验证码"
            } else {
                "输入需求"
            }));
            window.set_modal_prompt(SharedString::from(prompt));
            window.set_modal_input(SharedString::from(""));
            window.set_modal_open(true);
        }
        ProxyEvent::NeedCaptcha {
            base64,
            updated_at_ms: _,
        } => match decode_captcha_image(&base64) {
            Ok((image, w, h)) => {
                window.set_modal_type(1);
                window.set_modal_title(SharedString::from("图形验证码"));
                window.set_modal_prompt(SharedString::from(
                    "请在图片上按顺序点击对应位置，然后提交",
                ));
                window.set_captcha_image(image);
                window.set_captcha_natural_width(w);
                window.set_captcha_natural_height(h);
                window.set_modal_open(true);
            }
            Err(err) => {
                window.set_status_message(SharedString::from(format!(
                    "验证码解码失败：{err}"
                )));
            }
        },
        ProxyEvent::Error(msg) => {
            window.set_status_message(SharedString::from(msg));
        }
    }
}

fn decode_captcha_image(base64_str: &str) -> Result<(Image, i32, i32), String> {
    use base64::Engine;
    let raw = base64::engine::general_purpose::STANDARD
        .decode(base64_str.trim())
        .map_err(|e| format!("base64: {e}"))?;
    let img = image::load_from_memory(&raw)
        .map_err(|e| format!("image decode: {e}"))?
        .to_rgba8();
    let (w, h) = img.dimensions();
    let buffer =
        slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(img.as_raw(), w, h);
    Ok((Image::from_rgba8(buffer), w as i32, h as i32))
}

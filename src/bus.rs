//! Thread-safe event bus: worker threads → UI thread.
//!
//! Messages go into an mpsc channel and a `WM_APP_BUS` is posted to wake the
//! UI thread, which drains the channel.  No polling anywhere.

use std::any::Any;
use std::sync::mpsc::{channel, Receiver, Sender};

use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

use crate::modules::ModuleId;
use crate::util::SendHwnd;
use crate::window::WM_APP_BUS;

pub enum Msg {
    /// Payload for a specific module.
    Module(ModuleId, Box<dyn Any + Send>),
    ConfigFileChanged,
}

#[derive(Clone)]
pub struct Bus {
    tx: Sender<Msg>,
    hwnd: SendHwnd,
}

impl Bus {
    pub fn new(hwnd: SendHwnd) -> (Self, Receiver<Msg>) {
        let (tx, rx) = channel();
        (Self { tx, hwnd }, rx)
    }

    pub fn send(&self, m: Msg) {
        if self.tx.send(m).is_ok() {
            unsafe {
                let _ = PostMessageW(Some(self.hwnd.hwnd()), WM_APP_BUS, WPARAM(0), LPARAM(0));
            }
        }
    }

    pub fn to_module<T: Any + Send>(&self, id: ModuleId, payload: T) {
        self.send(Msg::Module(id, Box::new(payload)));
    }
}

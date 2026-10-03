//! Caps Lock / Num Lock / Scroll Lock peeks.
//!
//! The shell forwards lock-key presses from raw input (no hooks, no polling);
//! the toggle state is read shortly after, so it is the real keyboard state
//! even if the key was pressed in another app.

use std::time::{Duration, Instant};

use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CAPITAL, VK_NUMLOCK, VK_SCROLL};

use super::{Cx, Indicator, Module, ModuleId, Peek, SystemEvent, Trailing};
use crate::gfx::Icon;
use crate::util::palette;

const ID: ModuleId = "locks";
const KEYS: [(u16, &str); 3] = [(VK_CAPITAL.0, "Caps Lock"), (VK_NUMLOCK.0, "Num Lock"), (VK_SCROLL.0, "Scroll Lock")];

fn read() -> [bool; 3] {
    KEYS.map(|(vk, _)| unsafe { GetKeyState(vk as i32) } & 1 != 0)
}

pub struct Locks {
    state: [bool; 3],
    check_at: Option<Instant>,
}

impl Locks {
    pub fn new() -> Self {
        Self { state: [false; 3], check_at: None }
    }
}

impl Module for Locks {
    fn id(&self) -> ModuleId {
        ID
    }
    fn title(&self) -> &str {
        "Lock Keys"
    }
    fn icon(&self) -> Icon {
        Icon::Keyboard
    }

    fn start(&mut self, _cx: &mut Cx) {
        self.state = read();
    }

    fn on_system(&mut self, ev: &SystemEvent, cx: &mut Cx) {
        if let SystemEvent::RawKey { vk, down: true } = ev {
            if KEYS.iter().any(|k| k.0 == *vk) {
                // the toggle lands right after the key-down; read it a beat later
                self.check_at = Some(cx.now + Duration::from_millis(40));
            }
        }
    }

    fn next_tick(&self) -> Option<Instant> {
        self.check_at
    }

    fn on_tick(&mut self, cx: &mut Cx) {
        self.check_at = None;
        let new = read();
        let c = &cx.cfg.locks;
        let enabled = [c.caps, c.num, c.scroll];
        for i in 0..KEYS.len() {
            if new[i] != self.state[i] && enabled[i] {
                let (label, color) = if new[i] { ("On", palette::GREEN) } else { ("Off", palette::TEXT_DIM) };
                cx.fx.peek(
                    Peek::new(Icon::Keyboard, color, KEYS[i].1, "")
                        .trailing(Trailing::Text(label.into(), color))
                        .key("locks")
                        .duration_ms(1300),
                );
            }
        }
        self.state = new;
    }

    fn indicators(&self, out: &mut Vec<Indicator>) {
        if self.state[0] {
            out.push(Indicator {
                color: palette::TEXT_DIM,
                icon: Some(Icon::Keyboard),
                label: Some("Caps Lock".into()),
                battery: None,
                dot: false,
            });
        }
    }
}

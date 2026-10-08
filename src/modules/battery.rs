//! Battery + charging state. Purely event driven: Windows sends
//! WM_POWERBROADCAST when the power source or percentage changes.

use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};

use super::{Cx, Indicator, Module, ModuleId, Peek, SystemEvent, Trailing};
use crate::gfx::Icon;
use crate::util::{palette, Color};

const ID: ModuleId = "battery";

#[derive(Clone, Copy, PartialEq, Debug, Default)]
struct State {
    has_battery: bool,
    plugged: bool,
    percent: u8,
    saver: bool,
}

fn read() -> State {
    let mut s = SYSTEM_POWER_STATUS::default();
    if unsafe { GetSystemPowerStatus(&mut s) }.is_err() {
        return State::default();
    }
    State {
        has_battery: s.BatteryFlag != 128 && s.BatteryFlag != 255 && s.BatteryLifePercent <= 100,
        plugged: s.ACLineStatus == 1,
        percent: s.BatteryLifePercent.min(100),
        saver: s.SystemStatusFlag == 1,
    }
}

pub struct Battery {
    state: State,
    warned_low: bool,
    warned_full: bool,
}

impl Battery {
    pub fn new() -> Self {
        Self { state: State::default(), warned_low: false, warned_full: false }
    }

    fn level(&self) -> f32 {
        self.state.percent as f32 / 100.0
    }
}

impl Module for Battery {
    fn id(&self) -> ModuleId {
        ID
    }
    fn title(&self) -> &str {
        "Battery"
    }
    fn icon(&self) -> Icon {
        Icon::Bolt
    }

    fn start(&mut self, _cx: &mut Cx) {
        self.state = read();
    }

    fn on_system(&mut self, ev: &SystemEvent, cx: &mut Cx) {
        if !matches!(ev, SystemEvent::PowerChanged) {
            return;
        }
        let new = read();
        let old = self.state;
        self.state = new;
        if !new.has_battery || new == old {
            return;
        }
        cx.fx.redraw = true;
        let level = self.level();
        let pct = format!("{}%", new.percent);
        if new.plugged && !old.plugged && cx.cfg.battery.peek_on_plug {
            cx.fx.peek(
                Peek::new(
                    Icon::Bolt,
                    palette::GREEN,
                    "Charging",
                    if new.percent >= 100 { "Fully charged".to_string() } else { pct.clone() },
                )
                .trailing(Trailing::Battery { level, charging: true })
                .key("battery")
                .duration_ms(2600),
            );
            self.warned_low = false;
        } else if !new.plugged && old.plugged && cx.cfg.battery.peek_on_plug {
            cx.fx.peek(
                Peek::new(Icon::Bolt, Color::white(0.9), "On battery", pct.clone())
                    .trailing(Trailing::Battery { level, charging: false })
                    .key("battery")
                    .duration_ms(2000),
            );
        }
        let low = cx.cfg.battery.low_threshold;
        if !new.plugged && new.percent <= low && !self.warned_low {
            self.warned_low = true;
            cx.fx.peek(
                Peek::new(Icon::Bolt, palette::RED, "Low battery", format!("{pct} remaining"))
                    .trailing(Trailing::Battery { level, charging: false })
                    .key("battery")
                    .duration_ms(5000),
            );
        }
        if new.plugged && new.percent >= 100 && old.percent < 100 && !self.warned_full {
            self.warned_full = true;
            cx.fx.peek(
                Peek::new(Icon::Bolt, palette::GREEN, "Fully charged", "You can unplug now")
                    .trailing(Trailing::Battery { level: 1.0, charging: true })
                    .key("battery"),
            );
        }
        if new.percent < 95 {
            self.warned_full = false;
        }
    }

    fn indicators(&self, out: &mut Vec<Indicator>) {
        if self.state.has_battery {
            out.push(Indicator {
                color: if self.state.saver { palette::YELLOW } else { Color::white(0.9) },
                icon: None,
                label: None,
                battery: Some((self.level(), self.state.plugged)),
                dot: false,
            });
        }
    }
}

//! Volume & brightness peeks: the notch's take on the Windows flyout.
//!
//! Volume: the default output's IAudioEndpointVolume calls us back on every
//! change (keys, mixer, headset buttons), so nothing is polled. With
//! `replace_system_flyout` the volume keys are registered as global hotkeys:
//! Windows then hands them to us instead of changing the volume and showing
//! its flyout, and we set the volume ourselves. No keyboard hook, so other
//! keystrokes cost nothing extra. While a peek couldn't be seen (notch
//! hidden, fullscreen, or already open) the keys are released to Windows.
//!
//! Brightness: WMI raises WmiMonitorBrightnessEvent for the built-in panel;
//! a worker blocks on that event query. External monitors (DDC/CI) don't
//! report changes. Brightness keys are handled by the firmware, so Windows
//! may still show its own brightness flyout next to ours.

use std::any::Any;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Mutex, Once};

use windows::core::{implement, w, BSTR, PCWSTR};
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::Media::Audio::Endpoints::{
    IAudioEndpointVolume, IAudioEndpointVolumeCallback, IAudioEndpointVolumeCallback_Impl,
};
use windows::Win32::Media::Audio::{
    eConsole, eRender, EDataFlow, ERole, IMMDevice, IMMDeviceEnumerator, IMMNotificationClient,
    IMMNotificationClient_Impl, MMDeviceEnumerator, AUDIO_VOLUME_NOTIFICATION_DATA, DEVICE_STATE,
};
use windows::Win32::System::Com::StructuredStorage::PropVariantClear;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoSetProxyBlanket, CLSCTX_ALL, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
    EOAC_NONE, RPC_C_AUTHN_LEVEL_CALL, RPC_C_IMP_LEVEL_IMPERSONATE, STGM_READ,
};
use windows::Win32::System::Variant::{VariantClear, VARIANT, VT_I4, VT_LPWSTR, VT_UI1, VT_UI4};
use windows::Win32::System::Wmi::{
    IWbemClassObject, IWbemLocator, WbemLocator, WBEM_FLAG_FORWARD_ONLY, WBEM_FLAG_RETURN_IMMEDIATELY, WBEM_INFINITE,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS, MOD_NOREPEAT, VIRTUAL_KEY, VK_VOLUME_DOWN, VK_VOLUME_MUTE,
    VK_VOLUME_UP,
};

use super::{Cx, Module, ModuleId, Peek, SystemEvent, Trailing};
use crate::bus::Bus;
use crate::gfx::Icon;
use crate::util::{palette, Color, SendHwnd};

const ID: ModuleId = "osd";
/// PKEY_Device_FriendlyName ("Speakers (Realtek(R) Audio)").
const PKEY_FRIENDLY_NAME: PROPERTYKEY =
    PROPERTYKEY { fmtid: windows::core::GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0), pid: 14 };
/// Event context of the volume changes we make ourselves: their change
/// notification is skipped (the key handler reports them already).
const OWN_CHANGE: windows::core::GUID = windows::core::GUID::from_u128(0x6d1c2a4e_93b7_4f0a_b5e1_0d4e7a9c3f21);
/// Hotkey ids for the volume keys (the shell's own hotkeys start at 0x4E00).
const KEYS: [(i32, VIRTUAL_KEY); 3] = [(0x4F00, VK_VOLUME_UP), (0x4F01, VK_VOLUME_DOWN), (0x4F02, VK_VOLUME_MUTE)];

enum OsdMsg {
    /// The default output changed (or was bound for the first time).
    Device {
        name: String,
        level: f32,
        muted: bool,
    },
    /// `forced`: a key press we handled; peek even if nothing changed (at 0 / 100 %).
    Volume {
        level: f32,
        muted: bool,
        forced: bool,
    },
    Brightness(u8),
}

enum Cmd {
    Step(i32),
    ToggleMute,
    Rebind,
    Quit,
}

pub struct Osd {
    tx: Option<Sender<Cmd>>,
    /// Notch window the volume hotkeys are registered on, while they are.
    keys: Option<SendHwnd>,
    device: String,
    level: f32,
    muted: bool,
    brightness: Option<u8>,
}

impl Osd {
    pub fn new() -> Self {
        Self { tx: None, keys: None, device: String::new(), level: 0.0, muted: false, brightness: None }
    }

    /// Take the volume keys while a peek can be seen, give them back otherwise.
    fn sync_keys(&mut self, cx: &Cx, peeks_shown: bool) {
        let c = &cx.cfg.osd;
        STEP.store(c.volume_step, Ordering::Relaxed);
        let want = self.tx.is_some() && c.volume && c.replace_system_flyout && peeks_shown;
        match (want, self.keys) {
            (true, None) => {
                let hwnd = cx.hwnd;
                for (id, vk) in KEYS {
                    // volume up/down repeat while held, mute must not flip back and forth
                    let mods = if vk == VK_VOLUME_MUTE { MOD_NOREPEAT } else { HOT_KEY_MODIFIERS(0) };
                    if let Err(e) = unsafe { RegisterHotKey(Some(hwnd.hwnd()), id, mods, vk.0 as u32) } {
                        // another app owns this key; Windows keeps handling it
                        crate::log!("osd: volume key {:#x} unavailable: {e}", vk.0);
                    }
                }
                self.keys = Some(hwnd);
            }
            (false, Some(hwnd)) => {
                self.release_keys(hwnd);
            }
            _ => {}
        }
    }

    fn release_keys(&mut self, hwnd: SendHwnd) {
        for (id, _) in KEYS {
            let _ = unsafe { UnregisterHotKey(Some(hwnd.hwnd()), id) };
        }
        self.keys = None;
    }

    fn volume_peek(&self) -> Peek {
        let silent = self.muted || self.level < 0.005;
        let (icon, color) = if silent { (Icon::Mute, palette::TEXT_DIM) } else { (Icon::Volume, Color::white(0.92)) };
        Peek::new(icon, color, "Volume", short_device(&self.device))
            .trailing(Trailing::Level { frac: self.level, color, text: self.muted.then(|| "Muted".to_string()) })
            .key("osd-volume")
            .duration_ms(1600)
            .instant()
    }
}

impl Module for Osd {
    fn id(&self) -> ModuleId {
        ID
    }
    fn title(&self) -> &str {
        "Volume & Brightness"
    }
    fn icon(&self) -> Icon {
        Icon::Volume
    }

    fn start(&mut self, cx: &mut Cx) {
        let (tx, rx) = channel();
        let bus = cx.bus.clone();
        let tx2 = tx.clone();
        let _ = std::thread::Builder::new().name("osd-volume".into()).spawn(move || volume_worker(rx, tx2, bus));
        self.tx = Some(tx);
        // WMI has no way to cancel a blocking event wait; one watcher serves
        // the whole process and its messages are dropped while we're stopped.
        static BRIGHTNESS: Once = Once::new();
        let bus = cx.bus.clone();
        BRIGHTNESS.call_once(|| {
            let _ = std::thread::Builder::new().name("osd-brightness".into()).spawn(move || brightness_worker(bus));
        });
        self.sync_keys(cx, crate::app::peeks_shown());
    }

    fn stop(&mut self) {
        if let Some(hwnd) = self.keys {
            self.release_keys(hwnd);
        }
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(Cmd::Quit);
        }
    }

    fn on_system(&mut self, ev: &SystemEvent, cx: &mut Cx) {
        match ev {
            SystemEvent::ConfigReloaded => self.sync_keys(cx, crate::app::peeks_shown()),
            SystemEvent::PeeksShown(shown) => self.sync_keys(cx, *shown),
            SystemEvent::Hotkey(id) if self.keys.is_some() => {
                let cmd = match KEYS.iter().find(|k| k.0 == *id).map(|k| k.1) {
                    Some(VK_VOLUME_UP) => Cmd::Step(1),
                    Some(VK_VOLUME_DOWN) => Cmd::Step(-1),
                    Some(VK_VOLUME_MUTE) => Cmd::ToggleMute,
                    _ => return,
                };
                if let Some(tx) = &self.tx {
                    let _ = tx.send(cmd);
                }
            }
            _ => {}
        }
    }

    fn on_message(&mut self, msg: Box<dyn Any + Send>, cx: &mut Cx) {
        let Ok(m) = msg.downcast::<OsdMsg>() else { return };
        match *m {
            OsdMsg::Device { name, level, muted } => {
                self.device = name;
                self.level = level;
                self.muted = muted;
            }
            OsdMsg::Volume { level, muted, forced } => {
                let changed = (level - self.level).abs() > 0.004 || muted != self.muted;
                self.level = level;
                self.muted = muted;
                if (changed || forced) && cx.cfg.osd.volume {
                    cx.fx.peek(self.volume_peek());
                }
            }
            OsdMsg::Brightness(b) => {
                let changed = self.brightness != Some(b);
                self.brightness = Some(b);
                if changed && cx.cfg.osd.brightness {
                    let frac = b.min(100) as f32 / 100.0;
                    cx.fx.peek(
                        Peek::new(Icon::Brightness, Color::white(0.92), "Brightness", "")
                            .trailing(Trailing::Level { frac, color: Color::white(0.92), text: None })
                            .key("osd-brightness")
                            .duration_ms(1600)
                            .instant(),
                    );
                }
            }
        }
    }
}

/// "Speakers (Realtek(R) Audio)" → "Speakers (Realtek Audio)".
fn short_device(name: &str) -> String {
    name.replace("(R)", "").replace("(TM)", "").replace("  ", " ")
}

/// Next volume for a key press: snaps to the step grid like Windows does
/// (45 % + 2 → 46 %, not 47 %).
fn stepped(level: f32, dir: i32, step: u32) -> f32 {
    let pct = (level * 100.0).round() as i32;
    let s = step.max(1) as i32;
    let next = if dir > 0 { (pct / s + 1) * s } else { ((pct + s - 1) / s - 1) * s };
    next.clamp(0, 100) as f32 / 100.0
}

// ---------------------------------------------------------------------------
// Volume worker
// ---------------------------------------------------------------------------

static STEP: AtomicU32 = AtomicU32::new(2);

#[implement(IAudioEndpointVolumeCallback)]
struct VolumeEvents {
    bus: Bus,
}

impl IAudioEndpointVolumeCallback_Impl for VolumeEvents_Impl {
    fn OnNotify(&self, data: *mut AUDIO_VOLUME_NOTIFICATION_DATA) -> windows::core::Result<()> {
        if let Some(d) = unsafe { data.as_ref() }.filter(|d| d.guidEventContext != OWN_CHANGE) {
            self.bus.to_module(ID, OsdMsg::Volume { level: d.fMasterVolume, muted: d.bMuted.as_bool(), forced: false });
        }
        Ok(())
    }
}

#[implement(IMMNotificationClient)]
struct DeviceEvents {
    tx: Mutex<Sender<Cmd>>,
}

impl IMMNotificationClient_Impl for DeviceEvents_Impl {
    fn OnDeviceStateChanged(&self, _id: &PCWSTR, _state: DEVICE_STATE) -> windows::core::Result<()> {
        Ok(())
    }
    fn OnDeviceAdded(&self, _id: &PCWSTR) -> windows::core::Result<()> {
        Ok(())
    }
    fn OnDeviceRemoved(&self, _id: &PCWSTR) -> windows::core::Result<()> {
        Ok(())
    }
    fn OnDefaultDeviceChanged(&self, flow: EDataFlow, role: ERole, _id: &PCWSTR) -> windows::core::Result<()> {
        if flow == eRender && role == eConsole {
            if let Ok(tx) = self.tx.lock() {
                let _ = tx.send(Cmd::Rebind);
            }
        }
        Ok(())
    }
    fn OnPropertyValueChanged(&self, _id: &PCWSTR, _key: &PROPERTYKEY) -> windows::core::Result<()> {
        Ok(())
    }
}

fn volume_worker(rx: Receiver<Cmd>, tx: Sender<Cmd>, bus: Bus) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let enumerator: IMMDeviceEnumerator = match CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) {
            Ok(e) => e,
            Err(e) => {
                crate::log!("osd: audio devices unavailable: {e}");
                return;
            }
        };
        let devices: IMMNotificationClient = DeviceEvents { tx: Mutex::new(tx) }.into();
        let _ = enumerator.RegisterEndpointNotificationCallback(&devices);
        let events: IAudioEndpointVolumeCallback = VolumeEvents { bus: bus.clone() }.into();
        let mut ep = bind(&enumerator, &events, &bus);

        while let Ok(cmd) = rx.recv() {
            match cmd {
                Cmd::Quit => break,
                Cmd::Rebind => {
                    if let Some(v) = ep.take() {
                        let _ = v.UnregisterControlChangeNotify(&events);
                    }
                    ep = bind(&enumerator, &events, &bus);
                }
                Cmd::Step(dir) => {
                    if let Some(v) = &ep {
                        let level = v.GetMasterVolumeLevelScalar().unwrap_or(0.0);
                        // either volume key unmutes, as in Windows
                        let _ = v.SetMute(false, &OWN_CHANGE);
                        let _ = v
                            .SetMasterVolumeLevelScalar(stepped(level, dir, STEP.load(Ordering::Relaxed)), &OWN_CHANGE);
                        report(v, &bus);
                    }
                }
                Cmd::ToggleMute => {
                    if let Some(v) = &ep {
                        let muted = v.GetMute().is_ok_and(|m| m.as_bool());
                        let _ = v.SetMute(!muted, &OWN_CHANGE);
                        report(v, &bus);
                    }
                }
            }
        }
        if let Some(v) = ep {
            let _ = v.UnregisterControlChangeNotify(&events);
        }
        let _ = enumerator.UnregisterEndpointNotificationCallback(&devices);
    }
}

unsafe fn report(v: &IAudioEndpointVolume, bus: &Bus) {
    let level = v.GetMasterVolumeLevelScalar().unwrap_or(0.0);
    let muted = v.GetMute().is_ok_and(|m| m.as_bool());
    bus.to_module(ID, OsdMsg::Volume { level, muted, forced: true });
}

/// Subscribe to the default output's volume and report its current state.
unsafe fn bind(
    en: &IMMDeviceEnumerator,
    events: &IAudioEndpointVolumeCallback,
    bus: &Bus,
) -> Option<IAudioEndpointVolume> {
    let dev = en.GetDefaultAudioEndpoint(eRender, eConsole).ok()?;
    let vol: IAudioEndpointVolume = dev.Activate(CLSCTX_ALL, None).ok()?;
    vol.RegisterControlChangeNotify(events).ok()?;
    bus.to_module(
        ID,
        OsdMsg::Device {
            name: device_name(&dev),
            level: vol.GetMasterVolumeLevelScalar().unwrap_or(0.0),
            muted: vol.GetMute().is_ok_and(|m| m.as_bool()),
        },
    );
    Some(vol)
}

unsafe fn device_name(dev: &IMMDevice) -> String {
    let Ok(store) = dev.OpenPropertyStore(STGM_READ) else { return String::new() };
    let Ok(mut pv) = store.GetValue(&PKEY_FRIENDLY_NAME) else { return String::new() };
    let inner = &pv.Anonymous.Anonymous;
    let name =
        if inner.vt == VT_LPWSTR { inner.Anonymous.pwszVal.to_string().unwrap_or_default() } else { String::new() };
    let _ = PropVariantClear(&mut pv);
    name
}

// ---------------------------------------------------------------------------
// Brightness worker
// ---------------------------------------------------------------------------

fn brightness_worker(bus: Bus) {
    let run = || -> windows::core::Result<()> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let locator: IWbemLocator = CoCreateInstance(&WbemLocator, None, CLSCTX_INPROC_SERVER)?;
            let empty = BSTR::new();
            let svc = locator.ConnectServer(&BSTR::from("ROOT\\WMI"), &empty, &empty, &empty, 0, &empty, None)?;
            CoSetProxyBlanket(
                &svc,
                10, // RPC_C_AUTHN_WINNT
                0,  // RPC_C_AUTHZ_NONE
                PCWSTR::null(),
                RPC_C_AUTHN_LEVEL_CALL,
                RPC_C_IMP_LEVEL_IMPERSONATE,
                None,
                EOAC_NONE,
            )?;
            let events = svc.ExecNotificationQuery(
                &BSTR::from("WQL"),
                &BSTR::from("SELECT * FROM WmiMonitorBrightnessEvent"),
                WBEM_FLAG_RETURN_IMMEDIATELY | WBEM_FLAG_FORWARD_ONLY,
                None,
            )?;
            loop {
                let mut objs: [Option<IWbemClassObject>; 1] = [None];
                let mut n = 0u32;
                events.Next(WBEM_INFINITE, &mut objs, &mut n).ok()?;
                if let Some(b) = objs[0].take().and_then(|o| read_int(&o, w!("Brightness"))) {
                    bus.to_module(ID, OsdMsg::Brightness(b.min(100) as u8));
                }
            }
        }
    };
    if let Err(e) = run() {
        crate::log!("osd: brightness events unavailable: {e}");
    }
}

unsafe fn read_int(obj: &IWbemClassObject, name: PCWSTR) -> Option<u32> {
    let mut v = VARIANT::default();
    obj.Get(name, 0, &mut v, None, None).ok()?;
    let inner = &v.Anonymous.Anonymous;
    let out = match inner.vt {
        VT_UI1 => Some(inner.Anonymous.bVal as u32),
        VT_I4 => Some(inner.Anonymous.lVal.max(0) as u32),
        VT_UI4 => Some(inner.Anonymous.ulVal),
        _ => None,
    };
    let _ = VariantClear(&mut v);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_snap_to_grid() {
        assert_eq!(stepped(0.45, 1, 2), 0.46);
        assert_eq!(stepped(0.45, -1, 2), 0.44);
        assert_eq!(stepped(0.46, -1, 2), 0.44);
        assert_eq!(stepped(0.99, 1, 2), 1.0);
        assert_eq!(stepped(1.0, 1, 2), 1.0);
        assert_eq!(stepped(0.0, -1, 2), 0.0);
        assert_eq!(stepped(0.5, 1, 5), 0.55);
    }

    #[test]
    fn device_names() {
        assert_eq!(short_device("Speakers (Realtek(R) Audio)"), "Speakers (Realtek Audio)");
    }
}

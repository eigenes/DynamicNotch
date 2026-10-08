//! Bluetooth connect / disconnect peeks with battery level ("AirPods Pro ·
//! Connected · 80%").
//!
//! A DeviceWatcher over paired Bluetooth and Bluetooth LE association
//! endpoints reports `IsConnected` changes (event driven, no polling). The
//! battery comes from the device's PnP property Windows fills in for hands-free
//! headsets and HID devices (the value Settings shows), with the GATT Battery
//! Service as a fallback for LE devices. Headsets often report it a few
//! seconds after connecting, so it is retried briefly, and refreshed every
//! few minutes while something is connected for the low-battery warning.

use std::any::Any;
use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use windows::core::{Interface, GUID, HSTRING};
use windows::Devices::Bluetooth::GenericAttributeProfile::{
    GattCharacteristicUuids, GattCommunicationStatus, GattServiceUuids,
};
use windows::Devices::Bluetooth::{BluetoothCacheMode, BluetoothDevice, BluetoothLEDevice};
use windows::Devices::Enumeration::{DeviceInformation, DeviceInformationKind, DeviceInformationUpdate, DeviceWatcher};
use windows::Foundation::{IReference, TypedEventHandler};
use windows::Storage::Streams::DataReader;
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};
use windows_collections::{IIterable, IMapView};
use windows_core::IInspectable;

use super::{Cx, Module, ModuleId, Peek, Trailing};
use crate::bus::Bus;
use crate::gfx::Icon;
use crate::util::palette;

const ID: ModuleId = "bluetooth";
const IS_CONNECTED: &str = "System.Devices.Aep.IsConnected";
const CONTAINER: &str = "System.Devices.Aep.ContainerId";
/// DEVPKEY_Bluetooth_Battery: percentage on the device's PnP nodes.
const BATTERY: &str = "{104EA319-6EE2-4701-BD47-8DDBF425BBE5} 2";
/// Battery arrives late on many headsets: retry after these delays (ms).
const BATTERY_RETRIES: [u64; 4] = [0, 1500, 3500, 7000];
const BATTERY_REFRESH: Duration = Duration::from_secs(5 * 60);
/// A battery reading this soon after connecting updates the connect peek.
const PEEK_WINDOW: Duration = Duration::from_millis(3500);

#[derive(Clone, Copy, Debug, PartialEq)]
enum Kind {
    Audio,
    Mouse,
    Keyboard,
    Gamepad,
    Phone,
    Other,
}

impl Kind {
    fn icon(self) -> Icon {
        match self {
            Kind::Audio => Icon::Headphones,
            Kind::Mouse => Icon::Mouse,
            Kind::Keyboard => Icon::Keyboard,
            Kind::Gamepad => Icon::Gamepad,
            Kind::Phone => Icon::Phone,
            Kind::Other => Icon::Bluetooth,
        }
    }
}

enum BtMsg {
    Connected {
        id: String,
        name: String,
        kind: Kind,
    },
    /// Already connected when we started: tracked, but no peek.
    Present {
        id: String,
        name: String,
        kind: Kind,
    },
    Disconnected {
        id: String,
    },
    Battery {
        id: String,
        level: u8,
    },
}

struct Device {
    name: String,
    kind: Kind,
    connected_at: Instant,
    /// A battery reading before this instant updates the connect peek.
    peek_until: Option<Instant>,
    battery: Option<u8>,
    warned: bool,
}

pub struct Bluetooth {
    tx: Option<Sender<Ev>>,
    devices: HashMap<String, Device>,
}

impl Bluetooth {
    pub fn new() -> Self {
        Self { tx: None, devices: HashMap::new() }
    }
}

fn connect_peek(d: &Device) -> Peek {
    let sub = match d.battery {
        Some(b) => format!("Connected · {b}%"),
        None => "Connected".to_string(),
    };
    let p = Peek::new(d.kind.icon(), palette::BLUE, &d.name, sub).key("bluetooth").duration_ms(3500);
    match d.battery {
        Some(b) => p.trailing(Trailing::Battery { level: b as f32 / 100.0, charging: false }),
        None => p,
    }
}

impl Module for Bluetooth {
    fn id(&self) -> ModuleId {
        ID
    }
    fn title(&self) -> &str {
        "Bluetooth"
    }
    fn icon(&self) -> Icon {
        Icon::Bluetooth
    }

    fn start(&mut self, cx: &mut Cx) {
        let (tx, rx) = channel();
        let bus = cx.bus.clone();
        let tx2 = tx.clone();
        let _ = std::thread::Builder::new().name("bluetooth".into()).spawn(move || worker(rx, tx2, bus));
        self.tx = Some(tx);
    }

    fn stop(&mut self) {
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(Ev::Quit);
        }
        self.devices.clear();
    }

    fn on_message(&mut self, msg: Box<dyn Any + Send>, cx: &mut Cx) {
        let Ok(m) = msg.downcast::<BtMsg>() else { return };
        let cfg = &cx.cfg.bluetooth;
        let now = Instant::now();
        match *m {
            BtMsg::Connected { id, name, kind } => {
                // dual-mode devices connect as Bluetooth and Bluetooth LE at once
                let twin =
                    self.devices.values().any(|d| d.name == name && now - d.connected_at < Duration::from_secs(10));
                let show = cfg.peek_on_connect && !twin;
                let d = Device {
                    name,
                    kind,
                    connected_at: now,
                    peek_until: show.then(|| now + PEEK_WINDOW),
                    battery: None,
                    warned: false,
                };
                if show {
                    cx.fx.peek(connect_peek(&d));
                }
                self.devices.insert(id, d);
            }
            BtMsg::Present { id, name, kind } => {
                let d = Device { name, kind, connected_at: now, peek_until: None, battery: None, warned: false };
                self.devices.insert(id, d);
            }
            BtMsg::Disconnected { id } => {
                if let Some(d) = self.devices.remove(&id) {
                    let twin = self.devices.values().any(|o| o.name == d.name);
                    if cfg.peek_on_disconnect && !twin {
                        cx.fx.peek(
                            Peek::new(d.kind.icon(), palette::TEXT_DIM, &d.name, "Disconnected")
                                .key("bluetooth")
                                .duration_ms(2200),
                        );
                    }
                }
            }
            BtMsg::Battery { id, level } => {
                let Some(d) = self.devices.get_mut(&id) else { return };
                let first = d.battery.is_none();
                d.battery = Some(level);
                let low = cfg.low_battery > 0 && level <= cfg.low_battery;
                if first && d.peek_until.is_some_and(|t| now < t) {
                    cx.fx.peek(connect_peek(d));
                    // the connect peek already shows the (red) level
                    d.warned |= low;
                } else if low && !d.warned {
                    d.warned = true;
                    cx.fx.peek(
                        Peek::new(
                            d.kind.icon(),
                            palette::RED,
                            format!("{} battery low", d.name),
                            format!("{level}% left"),
                        )
                        .trailing(Trailing::Battery { level: level as f32 / 100.0, charging: false })
                        .key("bluetooth-low")
                        .duration_ms(5000),
                    );
                }
                if level > cfg.low_battery.saturating_add(5) {
                    d.warned = false;
                }
            }
        }
        cx.fx.redraw = true;
    }
}

// ---------------------------------------------------------------------------
// Worker
// ---------------------------------------------------------------------------

enum Ev {
    Added { id: String, name: String, connected: bool, container: Option<GUID> },
    Updated { id: String, connected: Option<bool> },
    Removed(String),
    Ready,
    Quit,
}

struct Endpoint {
    name: String,
    connected: bool,
    container: Option<GUID>,
}

fn worker(rx: Receiver<Ev>, tx: Sender<Ev>, bus: Bus) {
    unsafe {
        let _ = RoInitialize(RO_INIT_MULTITHREADED);
    }
    let watcher = match watch(tx) {
        Ok(w) => w,
        Err(e) => {
            crate::log!("bluetooth: device watcher unavailable: {e}");
            return;
        }
    };
    let mut eps: HashMap<String, Endpoint> = HashMap::new();
    // events before EnumerationCompleted are the initial state, not changes
    let mut ready = false;
    loop {
        let any_connected = eps.values().any(|e| e.connected);
        let ev = if any_connected {
            match rx.recv_timeout(BATTERY_REFRESH) {
                Ok(ev) => Some(ev),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => break,
            }
        } else {
            match rx.recv() {
                Ok(ev) => Some(ev),
                Err(_) => break,
            }
        };
        match ev {
            None => {
                for (id, e) in eps.iter().filter(|(_, e)| e.connected) {
                    read_battery(id.clone(), e.container, &[0], bus.clone());
                }
            }
            Some(Ev::Quit) => break,
            Some(Ev::Ready) => {
                ready = true;
                // track what is already connected (battery for the low warning)
                for (id, e) in eps.iter().filter(|(_, e)| e.connected) {
                    let kind = device_kind(id, &e.name);
                    bus.to_module(ID, BtMsg::Present { id: id.clone(), name: e.name.clone(), kind });
                    read_battery(id.clone(), e.container, &[0], bus.clone());
                }
            }
            Some(Ev::Added { id, name, connected, container }) => {
                let ep = Endpoint { name, connected: false, container };
                eps.insert(id.clone(), ep);
                if connected {
                    set_connected(&mut eps, &id, true, ready, &bus);
                }
            }
            Some(Ev::Updated { id, connected: Some(c) }) => set_connected(&mut eps, &id, c, ready, &bus),
            Some(Ev::Updated { .. }) => {}
            Some(Ev::Removed(id)) => {
                if eps.get(&id).is_some_and(|e| e.connected) {
                    set_connected(&mut eps, &id, false, ready, &bus);
                }
                eps.remove(&id);
            }
        }
    }
    let _ = watcher.Stop();
}

fn set_connected(eps: &mut HashMap<String, Endpoint>, id: &str, connected: bool, ready: bool, bus: &Bus) {
    let Some(e) = eps.get_mut(id) else { return };
    if e.connected == connected {
        return;
    }
    e.connected = connected;
    if !ready {
        return;
    }
    if connected {
        let kind = device_kind(id, &e.name);
        bus.to_module(ID, BtMsg::Connected { id: id.to_string(), name: e.name.clone(), kind });
        read_battery(id.to_string(), e.container, &BATTERY_RETRIES, bus.clone());
    } else {
        bus.to_module(ID, BtMsg::Disconnected { id: id.to_string() });
    }
}

fn watch(tx: Sender<Ev>) -> windows::core::Result<DeviceWatcher> {
    let aqs = format!(
        "({}) OR ({})",
        BluetoothDevice::GetDeviceSelectorFromPairingState(true)?,
        BluetoothLEDevice::GetDeviceSelectorFromPairingState(true)?
    );
    let props: IIterable<HSTRING> = vec![HSTRING::from(IS_CONNECTED), HSTRING::from(CONTAINER)].into();
    let w = DeviceInformation::CreateWatcherWithKindAqsFilterAndAdditionalProperties(
        &HSTRING::from(aqs),
        &props,
        DeviceInformationKind::AssociationEndpoint,
    )?;
    let t = tx.clone();
    w.Added(&TypedEventHandler::<DeviceWatcher, DeviceInformation>::new(move |_, info| {
        if let Ok(info) = info.ok() {
            let p = info.Properties()?;
            let _ = t.send(Ev::Added {
                id: info.Id()?.to_string(),
                name: info.Name()?.to_string(),
                connected: prop::<bool>(&p, IS_CONNECTED).unwrap_or(false),
                container: prop::<GUID>(&p, CONTAINER),
            });
        }
        Ok(())
    }))?;
    let t = tx.clone();
    w.Updated(&TypedEventHandler::<DeviceWatcher, DeviceInformationUpdate>::new(move |_, u| {
        if let Ok(u) = u.ok() {
            let connected = prop::<bool>(&u.Properties()?, IS_CONNECTED);
            let _ = t.send(Ev::Updated { id: u.Id()?.to_string(), connected });
        }
        Ok(())
    }))?;
    let t = tx.clone();
    w.Removed(&TypedEventHandler::<DeviceWatcher, DeviceInformationUpdate>::new(move |_, u| {
        if let Ok(u) = u.ok() {
            let _ = t.send(Ev::Removed(u.Id()?.to_string()));
        }
        Ok(())
    }))?;
    w.EnumerationCompleted(&TypedEventHandler::<DeviceWatcher, IInspectable>::new(move |_, _| {
        let _ = tx.send(Ev::Ready);
        Ok(())
    }))?;
    w.Start()?;
    Ok(w)
}

fn prop<T>(map: &IMapView<HSTRING, IInspectable>, key: &str) -> Option<T>
where
    T: windows::core::RuntimeType + 'static,
{
    map.Lookup(&HSTRING::from(key)).ok()?.cast::<IReference<T>>().ok()?.Value().ok()
}

fn device_kind(id: &str, name: &str) -> Kind {
    let hid = HSTRING::from(id);
    let by_class = if id.starts_with("BluetoothLE#") {
        BluetoothLEDevice::FromIdAsync(&hid).and_then(|op| op.join()).ok().and_then(|d| {
            let a = d.Appearance().ok()?;
            let k = match (a.Category().ok()?, a.SubCategory().ok()?) {
                (0x0F, 1) => Kind::Keyboard,
                (0x0F, 2) => Kind::Mouse,
                (0x0F, 3 | 4) => Kind::Gamepad,
                (0x01, _) => Kind::Phone,
                _ => Kind::Other,
            };
            let _ = d.Close();
            Some(k)
        })
    } else {
        BluetoothDevice::FromIdAsync(&hid).and_then(|op| op.join()).ok().and_then(|d| {
            let k = class_kind(d.ClassOfDevice().ok()?.RawValue().ok()?);
            let _ = d.Close();
            Some(k)
        })
    };
    match by_class {
        Some(k) if k != Kind::Other => k,
        _ => name_kind(name),
    }
}

/// Bluetooth class of device: major class in bits 8-12, minor in 2-7.
fn class_kind(cod: u32) -> Kind {
    let minor = (cod >> 2) & 0x3F;
    match (cod >> 8) & 0x1F {
        2 => Kind::Phone,
        4 => Kind::Audio,
        5 if minor & 0x10 != 0 => Kind::Keyboard,
        5 if minor & 0x20 != 0 => Kind::Mouse,
        5 if matches!(minor & 0x0F, 1 | 2) => Kind::Gamepad,
        _ => Kind::Other,
    }
}

fn name_kind(name: &str) -> Kind {
    let n = name.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| n.contains(w));
    if has(&["airpods", "buds", "headphone", "headset", "earphone", "beats", "wh-", "wf-", "speaker", "soundcore"]) {
        Kind::Audio
    } else if has(&["mouse", "trackpad", "mx master", "mx anywhere"]) {
        Kind::Mouse
    } else if has(&["keyboard", "keys"]) {
        Kind::Keyboard
    } else if has(&["controller", "gamepad", "xbox", "dualsense", "dualshock", "joy-con"]) {
        Kind::Gamepad
    } else if has(&["iphone", "phone", "galaxy", "pixel"]) {
        Kind::Phone
    } else {
        Kind::Other
    }
}

/// Read the battery on a short-lived thread (GATT calls can take seconds),
/// retrying after each delay until a value shows up.
fn read_battery(id: String, container: Option<GUID>, delays: &'static [u64], bus: Bus) {
    let _ = std::thread::Builder::new().name("bluetooth-battery".into()).spawn(move || {
        unsafe {
            let _ = RoInitialize(RO_INIT_MULTITHREADED);
        }
        let mut waited = 0;
        for &at in delays {
            std::thread::sleep(Duration::from_millis(at - waited));
            waited = at;
            let level = container
                .and_then(pnp_battery)
                .or_else(|| id.starts_with("BluetoothLE#").then(|| gatt_battery(&id)).flatten());
            if let Some(level) = level {
                bus.to_module(ID, BtMsg::Battery { id, level: level.min(100) });
                return;
            }
        }
    });
}

/// The battery percentage Windows keeps on the device's PnP nodes.
fn pnp_battery(container: GUID) -> Option<u8> {
    let aqs = HSTRING::from(format!("System.Devices.ContainerId:=\"{{{container:?}}}\""));
    let props: IIterable<HSTRING> = vec![HSTRING::from(BATTERY)].into();
    let list = DeviceInformation::FindAllAsyncWithKindAqsFilterAndAdditionalProperties(
        &aqs,
        &props,
        DeviceInformationKind::Device,
    )
    .ok()?
    .join()
    .ok()?;
    (0..list.Size().ok()?).find_map(|i| prop::<u8>(&list.GetAt(i).ok()?.Properties().ok()?, BATTERY))
}

/// GATT Battery Service (0x180F) / Battery Level (0x2A19).
fn gatt_battery(id: &str) -> Option<u8> {
    let dev = BluetoothLEDevice::FromIdAsync(&HSTRING::from(id)).ok()?.join().ok()?;
    let read = || -> Option<u8> {
        let services = dev
            .GetGattServicesForUuidWithCacheModeAsync(GattServiceUuids::Battery().ok()?, BluetoothCacheMode::Cached)
            .ok()?
            .join()
            .ok()?;
        if services.Status().ok()? != GattCommunicationStatus::Success {
            return None;
        }
        let service = services.Services().ok()?.GetAt(0).ok()?;
        let level = (|| {
            let chars = service
                .GetCharacteristicsForUuidWithCacheModeAsync(
                    GattCharacteristicUuids::BatteryLevel().ok()?,
                    BluetoothCacheMode::Cached,
                )
                .ok()?
                .join()
                .ok()?;
            let ch = chars.Characteristics().ok()?.GetAt(0).ok()?;
            let r = ch.ReadValueWithCacheModeAsync(BluetoothCacheMode::Uncached).ok()?.join().ok()?;
            if r.Status().ok()? != GattCommunicationStatus::Success {
                return None;
            }
            DataReader::FromBuffer(&r.Value().ok()?).ok()?.ReadByte().ok()
        })();
        let _ = service.Close();
        level
    };
    let level = read();
    let _ = dev.Close();
    level
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classes() {
        assert_eq!(class_kind(0x240404), Kind::Audio); // wearable headset
        assert_eq!(class_kind(0x002540), Kind::Keyboard);
        assert_eq!(class_kind(0x002580), Kind::Mouse);
        assert_eq!(class_kind(0x002508), Kind::Gamepad);
        assert_eq!(class_kind(0x5a020c), Kind::Phone);
        assert_eq!(name_kind("Sony WH-1000XM5"), Kind::Audio);
        assert_eq!(name_kind("Xbox Wireless Controller"), Kind::Gamepad);
        assert_eq!(name_kind("Thing"), Kind::Other);
    }
}

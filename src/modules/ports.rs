//! Port watcher: local dev servers (localhost:3000, :5173, ...) with an Open
//! button.
//!
//! Windows has no notification for new listening sockets, so a worker reads
//! the TCP listener tables (IPv4 + IPv6; recent Node/Vite bind only to ::1)
//! every few seconds — one cheap syscall — and only reports changes. A
//! listener counts as a dev server when it is reachable on localhost and is
//! owned by a dev runtime (node, python, bun, a cargo-built binary, ...).

use std::any::Any;
use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use windows::core::PWSTR;
use windows::Win32::Foundation::{CloseHandle, NO_ERROR};
use windows::Win32::NetworkManagement::IpHelper::{
    GetExtendedTcpTable, MIB_TCP6ROW_OWNER_PID, MIB_TCPROW_OWNER_PID, TCP_TABLE_OWNER_PID_LISTENER,
};
use windows::Win32::Networking::WinSock::{AF_INET, AF_INET6};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};

use super::{CardSize, Cx, Module, ModuleId, Peek, SystemEvent};
use crate::bus::Bus;
use crate::config::PortsCfg;
use crate::gfx::{Icon, TextStyle};
use crate::sys::{self, clip, http};
use crate::ui::{ButtonStyle, Ui};
use crate::util::{palette, Rect, SendHwnd};

const ID: ModuleId = "ports";
const ROW_H: f32 = 42.0;
const MAX_ROWS: usize = 6;
const SCAN_EVERY: Duration = Duration::from_secs(3);
/// A port that comes back within this window is a restart, not a new server.
const RESTART_GRACE: Duration = Duration::from_secs(120);

const DEV_PROCESSES: &[&str] = &[
    "node",
    "bun",
    "deno",
    "python",
    "python3",
    "pythonw",
    "py",
    "uvicorn",
    "gunicorn",
    "hypercorn",
    "flask",
    "streamlit",
    "jupyter",
    "jupyter-lab",
    "ruby",
    "puma",
    "rails",
    "php",
    "php-cgi",
    "java",
    "javaw",
    "dotnet",
    "go",
    "air",
    "hugo",
    "caddy",
    "nginx",
    "httpd",
    "wrangler",
    "workerd",
    "esbuild",
    "http-server",
    "live-server",
    "wslrelay",
    "com.docker.backend",
];

#[derive(Clone, Debug, PartialEq)]
struct Listener {
    port: u16,
    pid: u32,
    /// Lowercase file name without ".exe" ("node").
    exe: String,
    /// Lowercase full image path.
    path: String,
    v4: bool,
    v6: bool,
}

enum PortsMsg {
    Snapshot(Vec<Listener>),
    Title(u16, u32, Option<String>),
}

struct Server {
    l: Listener,
    title: Option<String>,
}

impl Server {
    fn url(&self) -> String {
        format!("http://localhost:{}", self.l.port)
    }
    fn detail(&self) -> String {
        match &self.title {
            Some(t) => format!("{t} · {}", self.l.exe),
            None => self.l.exe.clone(),
        }
    }
}

pub struct Ports {
    all: Vec<Listener>,
    servers: Vec<Server>,
    /// Last time each port was seen as a dev server.
    seen: HashMap<u16, Instant>,
    initialized: bool,
    running: bool,
    copied: Option<(u16, Instant)>,
    hwnd: SendHwnd,
    bus: Option<Bus>,
}

impl Ports {
    pub fn new() -> Self {
        Self {
            all: Vec::new(),
            servers: Vec::new(),
            seen: HashMap::new(),
            initialized: false,
            running: false,
            copied: None,
            hwnd: SendHwnd(0),
            bus: None,
        }
    }

    /// Rebuild the server list from the latest snapshot; returns the new ones.
    fn refresh(&mut self, cfg: &PortsCfg) -> Vec<usize> {
        let mut next = Vec::new();
        let mut fresh = Vec::new();
        for l in self.all.iter().filter(|l| is_dev(l, cfg)) {
            let old = self.servers.iter().position(|s| s.l.port == l.port && s.l.pid == l.pid);
            let title = old.and_then(|i| self.servers[i].title.clone());
            if old.is_none() {
                fresh.push(next.len());
            }
            next.push(Server { l: l.clone(), title });
        }
        self.servers = next;
        fresh
    }

    fn fetch_title(&self, s: &Server) {
        let Some(bus) = self.bus.clone() else { return };
        let (port, pid) = (s.l.port, s.l.pid);
        let host = if s.l.v4 { "127.0.0.1".to_string() } else { "[::1]".to_string() };
        let _ = std::thread::Builder::new().name("ports-title".into()).spawn(move || {
            let url = format!("http://{host}:{port}/");
            let res = http::fetch(
                "GET",
                &url,
                &[("Accept", "text/html".into())],
                &[],
                http::Timeouts(1000, 1000, 1000, 2000),
                64 * 1024,
                &AtomicBool::new(false),
            );
            let title = res.ok().and_then(|(_, body)| html_title(&String::from_utf8_lossy(&body)));
            bus.to_module(ID, PortsMsg::Title(port, pid, title));
        });
    }
}

impl Module for Ports {
    fn id(&self) -> ModuleId {
        ID
    }
    fn title(&self) -> &str {
        "Dev Servers"
    }
    fn icon(&self) -> Icon {
        Icon::Globe
    }

    fn start(&mut self, cx: &mut Cx) {
        self.hwnd = cx.hwnd;
        self.bus = Some(cx.bus.clone());
        if self.running {
            return;
        }
        self.running = true;
        let bus = cx.bus.clone();
        let _ = std::thread::Builder::new().name("ports".into()).spawn(move || worker(bus));
    }

    fn on_message(&mut self, msg: Box<dyn Any + Send>, cx: &mut Cx) {
        let Ok(m) = msg.downcast::<PortsMsg>() else { return };
        match *m {
            PortsMsg::Snapshot(all) => {
                self.all = all;
                let fresh = self.refresh(&cx.cfg.ports);
                let now = cx.now;
                for &i in &fresh {
                    let s = &self.servers[i];
                    let restarted = self.seen.get(&s.l.port).is_some_and(|t| now - *t < RESTART_GRACE);
                    if self.initialized && !restarted && cx.cfg.ports.peek_on_start {
                        cx.fx.peek(
                            Peek::new(
                                Icon::Globe,
                                palette::TEAL,
                                "Dev server started",
                                format!("localhost:{} · {}", s.l.port, s.l.exe),
                            )
                            .key("ports")
                            .duration_ms(3500)
                            .page(ID),
                        );
                    }
                    self.fetch_title(s);
                }
                for s in &self.servers {
                    self.seen.insert(s.l.port, now);
                }
                self.initialized = true;
            }
            PortsMsg::Title(port, pid, title) => {
                if let Some(s) = self.servers.iter_mut().find(|s| s.l.port == port && s.l.pid == pid) {
                    s.title = title;
                }
            }
        }
        cx.fx.redraw = true;
    }

    fn on_system(&mut self, ev: &SystemEvent, cx: &mut Cx) {
        if let SystemEvent::ConfigReloaded = ev {
            for i in self.refresh(&cx.cfg.ports) {
                self.fetch_title(&self.servers[i]);
            }
            cx.fx.redraw = true;
        }
    }

    fn card(&self) -> Option<CardSize> {
        (!self.servers.is_empty()).then_some(CardSize::Small)
    }

    fn draw_card(&mut self, ui: &mut Ui, r: Rect) {
        if ui.card(r, true) {
            ui.fx.open_page = Some(ID);
        }
        let Some(s) = self.servers.first() else { return };
        let inner = r.inset_xy(12.0, 0.0);
        ui.p.icon(Icon::Globe, Rect::new(inner.x, r.cy() - 9.0, 18.0, 18.0), palette::TEAL);
        let open = Rect::new(inner.right() - 26.0, r.cy() - 13.0, 26.0, 26.0);
        let tx = inner.x + 28.0;
        let more = self.servers.len() - 1;
        let label =
            if more > 0 { format!("localhost:{}  +{more}", s.l.port) } else { format!("localhost:{}", s.l.port) };
        ui.p.text(&label, Rect::new(tx, r.y, open.x - tx - 6.0, r.h), &TextStyle::new(12.5, palette::TEXT));
        let url = s.url();
        if ui.icon_button(open, Icon::Link, ButtonStyle::default().scale(0.55).fg(palette::TEXT_DIM)) {
            sys::shell_open(&url);
        }
    }

    fn has_page(&self) -> bool {
        true
    }

    fn page_height(&self, _w: f32) -> f32 {
        if self.servers.is_empty() {
            110.0
        } else {
            28.0 + self.servers.len().min(MAX_ROWS) as f32 * (ROW_H + 4.0)
        }
    }

    fn draw_page(&mut self, ui: &mut Ui, r: Rect) {
        let (head, body) = r.split_top(24.0, 4.0);
        ui.caption("DEV SERVERS", Rect::new(head.x, head.y, 200.0, head.h));
        if self.servers.is_empty() {
            ui.empty_state(body, Icon::Globe, "Local dev servers show up here");
            return;
        }
        let copied = self.copied.filter(|c| c.1.elapsed() < Duration::from_millis(1500)).map(|c| c.0);
        let mut copy = None;
        let mut y = body.y;
        for s in self.servers.iter().take(MAX_ROWS) {
            let row = Rect::new(body.x, y, body.w, ROW_H);
            ui.p.fill_rounded(row, 10.0, palette::CARD);
            ui.p.icon(Icon::Globe, Rect::new(row.x + 11.0, row.cy() - 9.0, 18.0, 18.0), palette::TEAL);
            let open = Rect::new(row.right() - 72.0, row.cy() - 13.0, 64.0, 26.0);
            let cb = Rect::new(open.x - 32.0, row.cy() - 13.0, 26.0, 26.0);
            let tx = row.x + 40.0;
            let tw = cb.x - tx - 8.0;
            ui.p.text(
                &format!("localhost:{}", s.l.port),
                Rect::new(tx, row.y + 4.0, tw, 18.0),
                &TextStyle::new(13.0, palette::TEXT).semibold(),
            );
            ui.p.text(&s.detail(), Rect::new(tx, row.y + 21.0, tw, 16.0), &TextStyle::new(11.5, palette::TEXT_DIM));
            let done = copied == Some(s.l.port);
            let icon = if done { Icon::Check } else { Icon::Copy };
            let fg = if done { palette::GREEN } else { palette::TEXT_DIM };
            if ui.icon_button(cb, icon, ButtonStyle::default().scale(0.55).fg(fg)) {
                copy = Some((s.l.port, s.url()));
            }
            if ui.pill_button(open, "Open", None, ButtonStyle::filled(palette::TEAL.with_a(0.22)).fg(palette::TEAL)) {
                sys::shell_open(&s.url());
            }
            y += ROW_H + 4.0;
        }
        if let Some((port, url)) = copy {
            if clip::set_text(self.hwnd.hwnd(), &url) {
                self.copied = Some((port, Instant::now()));
            }
        }
        if copied.is_some() {
            ui.animate(); // let the check mark time out
        }
    }
}

fn is_dev(l: &Listener, cfg: &PortsCfg) -> bool {
    if cfg.ignore_ports.contains(&l.port) {
        return false;
    }
    if cfg.extra_ports.contains(&l.port) {
        return true;
    }
    // ephemeral ports are IPC/debug channels, 9229 is the Node inspector
    if l.port >= 49152 || l.port == 9229 {
        return false;
    }
    let extra = cfg.extra_processes.iter().any(|p| p.to_ascii_lowercase().trim_end_matches(".exe") == l.exe);
    let cargo_built = l.path.contains("\\target\\debug\\") || l.path.contains("\\target\\release\\");
    extra || cargo_built || DEV_PROCESSES.contains(&l.exe.as_str())
}

/// `<title>` of an HTML page, whitespace-collapsed and entity-decoded.
fn html_title(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let start = lower.find("<title")?;
    let open_end = start + lower[start..].find('>')? + 1;
    let close = open_end + lower[open_end..].find("</title")?;
    let raw = &html[open_end..close];
    let text = raw
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'");
    let t: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    (!t.is_empty()).then(|| crate::util::truncate_chars(&t, 60))
}

// ---------------------------------------------------------------------------
// Worker
// ---------------------------------------------------------------------------

fn worker(bus: Bus) {
    let mut names: HashMap<u32, (String, String)> = HashMap::new();
    let mut last: Option<Vec<Listener>> = None;
    loop {
        let rows = listeners();
        names.retain(|pid, _| rows.iter().any(|r| r.1 == *pid));
        let mut out: Vec<Listener> = Vec::new();
        for (port, pid, v6) in rows {
            if let Some(l) = out.iter_mut().find(|l| l.port == port && l.pid == pid) {
                if v6 {
                    l.v6 = true;
                } else {
                    l.v4 = true;
                }
                continue;
            }
            let (exe, path) = names.entry(pid).or_insert_with(|| process_name(pid)).clone();
            if exe.is_empty() {
                continue; // system or elevated process
            }
            out.push(Listener { port, pid, exe, path, v4: !v6, v6 });
        }
        out.sort_by_key(|l| (l.port, l.pid));
        if last.as_ref() != Some(&out) {
            bus.to_module(ID, PortsMsg::Snapshot(out.clone()));
            last = Some(out);
        }
        std::thread::sleep(SCAN_EVERY);
    }
}

/// (port, pid, is_ipv6) for every listening socket reachable via localhost.
fn listeners() -> Vec<(u16, u32, bool)> {
    let mut out = Vec::new();
    for v6 in [false, true] {
        let af = if v6 { AF_INET6.0 } else { AF_INET.0 } as u32;
        let mut size = 0u32;
        unsafe {
            GetExtendedTcpTable(None, &mut size, false, af, TCP_TABLE_OWNER_PID_LISTENER, 0);
        }
        if size == 0 {
            continue;
        }
        // u32 storage keeps the rows 4-byte aligned
        let mut buf = vec![0u32; size as usize / 4 + 1];
        let r = unsafe {
            GetExtendedTcpTable(Some(buf.as_mut_ptr().cast()), &mut size, false, af, TCP_TABLE_OWNER_PID_LISTENER, 0)
        };
        if r != NO_ERROR.0 {
            continue;
        }
        let n = buf[0] as usize;
        let base = unsafe { buf.as_ptr().add(1) as *const u8 };
        for i in 0..n {
            unsafe {
                if v6 {
                    let row = std::ptr::read_unaligned((base as *const MIB_TCP6ROW_OWNER_PID).add(i));
                    let a = row.ucLocalAddr;
                    let any = a == [0; 16];
                    let loopback = a[..15] == [0; 15] && a[15] == 1;
                    if any || loopback {
                        out.push((port(row.dwLocalPort), row.dwOwningPid, true));
                    }
                } else {
                    let row = std::ptr::read_unaligned((base as *const MIB_TCPROW_OWNER_PID).add(i));
                    let a = row.dwLocalAddr.to_le_bytes(); // network order in memory
                    if a == [0; 4] || a[0] == 127 {
                        out.push((port(row.dwLocalPort), row.dwOwningPid, false));
                    }
                }
            }
        }
    }
    out
}

fn port(dw: u32) -> u16 {
    u16::from_be((dw & 0xFFFF) as u16)
}

/// (lowercase exe stem, lowercase full path); empty when not accessible.
fn process_name(pid: u32) -> (String, String) {
    if pid <= 4 {
        return (String::new(), String::new());
    }
    unsafe {
        let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return (String::new(), String::new());
        };
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len).is_ok();
        let _ = CloseHandle(h);
        if !ok {
            return (String::new(), String::new());
        }
        let path = String::from_utf16_lossy(&buf[..len as usize]).to_ascii_lowercase();
        let file = path.rsplit('\\').next().unwrap_or(&path);
        let stem = file.strip_suffix(".exe").unwrap_or(file).to_string();
        (stem, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn l(port: u16, exe: &str, path: &str) -> Listener {
        Listener { port, pid: 1, exe: exe.into(), path: path.into(), v4: true, v6: false }
    }

    #[test]
    fn dev_detection() {
        let cfg = PortsCfg::default();
        assert!(is_dev(&l(5173, "node", r"c:\nodejs\node.exe"), &cfg));
        assert!(is_dev(&l(8080, "api", r"c:\code\api\target\debug\api.exe"), &cfg));
        assert!(!is_dev(&l(5173, "spotify", r"c:\spotify\spotify.exe"), &cfg));
        assert!(!is_dev(&l(53000, "node", r"c:\nodejs\node.exe"), &cfg));
        assert!(!is_dev(&l(9229, "node", r"c:\nodejs\node.exe"), &cfg));
        let cfg = PortsCfg {
            extra_ports: vec![11434],
            ignore_ports: vec![3000],
            extra_processes: vec!["MyServer.exe".into()],
            ..PortsCfg::default()
        };
        assert!(is_dev(&l(11434, "ollama", ""), &cfg));
        assert!(!is_dev(&l(3000, "node", ""), &cfg));
        assert!(is_dev(&l(4000, "myserver", ""), &cfg));
    }

    #[test]
    fn titles() {
        assert_eq!(html_title("<html><head><TITLE>\n  My &amp; App </TITLE>").as_deref(), Some("My & App"));
        assert_eq!(html_title("<title lang=en>Vite</title>").as_deref(), Some("Vite"));
        assert_eq!(html_title("<title></title>"), None);
        assert_eq!(html_title("no title"), None);
        assert_eq!(port(0x3514), 5173); // 5173 = 0x1435, stored big-endian
    }
}

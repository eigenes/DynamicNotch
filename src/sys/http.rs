//! Blocking HTTPS POST with streamed response body, on top of WinHTTP
//! (uses the system TLS stack and proxy settings; no extra dependencies).

use std::sync::atomic::{AtomicBool, Ordering};

use windows::core::{w, PCWSTR};
use windows::Win32::Networking::WinHttp::*;

use crate::util::wide;

struct Handle(*mut core::ffi::c_void);

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let _ = WinHttpCloseHandle(self.0);
            }
        }
    }
}

pub struct Url {
    pub secure: bool,
    pub host: String,
    pub port: u16,
    pub path: String,
}

pub fn parse_url(url: &str) -> Option<Url> {
    let (secure, rest) = if let Some(r) = url.strip_prefix("https://") {
        (true, r)
    } else if let Some(r) = url.strip_prefix("http://") {
        (false, r)
    } else {
        return None;
    };
    let (hostport, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) => (h, p.parse().ok()?),
        None => (hostport, if secure { 443 } else { 80 }),
    };
    if host.is_empty() {
        return None;
    }
    Some(Url { secure, host: host.to_string(), port, path: path.to_string() })
}

/// WinHTTP timeouts in milliseconds: (resolve, connect, send, receive).
#[derive(Clone, Copy)]
pub struct Timeouts(pub i32, pub i32, pub i32, pub i32);

impl Timeouts {
    pub const DEFAULT: Timeouts = Timeouts(10_000, 15_000, 30_000, 120_000);
}

/// POST `body` and feed the response body to `on_chunk` as it arrives.
/// Returns the HTTP status code. `on_chunk` returning false (or `cancel`
/// becoming true) aborts the transfer.
pub fn post_stream(
    url: &str,
    headers: &[(&str, String)],
    body: &[u8],
    cancel: &AtomicBool,
    on_chunk: impl FnMut(&[u8]) -> bool,
) -> Result<u16, String> {
    request("POST", url, headers, body, Timeouts::DEFAULT, cancel, on_chunk)
}

/// Send a request and collect up to `limit` bytes of the response body.
pub fn fetch(
    method: &str,
    url: &str,
    headers: &[(&str, String)],
    body: &[u8],
    timeouts: Timeouts,
    limit: usize,
    cancel: &AtomicBool,
) -> Result<(u16, Vec<u8>), String> {
    let mut out = Vec::new();
    let status = request(method, url, headers, body, timeouts, cancel, |c| {
        out.extend_from_slice(&c[..c.len().min(limit.saturating_sub(out.len()))]);
        out.len() < limit
    })?;
    Ok((status, out))
}

/// Shared implementation of `post_stream` / `fetch`.
pub fn request(
    method: &str,
    url: &str,
    headers: &[(&str, String)],
    body: &[u8],
    timeouts: Timeouts,
    cancel: &AtomicBool,
    mut on_chunk: impl FnMut(&[u8]) -> bool,
) -> Result<u16, String> {
    let u = parse_url(url).ok_or_else(|| format!("invalid URL: {url}"))?;
    unsafe {
        let session = Handle(WinHttpOpen(
            w!("DynamicNotch/0.1"),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            PCWSTR::null(),
            PCWSTR::null(),
            0,
        ));
        if session.0.is_null() {
            return Err("WinHttpOpen failed".into());
        }
        let Timeouts(t_resolve, t_connect, t_send, t_receive) = timeouts;
        let _ = WinHttpSetTimeouts(session.0, t_resolve, t_connect, t_send, t_receive);
        let host = wide(&u.host);
        let connect = Handle(WinHttpConnect(session.0, PCWSTR(host.as_ptr()), u.port, 0));
        if connect.0.is_null() {
            return Err(format!("cannot connect to {}", u.host));
        }
        let path = wide(&u.path);
        let verb = wide(method);
        let flags = if u.secure { WINHTTP_FLAG_SECURE } else { WINHTTP_OPEN_REQUEST_FLAGS(0) };
        let req = Handle(WinHttpOpenRequest(
            connect.0,
            PCWSTR(verb.as_ptr()),
            PCWSTR(path.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            std::ptr::null(),
            flags,
        ));
        if req.0.is_null() {
            return Err("WinHttpOpenRequest failed".into());
        }
        let mut hdr = String::new();
        for (k, v) in headers {
            hdr.push_str(k);
            hdr.push_str(": ");
            hdr.push_str(v);
            hdr.push_str("\r\n");
        }
        if !hdr.is_empty() {
            let hdr_w: Vec<u16> = hdr.encode_utf16().collect();
            WinHttpAddRequestHeaders(req.0, &hdr_w, WINHTTP_ADDREQ_FLAG_ADD | WINHTTP_ADDREQ_FLAG_REPLACE)
                .map_err(|e| format!("headers: {e}"))?;
        }
        let data = (!body.is_empty()).then(|| body.as_ptr() as *const _);
        WinHttpSendRequest(req.0, None, data, body.len() as u32, body.len() as u32, 0)
            .map_err(|e| net_error("send", e))?;
        WinHttpReceiveResponse(req.0, std::ptr::null_mut()).map_err(|e| net_error("receive", e))?;

        let mut status: u32 = 0;
        let mut len = 4u32;
        let mut idx = 0u32;
        let _ = WinHttpQueryHeaders(
            req.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some(&mut status as *mut u32 as *mut _),
            &mut len,
            &mut idx,
        );

        let mut buf = vec![0u8; 16 * 1024];
        loop {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            let mut avail = 0u32;
            WinHttpQueryDataAvailable(req.0, &mut avail).map_err(|e| net_error("read", e))?;
            if avail == 0 {
                break;
            }
            let want = (avail as usize).min(buf.len());
            let mut read = 0u32;
            WinHttpReadData(req.0, buf.as_mut_ptr() as *mut _, want as u32, &mut read)
                .map_err(|e| net_error("read", e))?;
            if read == 0 {
                break;
            }
            if !on_chunk(&buf[..read as usize]) {
                break;
            }
        }
        Ok(status as u16)
    }
}

fn net_error(stage: &str, e: windows::core::Error) -> String {
    let code = (e.code().0 as u32) & 0xFFFF;
    let msg = match code {
        12002 => "request timed out",
        12007 => "could not resolve host (offline?)",
        12029 => "could not connect",
        12030 => "connection was closed",
        12175 => "TLS error",
        _ => "",
    };
    if msg.is_empty() {
        format!("network error during {stage} ({code})")
    } else {
        msg.to_string()
    }
}

/// Incremental Server-Sent-Events line splitter: yields complete `data:` payloads.
#[derive(Default)]
pub struct SseParser {
    buf: Vec<u8>,
}

impl SseParser {
    pub fn push(&mut self, chunk: &[u8], mut on_data: impl FnMut(&str)) {
        self.buf.extend_from_slice(chunk);
        while let Some(pos) = self.buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=pos).collect();
            let line = String::from_utf8_lossy(&line);
            let line = line.trim_end_matches(['\r', '\n']);
            if let Some(data) = line.strip_prefix("data:") {
                on_data(data.trim_start());
            }
            // comments (": keep-alive") and other fields are ignored
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_parsing() {
        let u = parse_url("https://openrouter.ai/api/v1/chat/completions").unwrap();
        assert!(u.secure);
        assert_eq!(u.host, "openrouter.ai");
        assert_eq!(u.port, 443);
        assert_eq!(u.path, "/api/v1/chat/completions");
        let u = parse_url("http://localhost:8080").unwrap();
        assert_eq!((u.port, u.path.as_str()), (8080, "/"));
    }

    #[test]
    fn sse_split_across_chunks() {
        let mut p = SseParser::default();
        let mut got = Vec::new();
        p.push(b": OPENROUTER PROCESSING\n\ndata: {\"a\"", |d| got.push(d.to_string()));
        p.push(b":1}\r\n\ndata: [DONE]\n", |d| got.push(d.to_string()));
        assert_eq!(got, vec!["{\"a\":1}".to_string(), "[DONE]".to_string()]);
    }
}

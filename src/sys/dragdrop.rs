//! OLE drag & drop for the file shelf: the notch window accepts dropped files
//! (as references — the drop always reports COPY, never MOVE, so the source
//! keeps its files), and parked files are dragged back out with the shell's
//! own data object, so Explorer, browsers and chat apps all understand them.

use std::cell::Cell;
use std::sync::atomic::{AtomicBool, Ordering};

use windows::core::{implement, Ref, Result, PCWSTR};
use windows::Win32::Foundation::{HWND, POINTL, S_OK, SIZE};
use windows::Win32::Graphics::Gdi::{
    DeleteObject, GetDC, GetDIBits, GetObjectW, ReleaseDC, BITMAP, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
    DIB_RGB_COLORS, HGDIOBJ,
};
use windows::Win32::System::Com::{IDataObject, DVASPECT_CONTENT, FORMATETC, TYMED_HGLOBAL};
use windows::Win32::System::Ole::{
    IDropSource, IDropTarget, IDropTarget_Impl, OleInitialize, RegisterDragDrop, ReleaseStgMedium, RevokeDragDrop,
    CF_HDROP, DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_LINK, DROPEFFECT_MOVE, DROPEFFECT_NONE,
};
use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    DragQueryFileW, IShellItemArray, IShellItemImageFactory, ILFree, SHCreateItemFromParsingName,
    SHCreateShellItemArrayFromIDLists, SHDoDragDrop, SHParseDisplayName, BHID_DataObject, HDROP, SIIGBF_RESIZETOFIT,
};

use crate::gfx::ImageData;
use crate::util::wide;

/// Set while one of our own drags is running, so dragging a parked file
/// over the notch doesn't park it again.
static DRAGGING_OUT: AtomicBool = AtomicBool::new(false);

pub enum DropEvent {
    /// Files entered the notch. The handler returns whether they are accepted.
    Enter,
    Leave,
    Drop(Vec<String>),
}

#[implement(IDropTarget)]
struct Target {
    handler: fn(DropEvent) -> bool,
    accepting: Cell<bool>,
}

fn file_format() -> FORMATETC {
    FORMATETC {
        cfFormat: CF_HDROP.0,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    }
}

/// COPY when the source allows it (else LINK): parked files are references.
fn pick(allowed: DROPEFFECT) -> DROPEFFECT {
    if allowed.0 & DROPEFFECT_COPY.0 != 0 {
        DROPEFFECT_COPY
    } else if allowed.0 & DROPEFFECT_LINK.0 != 0 {
        DROPEFFECT_LINK
    } else {
        DROPEFFECT_NONE
    }
}

impl IDropTarget_Impl for Target_Impl {
    fn DragEnter(
        &self,
        data: Ref<IDataObject>,
        _keys: MODIFIERKEYS_FLAGS,
        _pt: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> Result<()> {
        let has_files = data.as_ref().map_or(false, |d| unsafe { d.QueryGetData(&file_format()) } == S_OK);
        let ok = has_files && !DRAGGING_OUT.load(Ordering::Relaxed) && (self.handler)(DropEvent::Enter);
        self.accepting.set(ok);
        unsafe { *effect = if ok { pick(*effect) } else { DROPEFFECT_NONE } };
        Ok(())
    }

    fn DragOver(&self, _keys: MODIFIERKEYS_FLAGS, _pt: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
        unsafe { *effect = if self.accepting.get() { pick(*effect) } else { DROPEFFECT_NONE } };
        Ok(())
    }

    fn DragLeave(&self) -> Result<()> {
        if self.accepting.replace(false) {
            (self.handler)(DropEvent::Leave);
        }
        Ok(())
    }

    fn Drop(&self, data: Ref<IDataObject>, _keys: MODIFIERKEYS_FLAGS, _pt: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
        if !self.accepting.replace(false) {
            unsafe { *effect = DROPEFFECT_NONE };
            return Ok(());
        }
        let files = data.as_ref().map(|d| unsafe { dropped_files(d) }).unwrap_or_default();
        unsafe { *effect = if files.is_empty() { DROPEFFECT_NONE } else { pick(*effect) } };
        (self.handler)(DropEvent::Drop(files));
        Ok(())
    }
}

unsafe fn dropped_files(data: &IDataObject) -> Vec<String> {
    let mut out = Vec::new();
    let Ok(mut medium) = data.GetData(&file_format()) else { return out };
    let hdrop = HDROP(medium.u.hGlobal.0);
    let n = DragQueryFileW(hdrop, u32::MAX, None);
    for i in 0..n {
        let len = DragQueryFileW(hdrop, i, None) as usize;
        let mut buf = vec![0u16; len + 1];
        let got = DragQueryFileW(hdrop, i, Some(&mut buf)) as usize;
        if got > 0 {
            out.push(String::from_utf16_lossy(&buf[..got]));
        }
    }
    ReleaseStgMedium(&mut medium);
    out
}

/// Make `hwnd` a drop target for files. `handler` runs on the UI thread.
pub fn register_target(hwnd: HWND, handler: fn(DropEvent) -> bool) {
    unsafe {
        // the thread is already an STA (RoInitialize); OLE needs its own init on top
        if let Err(e) = OleInitialize(None) {
            crate::log!("OleInitialize failed: {e}");
            return;
        }
        let target: IDropTarget = Target { handler, accepting: Cell::new(false) }.into();
        if let Err(e) = RegisterDragDrop(hwnd, &target) {
            crate::log!("RegisterDragDrop failed: {e}");
        }
    }
}

pub fn revoke_target(hwnd: HWND) {
    unsafe {
        let _ = RevokeDragDrop(hwnd);
    }
}

/// Drag `files` out of the notch (modal until the drop). The shell decides
/// copy vs. move like any Explorer drag; files are never deleted here.
pub fn drag_out(hwnd: HWND, files: &[String]) -> Option<DROPEFFECT> {
    unsafe {
        let mut pidls: Vec<*mut ITEMIDLIST> = Vec::new();
        for f in files {
            let w = wide(f);
            let mut p: *mut ITEMIDLIST = std::ptr::null_mut();
            if SHParseDisplayName(PCWSTR(w.as_ptr()), None, &mut p, 0, None).is_ok() && !p.is_null() {
                pidls.push(p);
            }
        }
        if pidls.is_empty() {
            return None;
        }
        let consts: Vec<*const ITEMIDLIST> = pidls.iter().map(|p| *p as *const _).collect();
        let res = (|| -> Result<DROPEFFECT> {
            let items: IShellItemArray = SHCreateShellItemArrayFromIDLists(&consts)?;
            let data: IDataObject = items.BindToHandler(None, &BHID_DataObject)?;
            DRAGGING_OUT.store(true, Ordering::Relaxed);
            let allowed = DROPEFFECT(DROPEFFECT_COPY.0 | DROPEFFECT_MOVE.0 | DROPEFFECT_LINK.0);
            let r = SHDoDragDrop(Some(hwnd), &data, None::<&IDropSource>, allowed);
            DRAGGING_OUT.store(false, Ordering::Relaxed);
            r
        })();
        for p in pidls {
            ILFree(Some(p));
        }
        match res {
            Ok(e) => Some(e),
            Err(e) => {
                crate::log!("drag out failed: {e}");
                None
            }
        }
    }
}

/// Shell thumbnail (or icon) for a file or folder. COM must be initialised
/// on the calling thread.
pub fn thumbnail(path: &str, size: i32) -> Option<ImageData> {
    unsafe {
        let w = wide(path);
        let factory: IShellItemImageFactory = SHCreateItemFromParsingName(PCWSTR(w.as_ptr()), None).ok()?;
        let hbm = factory.GetImage(SIZE { cx: size, cy: size }, SIIGBF_RESIZETOFIT).ok()?;
        let mut bm = BITMAP::default();
        let got = GetObjectW(HGDIOBJ(hbm.0), std::mem::size_of::<BITMAP>() as i32, Some(&mut bm as *mut _ as *mut _));
        let (bw, bh) = (bm.bmWidth, bm.bmHeight.abs());
        let mut px = vec![0u8; (bw.max(0) * bh * 4) as usize];
        let mut ok = got != 0 && bw > 0 && bh > 0;
        if ok {
            let mut bi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: bw,
                    biHeight: -bh, // top-down
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let dc = GetDC(None);
            ok = GetDIBits(dc, hbm, 0, bh as u32, Some(px.as_mut_ptr().cast()), &mut bi, DIB_RGB_COLORS) != 0;
            ReleaseDC(None, dc);
        }
        let _ = DeleteObject(HGDIOBJ(hbm.0));
        if !ok {
            return None;
        }
        fix_alpha(&mut px);
        Some(ImageData::from_pbgra(bw as u32, bh as u32, px))
    }
}

/// Shell bitmaps come either without alpha (photos: all zero) or with
/// straight alpha (icons); the renderer wants premultiplied BGRA.
fn fix_alpha(px: &mut [u8]) {
    if px.chunks_exact(4).all(|p| p[3] == 0) {
        for p in px.chunks_exact_mut(4) {
            p[3] = 255;
        }
        return;
    }
    let straight = px.chunks_exact(4).any(|p| p[0].max(p[1]).max(p[2]) > p[3]);
    if straight {
        for p in px.chunks_exact_mut(4) {
            let a = p[3] as u32;
            for c in &mut p[..3] {
                *c = ((*c as u32 * a + 127) / 255) as u8;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alpha_fixups() {
        let mut opaque = vec![10, 20, 30, 0, 40, 50, 60, 0];
        fix_alpha(&mut opaque);
        assert_eq!(opaque, vec![10, 20, 30, 255, 40, 50, 60, 255]);
        let mut straight = vec![200, 100, 0, 128];
        fix_alpha(&mut straight);
        assert_eq!(straight, vec![100, 50, 0, 128]);
        let mut pre = vec![100, 50, 0, 128];
        fix_alpha(&mut pre);
        assert_eq!(pre, vec![100, 50, 0, 128]);
        assert_eq!(pick(DROPEFFECT(DROPEFFECT_COPY.0 | DROPEFFECT_MOVE.0)), DROPEFFECT_COPY);
        assert_eq!(pick(DROPEFFECT_MOVE), DROPEFFECT_NONE);
    }
}

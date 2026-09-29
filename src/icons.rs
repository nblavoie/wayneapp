//! Chargement asynchrone des icônes (thread dédié) → bitmaps 32 bpp prémultipliés,
//! prêts pour AlphaBlend. L'interface ne bloque jamais sur une icône.

use std::ffi::c_void;
use std::sync::mpsc::{channel, Receiver, Sender};

use windows::core::{Interface, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Com::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

use crate::index::wide;

pub struct Icon {
    pub bmp: isize,
    pub w: i32,
    pub h: i32,
}

impl Icon {
    pub fn free(&self) {
        unsafe {
            let _ = DeleteObject(HGDIOBJ(self.bmp as *mut c_void));
        }
    }
}

pub type Request = (String, String, i32);
pub type Reply = (String, Option<Icon>);

pub fn spawn_worker(hwnd: isize, notify: u32) -> (Sender<Request>, Receiver<Reply>) {
    let (req_tx, req_rx) = channel::<Request>();
    let (rep_tx, rep_rx) = channel::<Reply>();
    std::thread::spawn(move || unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        for (key, src, px) in req_rx {
            let icon = load(&src, px);
            if rep_tx.send((key, icon)).is_err() {
                break;
            }
            let _ = PostMessageW(HWND(hwnd as *mut c_void), notify, WPARAM(0), LPARAM(0));
        }
    });
    (req_tx, rep_rx)
}

unsafe fn load(src: &str, px: i32) -> Option<Icon> {
    let w = wide(src);
    let item: IShellItem = SHCreateItemFromParsingName(PCWSTR(w.as_ptr()), None).ok()?;
    let factory: IShellItemImageFactory = item.cast().ok()?;
    let hbmp = factory.GetImage(SIZE { cx: px, cy: px }, SIIGBF_ICONONLY).ok()?;

    let mut bm = BITMAP::default();
    GetObjectW(HGDIOBJ(hbmp.0), std::mem::size_of::<BITMAP>() as i32, Some(&mut bm as *mut _ as *mut c_void));
    let (bw, bh) = (bm.bmWidth, bm.bmHeight.abs());
    if bw <= 0 || bh <= 0 {
        let _ = DeleteObject(HGDIOBJ(hbmp.0));
        return None;
    }
    let mut bi = BITMAPINFO::default();
    bi.bmiHeader = BITMAPINFOHEADER {
        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: bw,
        biHeight: -bh,
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB.0,
        ..Default::default()
    };
    let mut pixels = vec![0u8; (bw * bh * 4) as usize];
    let dc = CreateCompatibleDC(None);
    GetDIBits(dc, hbmp, 0, bh as u32, Some(pixels.as_mut_ptr() as *mut c_void), &mut bi, DIB_RGB_COLORS);
    let _ = DeleteObject(HGDIOBJ(hbmp.0));

    // Normalisation : pas d'alpha → opaque ; alpha droit → prémultiplié.
    if !pixels.chunks(4).any(|p| p[3] != 0) {
        pixels.chunks_mut(4).for_each(|p| p[3] = 255);
    } else if pixels.chunks(4).any(|p| p[0] > p[3] || p[1] > p[3] || p[2] > p[3]) {
        for p in pixels.chunks_mut(4) {
            let a = p[3] as u32;
            for c in &mut p[..3] {
                *c = ((*c as u32 * a + 127) / 255) as u8;
            }
        }
    }

    let mut bits: *mut c_void = std::ptr::null_mut();
    let out = CreateDIBSection(dc, &bi, DIB_RGB_COLORS, &mut bits, None, 0);
    let _ = DeleteDC(dc);
    let out = out.ok()?;
    std::ptr::copy_nonoverlapping(pixels.as_ptr(), bits as *mut u8, pixels.len());
    Some(Icon { bmp: out.0 as isize, w: bw, h: bh })
}

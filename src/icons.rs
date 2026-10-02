//! Chargement asynchrone des icônes (thread dédié) → bitmaps 32 bpp prémultipliés,
//! prêts pour AlphaBlend. L'interface ne bloque jamais sur une icône.

use std::ffi::c_void;
use std::os::windows::process::CommandExt;
use std::sync::mpsc::{channel, Receiver, Sender};

use windows::core::{Interface, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Com::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, PostMessageW, HICON, ICONINFO};

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

pub enum Request {
    /// Icône d'un élément. `exe` : lire l'icône directement dans ce fichier (rafraîchissement),
    /// sans passer par le cache d'icônes de Windows.
    Load { key: String, src: String, px: i32, exe: Option<String> },
    /// Demande à Windows de reconstruire son cache d'icônes (traité avant les requêtes suivantes).
    RebuildShellCache,
}
pub type Reply = (String, Option<Icon>);

pub fn spawn_worker(hwnd: isize, notify: u32) -> (Sender<Request>, Receiver<Reply>) {
    let (req_tx, req_rx) = channel::<Request>();
    let (rep_tx, rep_rx) = channel::<Reply>();
    std::thread::spawn(move || unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        for req in req_rx {
            match req {
                Request::RebuildShellCache => rebuild_shell_cache(),
                Request::Load { key, src, px, exe } => {
                    let icon = exe.as_deref().and_then(|e| from_exe(e, px)).or_else(|| load(&src, px));
                    if rep_tx.send((key, icon)).is_err() {
                        break;
                    }
                    let _ = PostMessageW(HWND(hwnd as *mut c_void), notify, WPARAM(0), LPARAM(0));
                }
            }
        }
    });
    (req_tx, rep_rx)
}

/// `ie4uinit -show` reconstruit le cache d'icônes de l'utilisateur (menu Démarrer, barre des
/// tâches, Explorateur compris) ; SHCNE_ASSOCCHANGED fait relire les images au shell.
unsafe fn rebuild_shell_cache() {
    let exe = std::env::var("SystemRoot").map(|r| format!("{r}\\System32\\ie4uinit.exe")).unwrap_or_else(|_| "ie4uinit.exe".into());
    let status = std::process::Command::new(&exe).arg("-show").creation_flags(0x0800_0000).status();
    crate::log::log(&format!("rafraîchissement des icônes : ie4uinit -show → {status:?}"));
    SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None);
}

/// Icône lue directement dans un exécutable (ressource n° 0), à la taille demandée.
unsafe fn from_exe(path: &str, px: i32) -> Option<Icon> {
    let w = wide(path);
    let mut large = HICON::default();
    let hr = SHDefExtractIconW(PCWSTR(w.as_ptr()), 0, 0, Some(&mut large), None, px as u32 & 0xFFFF);
    if hr.is_err() || large.is_invalid() {
        return None;
    }
    let mut info = ICONINFO::default();
    let got = GetIconInfo(large, &mut info);
    let _ = DestroyIcon(large);
    got.ok()?;
    if !info.hbmMask.is_invalid() {
        let _ = DeleteObject(HGDIOBJ(info.hbmMask.0));
    }
    if info.hbmColor.is_invalid() {
        return None;
    }
    normalize(info.hbmColor)
}

/// Icône telle que Windows l'affiche pour cet élément (raccourci, app Store, dossier…).
unsafe fn load(src: &str, px: i32) -> Option<Icon> {
    let w = wide(src);
    let item: IShellItem = SHCreateItemFromParsingName(PCWSTR(w.as_ptr()), None).ok()?;
    let factory: IShellItemImageFactory = item.cast().ok()?;
    let hbmp = factory.GetImage(SIZE { cx: px, cy: px }, SIIGBF_ICONONLY).ok()?;
    normalize(hbmp)
}

/// Diagnostic (`--diag-icon <exe>`) : enregistre l'icône vue par le cache de Windows et celle
/// lue dans l'exe, en BMP 32 bpp dans le dossier temporaire, pour les comparer.
pub unsafe fn diag_compare(exe: &str, px: i32) -> Vec<String> {
    let mut out = Vec::new();
    for (label, icon) in [("shell", load(exe, px)), ("exe", from_exe(exe, px))] {
        let path = std::env::temp_dir().join(format!("wayne-icon-{label}.bmp"));
        match icon {
            Some(i) => {
                let mut bi = BITMAPINFO::default();
                bi.bmiHeader = BITMAPINFOHEADER { biSize: 40, biWidth: i.w, biHeight: -i.h, biPlanes: 1, biBitCount: 32, biCompression: BI_RGB.0, ..Default::default() };
                let mut px_buf = vec![0u8; (i.w * i.h * 4) as usize];
                let dc = CreateCompatibleDC(None);
                GetDIBits(dc, HBITMAP(i.bmp as *mut c_void), 0, i.h as u32, Some(px_buf.as_mut_ptr() as *mut c_void), &mut bi, DIB_RGB_COLORS);
                let _ = DeleteDC(dc);
                let mut file = Vec::new();
                file.extend_from_slice(b"BM");
                file.extend_from_slice(&(54 + px_buf.len() as u32).to_le_bytes());
                file.extend_from_slice(&[0u8; 4]);
                file.extend_from_slice(&54u32.to_le_bytes());
                let h = &bi.bmiHeader;
                for v in [40u32, h.biWidth as u32, h.biHeight as u32] {
                    file.extend_from_slice(&v.to_le_bytes());
                }
                file.extend_from_slice(&1u16.to_le_bytes());
                file.extend_from_slice(&32u16.to_le_bytes());
                file.extend_from_slice(&[0u8; 24]);
                file.extend_from_slice(&px_buf);
                let _ = std::fs::write(&path, file);
                out.push(format!("{label} : {}x{} → {}", i.w, i.h, path.display()));
                i.free();
            }
            None => out.push(format!("{label} : aucune icône")),
        }
    }
    out
}

/// Copie un bitmap en DIB 32 bpp prémultiplié (et libère l'original).
unsafe fn normalize(hbmp: HBITMAP) -> Option<Icon> {
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

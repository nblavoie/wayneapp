//! « Déjà ouvert ? » : retrouve une fenêtre existante de l'élément demandé pour la ramener
//! au premier plan au lieu de lancer une nouvelle instance.
//!
//! * Apps Store et apps à identité Windows (Chrome…) : identifiant d'app (AUMID) de la fenêtre,
//!   celui que la barre des tâches utilise pour grouper les fenêtres.
//! * Apps classiques : chemin de l'exécutable du processus propriétaire de la fenêtre.
//! * Dossiers : fenêtre de l'Explorateur dont l'onglet actif affiche ce dossier.

use std::ffi::c_void;

use windows::core::{Interface, GUID, PWSTR, VARIANT};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
use windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc;
use windows::Win32::System::Com::*;
use windows::Win32::System::Threading::*;
use windows::Win32::UI::Shell::PropertiesSystem::{IPropertyStore, SHGetPropertyStoreForWindow, PROPERTYKEY};
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::index::Item;
use crate::matcher::fold;

// System.AppUserModel.ID
const PKEY_AUMID: PROPERTYKEY = PROPERTYKEY { fmtid: GUID::from_u128(0x9F4C2855_9F79_4B39_A8D0_E1D42DE1D5F3), pid: 5 };

/// Exécutables qui hébergent plusieurs outils différents : les comparer confondrait
/// par exemple Services et l'Observateur d'événements (tous deux mmc.exe).
const SHARED_HOSTS: [&str; 16] = [
    "mmc.exe", "rundll32.exe", "explorer.exe", "cmd.exe", "powershell.exe", "pwsh.exe", "wscript.exe", "cscript.exe",
    "msiexec.exe", "java.exe", "javaw.exe", "python.exe", "pythonw.exe", "dllhost.exe", "control.exe", "update.exe",
];

/// Exécutable partagé par plusieurs outils (l'icône vient alors du raccourci, pas de l'exe).
pub fn is_shared_host(path: &str) -> bool {
    let file = path.rsplit('\\').next().unwrap_or("").to_lowercase();
    SHARED_HOSTS.contains(&file.as_str())
}

unsafe extern "system" fn collect(h: HWND, lp: LPARAM) -> BOOL {
    (*(lp.0 as *mut Vec<HWND>)).push(h);
    TRUE
}

/// Fenêtres principales visibles, de la plus haute à la plus basse (ordre Z).
unsafe fn app_windows() -> Vec<HWND> {
    let mut all: Vec<HWND> = Vec::new();
    let _ = EnumWindows(Some(collect), LPARAM(&mut all as *mut _ as isize));
    all.into_iter()
        .filter(|&h| {
            if !IsWindowVisible(h).as_bool() || GetWindowTextLengthW(h) == 0 {
                return false;
            }
            if GetWindow(h, GW_OWNER).map(|o| !o.is_invalid()).unwrap_or(false) {
                return false;
            }
            if GetWindowLongPtrW(h, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW.0 != 0 {
                return false;
            }
            // Fenêtres « masquées » par DWM : apps Store suspendues, autres bureaux virtuels.
            let mut cloaked = 0u32;
            let _ = DwmGetWindowAttribute(h, DWMWA_CLOAKED, &mut cloaked as *mut _ as *mut c_void, 4);
            cloaked == 0
        })
        .collect()
}

unsafe fn class_name(h: HWND) -> String {
    let mut buf = [0u16; 64];
    let n = GetClassNameW(h, &mut buf);
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

unsafe fn title(h: HWND) -> String {
    let mut buf = [0u16; 512];
    let n = GetWindowTextW(h, &mut buf);
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

unsafe fn window_aumid(h: HWND) -> Option<String> {
    let store: IPropertyStore = SHGetPropertyStoreForWindow(h).ok()?;
    let value = store.GetValue(&PKEY_AUMID).ok()?;
    let p = PropVariantToStringAlloc(&value).ok()?;
    let s = p.to_string().unwrap_or_default();
    CoTaskMemFree(Some(p.0 as *const c_void));
    (!s.is_empty()).then_some(s)
}

unsafe fn window_exe(h: HWND) -> Option<String> {
    let mut pid = 0u32;
    GetWindowThreadProcessId(h, Some(&mut pid));
    let hp = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
    let mut buf = [0u16; 1024];
    let mut n = buf.len() as u32;
    let r = QueryFullProcessImageNameW(hp, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut n);
    let _ = CloseHandle(hp);
    r.ok()?;
    Some(String::from_utf16_lossy(&buf[..n as usize]))
}

/// Nom de famille du paquet MSIX du processus (« Claude_pzs8sxrjxfjjc »), s'il est empaqueté.
/// Les apps empaquetées ne posent souvent pas d'identifiant sur leurs fenêtres.
unsafe fn window_package_family(h: HWND) -> Option<String> {
    let mut pid = 0u32;
    GetWindowThreadProcessId(h, Some(&mut pid));
    let hp = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
    let mut buf = [0u16; 256];
    let mut n = buf.len() as u32;
    let r = windows::Win32::Storage::Packaging::Appx::GetPackageFamilyName(hp, &mut n, PWSTR(buf.as_mut_ptr()));
    let _ = CloseHandle(hp);
    if r != ERROR_SUCCESS || n == 0 {
        return None;
    }
    Some(String::from_utf16_lossy(&buf[..n as usize - 1]))
}

/// file:///C:/Users/x/T%C3%A9l%C3%A9chargements → c:\users\x\téléchargements
fn url_to_path(url: &str) -> Option<String> {
    let rest = url.strip_prefix("file:///")?;
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&rest[i + 1..i + 3], 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(if bytes[i] == b'/' { b'\\' } else { bytes[i] });
        i += 1;
    }
    Some(norm_path(&String::from_utf8_lossy(&out)))
}

fn norm_path(p: &str) -> String {
    p.trim_end_matches('\\').to_lowercase()
}

/// Onglets de l'Explorateur ouverts : (fenêtre, dossier affiché).
unsafe fn explorer_tabs() -> Vec<(isize, String)> {
    let mut out = Vec::new();
    let Ok(sw) = CoCreateInstance::<_, IShellWindows>(&ShellWindows, None, CLSCTX_ALL) else { return out };
    let n = sw.Count().unwrap_or(0);
    for i in 0..n {
        let Ok(disp) = sw.Item(&VARIANT::from(i)) else { continue };
        let Ok(web) = disp.cast::<IWebBrowserApp>() else { continue };
        let (Ok(hwnd), Ok(url)) = (web.HWND(), web.LocationURL()) else { continue };
        if let Some(path) = url_to_path(&url.to_string()) {
            out.push((hwnd.0 as isize, path));
        }
    }
    out
}

/// Diagnostic : ce que Windows rapporte pour chaque fenêtre principale.
pub unsafe fn describe_windows() -> Vec<String> {
    let mut all: Vec<HWND> = Vec::new();
    let _ = EnumWindows(Some(collect), LPARAM(&mut all as *mut _ as isize));
    let mut out: Vec<String> = app_windows()
        .into_iter()
        .map(|h| format!("{:?} [{}] « {} » aumid={:?} exe={:?}", h.0, class_name(h), title(h), window_aumid(h), window_exe(h)))
        .collect();
    for h in all.into_iter().filter(|&h| class_name(h) == "ApplicationFrameWindow") {
        let mut cloaked = 0u32;
        let _ = DwmGetWindowAttribute(h, DWMWA_CLOAKED, &mut cloaked as *mut _ as *mut c_void, 4);
        let owner = GetWindow(h, GW_OWNER).map(|o| !o.is_invalid()).unwrap_or(false);
        let ex = GetWindowLongPtrW(h, GWL_EXSTYLE) as u32;
        out.push(format!(
            "UWP {:?} « {} » visible={} cloaked={} owner={} toolwindow={} aumid={:?}",
            h.0,
            title(h),
            IsWindowVisible(h).as_bool(),
            cloaked,
            owner,
            ex & WS_EX_TOOLWINDOW.0 != 0,
            window_aumid(h)
        ));
    }
    out
}

/// Fenêtre existante correspondant à l'élément, s'il y en a une.
pub unsafe fn find(item: &Item) -> Option<HWND> {
    let wins = app_windows();

    if item.key == "dir:explorer" {
        return wins.into_iter().find(|&h| class_name(h) == "CabinetWClass");
    }
    if item.key.starts_with("dir:") {
        let target = norm_path(item.path.as_deref()?);
        let tabs = explorer_tabs();
        let name = fold(&item.name);
        // Le titre de la fenêtre est celui de l'onglet actif : on évite de ramener une fenêtre
        // dont le dossier voulu est dans un onglet en arrière-plan.
        return wins.into_iter().find(|&h| {
            class_name(h) == "CabinetWClass"
                && tabs.iter().any(|(t, p)| *t == h.0 as isize && *p == target)
                && fold(&title(h)).starts_with(&name)
        });
    }

    let aumid = item.key.strip_prefix("app:")?;
    let exe = item
        .path
        .as_deref()
        .filter(|p| p.to_lowercase().ends_with(".exe"))
        .filter(|p| {
            let file = p.rsplit('\\').next().unwrap_or("").to_lowercase();
            !SHARED_HOSTS.contains(&file.as_str())
        })
        .map(norm_path);
    // « Famille!App » pour les apps empaquetées.
    let family = aumid.split_once('!').map(|(f, _)| f);
    wins.into_iter().find(|&h| {
        if window_aumid(h).is_some_and(|a| a.eq_ignore_ascii_case(aumid)) {
            return true;
        }
        if let Some(f) = family {
            if window_package_family(h).is_some_and(|p| p.eq_ignore_ascii_case(f)) {
                return true;
            }
        }
        match (&exe, window_exe(h)) {
            (Some(e), Some(w)) => norm_path(&w) == *e,
            _ => false,
        }
    })
}

/// Ramène la fenêtre au premier plan, en la restaurant si elle est réduite.
pub unsafe fn activate(h: HWND) {
    if IsIconic(h).as_bool() {
        let _ = ShowWindow(h, SW_RESTORE);
    }
    if !SetForegroundWindow(h).as_bool() {
        SwitchToThisWindow(h, true);
    }
}

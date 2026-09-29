#![windows_subsystem = "windows"]
//! Installateur de Wayne : un seul .exe qui embarque wayne.exe.
//!
//! Installation par utilisateur (aucun droit admin) dans %LOCALAPPDATA%\Programs\Wayne :
//! raccourci du menu Démarrer, entrée dans « Applications installées », démarrage
//! automatique facultatif. Le même binaire, copié en uninstall.exe, sert à désinstaller.
//!
//!   Wayne-Setup.exe               installation interactive
//!   Wayne-Setup.exe /S            installation silencieuse (démarrage automatique activé)
//!   uninstall.exe /uninstall [/S] désinstallation

use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;

use windows::core::{w, Interface, HSTRING};
use windows::Win32::Foundation::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::Diagnostics::ToolHelp::*;
use windows::Win32::System::Registry::*;
use windows::Win32::System::Threading::*;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

static PAYLOAD: &[u8] = include_bytes!("../../target/release/wayne.exe");
const VERSION: &str = env!("CARGO_PKG_VERSION");
const PUBLISHER: &str = "Kortexs";
const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\Wayne";
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

type R<T> = Result<T, String>;

fn env_dir(var: &str) -> R<PathBuf> {
    std::env::var_os(var).map(PathBuf::from).ok_or_else(|| format!("variable {var} introuvable"))
}
fn install_dir() -> R<PathBuf> {
    Ok(env_dir("LOCALAPPDATA")?.join("Programs").join("Wayne"))
}
fn shortcut_path() -> R<PathBuf> {
    Ok(env_dir("APPDATA")?.join(r"Microsoft\Windows\Start Menu\Programs\Wayne.lnk"))
}
fn h(s: impl AsRef<str>) -> HSTRING {
    HSTRING::from(s.as_ref())
}
fn p(path: &std::path::Path) -> String {
    path.display().to_string()
}

fn message(text: &str, icon: MESSAGEBOX_STYLE) {
    unsafe {
        MessageBoxW(None, &h(text), w!("Wayne"), icon | MB_SETFOREGROUND);
    }
}
fn ask(text: &str) -> bool {
    unsafe { MessageBoxW(None, &h(text), w!("Wayne"), MB_YESNO | MB_ICONQUESTION | MB_SETFOREGROUND) == IDYES }
}

fn main() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
    let args: Vec<String> = std::env::args().skip(1).map(|a| a.to_lowercase()).collect();
    let silent = args.iter().any(|a| a == "/s" || a == "--silent");
    let result = if args.iter().any(|a| a == "/uninstall" || a == "--uninstall") { uninstall(silent) } else { install(silent) };
    if let Err(e) = result {
        if !silent {
            message(&format!("Une erreur est survenue :\n\n{e}"), MB_ICONERROR);
        }
        std::process::exit(1);
    }
}

fn install(silent: bool) -> R<()> {
    let dir = install_dir()?;
    if !silent
        && !ask(&format!(
            "Installer Wayne {VERSION} ?\n\nWayne s'ouvre avec Alt+Espace et lance tes applications et dossiers.\n\nDossier : {}\nAucun droit administrateur requis.",
            p(&dir)
        ))
    {
        return Ok(());
    }
    let autostart = silent || ask("Lancer Wayne automatiquement à l'ouverture de session ?\n\n(Recommandé : Alt+Espace est alors toujours disponible.)");

    kill_wayne();
    std::fs::create_dir_all(&dir).map_err(|e| format!("création de {} : {e}", p(&dir)))?;
    let exe = dir.join("wayne.exe");
    write_with_retry(&exe, PAYLOAD)?;
    let uninstaller = dir.join("uninstall.exe");
    let me = std::env::current_exe().map_err(|e| e.to_string())?;
    if me != uninstaller {
        std::fs::copy(&me, &uninstaller).map_err(|e| format!("copie du désinstallateur : {e}"))?;
    }

    create_shortcut(&exe, &shortcut_path()?)?;
    register_uninstall(&dir, &exe, &uninstaller)?;
    if autostart {
        reg_set_str(RUN_KEY, "Wayne", &format!("\"{}\" --hidden", p(&exe)))?;
    } else {
        reg_delete_value(RUN_KEY, "Wayne");
    }

    Command::new(&exe).arg("--hidden").spawn().map_err(|e| format!("lancement de Wayne : {e}"))?;
    if !silent {
        message("Wayne est installé et prêt.\n\nAppuie sur Alt+Espace pour l'ouvrir.", MB_ICONINFORMATION);
    }
    Ok(())
}

fn uninstall(silent: bool) -> R<()> {
    if !silent && !ask("Désinstaller Wayne ?") {
        return Ok(());
    }
    kill_wayne();
    let dir = install_dir()?;
    let _ = std::fs::remove_file(dir.join("wayne.exe"));
    let _ = std::fs::remove_file(shortcut_path()?);
    reg_delete_value(RUN_KEY, "Wayne");
    unsafe {
        let _ = RegDeleteTreeW(HKEY_CURRENT_USER, &h(UNINSTALL_KEY));
    }

    let data = env_dir("APPDATA")?.join("Wayne");
    if !silent && data.exists() && ask("Supprimer aussi l'historique d'apprentissage de Wayne (tes habitudes de lancement) ?") {
        let _ = std::fs::remove_dir_all(&data);
    }
    if !silent {
        message("Wayne a été désinstallé.", MB_ICONINFORMATION);
    }

    // uninstall.exe ne peut pas s'effacer lui-même : cmd attend notre sortie puis retire le dossier.
    let _ = Command::new("cmd.exe")
        .raw_arg(format!("/c ping -n 3 127.0.0.1 >nul & rmdir /s /q \"{}\"", p(&dir)))
        .current_dir(std::env::temp_dir())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();
    Ok(())
}

/// Ferme toute instance de Wayne en cours (mise à jour ou désinstallation).
fn kill_wayne() {
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return };
        let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut ok = Process32FirstW(snap, &mut e).is_ok();
        while ok {
            let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
            if String::from_utf16_lossy(&e.szExeFile[..len]).eq_ignore_ascii_case("wayne.exe") {
                if let Ok(hp) = OpenProcess(PROCESS_TERMINATE | PROCESS_SYNCHRONIZE, false, e.th32ProcessID) {
                    let _ = TerminateProcess(hp, 0);
                    WaitForSingleObject(hp, 3000);
                    let _ = CloseHandle(hp);
                }
            }
            ok = Process32NextW(snap, &mut e).is_ok();
        }
        let _ = CloseHandle(snap);
    }
}

fn write_with_retry(path: &std::path::Path, bytes: &[u8]) -> R<()> {
    let mut last = String::new();
    for _ in 0..20 {
        match std::fs::write(path, bytes) {
            Ok(()) => return Ok(()),
            Err(e) => last = e.to_string(),
        }
        std::thread::sleep(std::time::Duration::from_millis(150));
    }
    Err(format!("écriture de {} : {last}", p(path)))
}

fn create_shortcut(target: &std::path::Path, lnk: &std::path::Path) -> R<()> {
    unsafe {
        let run = || -> windows::core::Result<()> {
            let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
            link.SetPath(&h(p(target)))?;
            if let Some(d) = target.parent() {
                link.SetWorkingDirectory(&h(p(d)))?;
            }
            link.SetDescription(&h("Lanceur d'applications (Alt+Espace)"))?;
            link.SetIconLocation(&h(p(target)), 0)?;
            link.cast::<IPersistFile>()?.Save(&h(p(lnk)), true)
        };
        run().map_err(|e| format!("raccourci du menu Démarrer : {e}"))
    }
}

fn register_uninstall(dir: &std::path::Path, exe: &std::path::Path, uninstaller: &std::path::Path) -> R<()> {
    let un = p(uninstaller);
    reg_set_str(UNINSTALL_KEY, "DisplayName", "Wayne")?;
    reg_set_str(UNINSTALL_KEY, "DisplayVersion", VERSION)?;
    reg_set_str(UNINSTALL_KEY, "Publisher", PUBLISHER)?;
    reg_set_str(UNINSTALL_KEY, "DisplayIcon", &format!("{},0", p(exe)))?;
    reg_set_str(UNINSTALL_KEY, "InstallLocation", &p(dir))?;
    reg_set_str(UNINSTALL_KEY, "UninstallString", &format!("\"{un}\" /uninstall"))?;
    reg_set_str(UNINSTALL_KEY, "QuietUninstallString", &format!("\"{un}\" /uninstall /S"))?;
    let size_kb = std::fs::metadata(uninstaller).map(|m| m.len()).unwrap_or(0) / 1024 + PAYLOAD.len() as u64 / 1024;
    reg_set_dword(UNINSTALL_KEY, "EstimatedSize", size_kb as u32)?;
    reg_set_dword(UNINSTALL_KEY, "NoModify", 1)?;
    reg_set_dword(UNINSTALL_KEY, "NoRepair", 1)
}

fn reg_set_str(key: &str, name: &str, value: &str) -> R<()> {
    let data: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
    let err = unsafe { RegSetKeyValueW(HKEY_CURRENT_USER, &h(key), &h(name), REG_SZ.0, Some(data.as_ptr() as *const _), (data.len() * 2) as u32) };
    err.ok().map_err(|e| format!("registre {key}\\{name} : {e}"))
}

fn reg_set_dword(key: &str, name: &str, value: u32) -> R<()> {
    let err = unsafe { RegSetKeyValueW(HKEY_CURRENT_USER, &h(key), &h(name), REG_DWORD.0, Some(&value as *const u32 as *const _), 4) };
    err.ok().map_err(|e| format!("registre {key}\\{name} : {e}"))
}

fn reg_delete_value(key: &str, name: &str) {
    unsafe {
        let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, &h(key), &h(name));
    }
}

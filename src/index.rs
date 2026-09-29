//! Ce que Wayne peut lancer : les applications (dossier virtuel shell:AppsFolder, le même
//! que le menu Démarrer, apps classiques + Microsoft Store) et les dossiers de haut niveau.

use windows::core::{Interface, GUID, PCWSTR, PWSTR};
use windows::Win32::UI::Shell::PropertiesSystem::PROPERTYKEY;
use windows::Win32::System::Com::*;
use windows::Win32::System::Registry::*;
use windows::Win32::UI::Shell::*;

use crate::matcher::{fold, words};

#[derive(Clone)]
pub enum Action {
    Open(String),
    Quit,
    ToggleAutostart,
}

#[derive(Clone)]
pub struct Item {
    pub key: String,
    pub name: String,
    pub sub: String,
    pub action: Action,
    /// Chemin réel sur disque (pour Ctrl+Entrée = afficher dans l'Explorateur).
    pub path: Option<String>,
    /// Nom d'analyse shell utilisé pour charger l'icône.
    pub icon: Option<String>,
    pub fname: String,
    pub words: Vec<String>,
    pub initials: String,
    pub aliases: Vec<String>,
}

impl Item {
    fn new(key: String, name: String, sub: String, action: Action) -> Item {
        let ws = words(&name);
        let initials = ws.iter().filter_map(|w| w.chars().next()).collect();
        Item { key, fname: fold(&name), words: ws, initials, name, sub, action, path: None, icon: None, aliases: Vec::new() }
    }
    fn aliases(mut self, a: &[&str]) -> Item {
        self.aliases = a.iter().map(|s| fold(s)).collect();
        self
    }
}

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

unsafe fn take(p: PWSTR) -> String {
    let s = p.to_string().unwrap_or_default();
    CoTaskMemFree(Some(p.0 as *const _));
    s
}

unsafe fn known_folder(id: &GUID) -> Option<String> {
    SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).ok().map(|p| take(p))
}

/// Dossiers de haut niveau + commandes internes. Instantané (aucune énumération).
pub fn builtins() -> Vec<Item> {
    let mut v = Vec::new();
    let explorer = Item::new("dir:explorer".into(), "Explorateur de fichiers".into(), "Explorateur Windows".into(), Action::Open("explorer.exe".into()))
        .aliases(&["f", "file explorer", "explorer", "explorateur"]);
    v.push(Item { icon: Some("shell:AppsFolder\\Microsoft.Windows.Explorer".into()), ..explorer });

    let dirs: [(&GUID, &str, &str, &[&str]); 7] = [
        (&FOLDERID_Pictures, "images", "Images", &["i", "pictures", "photos"]),
        (&FOLDERID_Downloads, "downloads", "Téléchargements", &["d", "downloads"]),
        (&FOLDERID_Documents, "documents", "Documents", &["documents", "docs"]),
        (&FOLDERID_Desktop, "desktop", "Bureau", &["b", "desktop"]),
        (&FOLDERID_Music, "music", "Musique", &["m", "music"]),
        (&FOLDERID_Videos, "videos", "Vidéos", &["v", "videos", "movies"]),
        (&FOLDERID_Profile, "home", "Dossier personnel", &["h", "~", "home", "maison"]),
    ];
    for (id, key, name, aliases) in dirs {
        if let Some(path) = unsafe { known_folder(id) } {
            let it = Item::new(format!("dir:{key}"), name.into(), path.clone(), Action::Open(path.clone())).aliases(aliases);
            v.push(Item { icon: Some(path.clone()), path: Some(path), ..it });
        }
    }

    let (label, sub) = if autostart_enabled() {
        ("Ne plus lancer Wayne au démarrage", "Commande Wayne · retire Wayne du démarrage de Windows")
    } else {
        ("Lancer Wayne au démarrage de Windows", "Commande Wayne · Alt+Espace disponible dès l'ouverture de session")
    };
    // Les commandes portent l'icône de wayne.exe lui-même.
    let own_icon = std::env::current_exe().ok().map(|p| p.display().to_string());
    let autostart = Item::new("cmd:autostart".into(), label.into(), sub.into(), Action::ToggleAutostart).aliases(&["startup", "demarrage", "autostart"]);
    let quit = Item::new("cmd:quit".into(), "Quitter Wayne".into(), "Commande Wayne · ferme le lanceur".into(), Action::Quit).aliases(&["quit", "exit"]);
    v.push(Item { icon: own_icon.clone(), ..autostart });
    v.push(Item { icon: own_icon, ..quit });
    v
}

// System.Link.TargetParsingPath
const PKEY_LINK_TARGET: PROPERTYKEY = PROPERTYKEY { fmtid: GUID::from_u128(0xB9B4B3FC_2B51_4A42_B5D8_324146AFCF25), pid: 2 };

/// « {GUID-de-dossier-connu}\sous\chemin.exe » → « C:\…\sous\chemin.exe »
unsafe fn resolve_guid_path(parsing: &str) -> Option<String> {
    if !parsing.starts_with('{') {
        return None;
    }
    let end = parsing.find('}')?;
    let guid = CLSIDFromString(PCWSTR(wide(&parsing[..=end]).as_ptr())).ok()?;
    let base = known_folder(&guid)?;
    Some(format!("{}{}", base, &parsing[end + 1..]))
}

fn is_junk(name: &str, target: &str) -> bool {
    let n = fold(name);
    let t = target.to_ascii_lowercase();
    n.starts_with("uninstall") || n.starts_with("desinstall") || t.contains("unins0") || t.ends_with("uninstall.exe")
}

/// Énumération complète (thread d'arrière-plan). ~100–300 ms.
pub fn scan() -> Vec<Item> {
    let mut out = builtins();
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        if let Err(e) = scan_apps(&mut out) {
            let _ = e;
        }
        CoUninitialize();
    }
    out
}

unsafe fn scan_apps(out: &mut Vec<Item>) -> windows::core::Result<()> {
    let folder: IShellItem = SHGetKnownFolderItem(&FOLDERID_AppsFolder, KF_FLAG_DEFAULT, None)?;
    let en: IEnumShellItems = folder.BindToHandler(None, &BHID_EnumItems)?;
    loop {
        let mut buf = [None];
        let mut n = 0u32;
        if en.Next(&mut buf, Some(&mut n)).is_err() || n == 0 {
            break;
        }
        let Some(it) = buf[0].take() else { break };
        let Ok(name) = it.GetDisplayName(SIGDN_NORMALDISPLAY).map(|p| take(p)) else { continue };
        let Ok(parsing) = it.GetDisplayName(SIGDN_PARENTRELATIVEPARSING).map(|p| take(p)) else { continue };
        if parsing == "Microsoft.Windows.Explorer" || name.trim().is_empty() {
            continue;
        }
        let target = it
            .cast::<IShellItem2>()
            .ok()
            .and_then(|i2| i2.GetString(&PKEY_LINK_TARGET).ok())
            .map(|p| take(p))
            .filter(|s| !s.is_empty())
            .or_else(|| resolve_guid_path(&parsing))
            .or_else(|| (parsing.len() > 2 && parsing.as_bytes()[1] == b':').then(|| parsing.clone()));
        if is_junk(&name, target.as_deref().unwrap_or("")) {
            continue;
        }
        let sub = match &target {
            Some(t) => t.clone(),
            None if parsing.starts_with("http") => parsing.clone(),
            None if parsing.contains('!') => "Application Microsoft Store".into(),
            None => parsing.clone(),
        };
        let shell = format!("shell:AppsFolder\\{parsing}");
        let item = Item::new(format!("app:{parsing}"), name, sub, Action::Open(shell.clone()));
        out.push(Item { icon: Some(shell), path: target.filter(|t| std::path::Path::new(t).exists()), ..item });
    }
    Ok(())
}

// ---- Démarrage automatique (HKCU\…\Run) ----

const RUN_KEY: PCWSTR = windows::core::w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const RUN_VALUE: PCWSTR = windows::core::w!("Wayne");

pub fn autostart_enabled() -> bool {
    unsafe { RegGetValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_VALUE, RRF_RT_REG_SZ, None, None, None).is_ok() }
}

pub fn set_autostart(on: bool) {
    unsafe {
        if on {
            let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default();
            let v = wide(&format!("\"{exe}\" --hidden"));
            let _ = RegSetKeyValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_VALUE, REG_SZ.0, Some(v.as_ptr() as *const _), (v.len() * 2) as u32);
        } else {
            let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_VALUE);
        }
    }
}

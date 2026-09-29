//! stamp <exe> <ico> <version> <description> <nom-de-fichier-original>
//!
//! Insère dans un exécutable déjà compilé l'icône (RT_ICON + RT_GROUP_ICON) et le bloc
//! VERSIONINFO, via l'API Windows UpdateResource : aucun compilateur de ressources requis.

use windows::core::{HSTRING, PCWSTR};
use windows::Win32::System::LibraryLoader::{BeginUpdateResourceW, EndUpdateResourceW, UpdateResourceW};

const RT_ICON: u16 = 3;
const RT_GROUP_ICON: u16 = 14;
const RT_VERSION: u16 = 16;
const LANG: u16 = 0x0C0C; // français (Canada)
const CODEPAGE: u16 = 1200; // Unicode

const COMPANY: &str = "Kortexs";
const PRODUCT: &str = "Wayne";
const COPYRIGHT: &str = "© 2026 Kortexs";

fn res_id(id: u16) -> PCWSTR {
    PCWSTR(id as usize as *const u16)
}

fn u16le(v: &mut Vec<u8>, x: u16) {
    v.extend_from_slice(&x.to_le_bytes());
}
fn u32le(v: &mut Vec<u8>, x: u32) {
    v.extend_from_slice(&x.to_le_bytes());
}
fn wstr(v: &mut Vec<u8>, s: &str) {
    for c in s.encode_utf16().chain(std::iter::once(0)) {
        u16le(v, c);
    }
}
fn align4(v: &mut Vec<u8>) {
    while v.len() % 4 != 0 {
        v.push(0);
    }
}

/// Bloc générique VS_VERSIONINFO : wLength, wValueLength, wType, szKey, padding, valeur, padding, enfants.
fn block(key: &str, value: &[u8], value_len: u16, text: bool, children: &[Vec<u8>]) -> Vec<u8> {
    let mut v = vec![0, 0];
    u16le(&mut v, value_len);
    u16le(&mut v, text as u16);
    wstr(&mut v, key);
    align4(&mut v);
    v.extend_from_slice(value);
    for c in children {
        align4(&mut v);
        v.extend_from_slice(c);
    }
    let len = v.len() as u16;
    v[0..2].copy_from_slice(&len.to_le_bytes());
    v
}

fn string_entry(key: &str, val: &str) -> Vec<u8> {
    let mut data = Vec::new();
    wstr(&mut data, val);
    block(key, &data, (val.encode_utf16().count() + 1) as u16, true, &[])
}

fn version_info(version: &str, description: &str, original: &str) -> Vec<u8> {
    let mut parts: Vec<u16> = version.split('.').map(|p| p.parse().unwrap_or(0)).collect();
    parts.resize(4, 0);
    let ms = (parts[0] as u32) << 16 | parts[1] as u32;
    let ls = (parts[2] as u32) << 16 | parts[3] as u32;

    let mut ffi = Vec::new();
    for x in [0xFEEF04BDu32, 0x0001_0000, ms, ls, ms, ls, 0x3F, 0, 0x0004_0004 /* VOS_NT_WINDOWS32 */, 1 /* VFT_APP */, 0, 0, 0] {
        u32le(&mut ffi, x);
    }

    let strings = [
        ("CompanyName", COMPANY),
        ("FileDescription", description),
        ("FileVersion", version),
        ("InternalName", original.trim_end_matches(".exe")),
        ("LegalCopyright", COPYRIGHT),
        ("OriginalFilename", original),
        ("ProductName", PRODUCT),
        ("ProductVersion", version),
    ];
    let entries: Vec<Vec<u8>> = strings.iter().map(|(k, v)| string_entry(k, v)).collect();
    let table = block(&format!("{LANG:04X}{CODEPAGE:04X}"), &[], 0, true, &entries);
    let sfi = block("StringFileInfo", &[], 0, true, &[table]);

    let mut trans = Vec::new();
    u16le(&mut trans, LANG);
    u16le(&mut trans, CODEPAGE);
    let var = block("Translation", &trans, 4, false, &[]);
    let vfi = block("VarFileInfo", &[], 0, true, &[var]);

    block("VS_VERSION_INFO", &ffi, ffi.len() as u16, false, &[sfi, vfi])
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() != 6 {
        eprintln!("usage : stamp <exe> <ico> <version> <description> <nom-original>");
        std::process::exit(2);
    }
    let ico = std::fs::read(&a[2]).expect("lecture de l'icône");
    let count = u16::from_le_bytes([ico[4], ico[5]]) as usize;

    unsafe {
        let h = BeginUpdateResourceW(&HSTRING::from(a[1].as_str()), false).expect("BeginUpdateResource");
        // Chaque image du .ico devient une ressource RT_ICON ; le répertoire devient RT_GROUP_ICON.
        let mut group = Vec::new();
        u16le(&mut group, 0);
        u16le(&mut group, 1);
        u16le(&mut group, count as u16);
        for i in 0..count {
            let e = &ico[6 + 16 * i..6 + 16 * (i + 1)];
            let size = u32::from_le_bytes([e[8], e[9], e[10], e[11]]) as usize;
            let off = u32::from_le_bytes([e[12], e[13], e[14], e[15]]) as usize;
            let img = &ico[off..off + size];
            let id = (i + 1) as u16;
            UpdateResourceW(h, res_id(RT_ICON), res_id(id), LANG, Some(img.as_ptr() as *const _), img.len() as u32).expect("RT_ICON");
            group.extend_from_slice(&e[..12]);
            u16le(&mut group, id);
        }
        UpdateResourceW(h, res_id(RT_GROUP_ICON), res_id(1), LANG, Some(group.as_ptr() as *const _), group.len() as u32).expect("RT_GROUP_ICON");
        let vi = version_info(&a[3], &a[4], &a[5]);
        UpdateResourceW(h, res_id(RT_VERSION), res_id(1), LANG, Some(vi.as_ptr() as *const _), vi.len() as u32).expect("RT_VERSION");
        EndUpdateResourceW(h, false).expect("EndUpdateResource");
    }
    println!("{} : icône + version {} insérées", a[1], a[3]);
}

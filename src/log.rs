//! Journal de diagnostic : %APPDATA%\Wayne\wayne.log (≈ 256 Ko max, puis remis à zéro).

use std::io::Write;
use std::path::PathBuf;

const MAX_BYTES: u64 = 256 * 1024;

fn path() -> PathBuf {
    std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(".")).join("Wayne").join("wayne.log")
}

pub fn log(msg: &str) {
    let p = path();
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let truncate = std::fs::metadata(&p).map(|m| m.len() > MAX_BYTES).unwrap_or(false);
    let file = std::fs::OpenOptions::new().create(true).write(true).append(!truncate).truncate(truncate).open(&p);
    if let Ok(mut f) = file {
        let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
        let _ = writeln!(
            f,
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02} [{}] {msg}",
            t.wYear,
            t.wMonth,
            t.wDay,
            t.wHour,
            t.wMinute,
            t.wSecond,
            std::process::id()
        );
    }
}

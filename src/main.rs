#![windows_subsystem = "windows"]
//! Wayne — lanceur façon Alfred pour Windows. Alt+Espace.

mod brain;
mod icons;
mod index;
mod log;
mod matcher;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::{Duration, Instant};

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Graphics::GdiPlus::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::DataExchange::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::*;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use brain::Brain;
use icons::Icon;
use index::{wide, Action, Item};

const CLASS: PCWSTR = w!("WayneWindow");
const WM_APP_INDEX: u32 = WM_APP + 1;
const WM_APP_ICON: u32 = WM_APP + 2;
const WM_APP_SHOW: u32 = WM_APP + 3;
const WM_APP_TRAY: u32 = WM_APP + 4;
const TRAY_ID: u32 = 1;
const MENU_OPEN: usize = 1;
const MENU_AUTOSTART: usize = 2;
const MENU_QUIT: usize = 3;
const HOTKEY_ID: i32 = 1;
const CARET_TIMER: usize = 1;
const MAX_ROWS: usize = 6;
const REINDEX_AFTER: Duration = Duration::from_secs(120);

// Mise en page (pixels logiques à 100 %)
const WIDTH: f32 = 720.0;
const PAD: f32 = 12.0;
const INPUT_H: f32 = 62.0;
const LOGO: f32 = 50.0;
const GAP: f32 = 10.0;
const ROW_H: f32 = 54.0;
const ICON: f32 = 36.0;

// Couleurs (r, g, b)
const BG: (u8, u8, u8) = (0x26, 0x26, 0x26);
const INPUT_BG: (u8, u8, u8) = (0x1C, 0x1C, 0x1C);
const SEL: (u8, u8, u8) = (0x35, 0x7F, 0x87);
const TITLE: (u8, u8, u8) = (0xF2, 0xF2, 0xF2);
const SUB: (u8, u8, u8) = (0x9A, 0x9A, 0x9A);
const SUB_SEL: (u8, u8, u8) = (0xD5, 0xE9, 0xEB);
const HINT: (u8, u8, u8) = (0x80, 0x80, 0x80);
const PLACEHOLDER: (u8, u8, u8) = (0x55, 0x55, 0x55);

fn cref(c: (u8, u8, u8)) -> COLORREF {
    COLORREF(c.0 as u32 | (c.1 as u32) << 8 | (c.2 as u32) << 16)
}
fn argb(c: (u8, u8, u8)) -> u32 {
    0xFF00_0000 | (c.0 as u32) << 16 | (c.1 as u32) << 8 | c.2 as u32
}

struct Fonts {
    query: HFONT,
    title: HFONT,
    sub: HFONT,
    hint: HFONT,
    symbol: HFONT,
    logo: HFONT,
    glyph: HFONT,
}

unsafe fn font(px: f32, weight: i32, face: &str) -> HFONT {
    let mut lf = LOGFONTW { lfHeight: -(px.round() as i32), lfWeight: weight, lfQuality: CLEARTYPE_QUALITY, ..Default::default() };
    for (i, c) in face.encode_utf16().take(31).enumerate() {
        lf.lfFaceName[i] = c;
    }
    CreateFontIndirectW(&lf)
}

impl Fonts {
    unsafe fn new(s: f32) -> Fonts {
        Fonts {
            query: font(30.0 * s, 300, "Segoe UI"),
            title: font(19.0 * s, 400, "Segoe UI"),
            sub: font(13.0 * s, 400, "Segoe UI"),
            hint: font(15.0 * s, 400, "Segoe UI"),
            symbol: font(20.0 * s, 400, "Segoe UI Symbol"),
            logo: font(28.0 * s, 700, "Segoe UI"),
            glyph: font(18.0 * s, 600, "Segoe UI"),
        }
    }
    unsafe fn free(&self) {
        for f in [self.query, self.title, self.sub, self.hint, self.symbol, self.logo, self.glyph] {
            let _ = DeleteObject(HGDIOBJ(f.0));
        }
    }
}

/// Ce que le gestionnaire de messages doit faire une fois l'état relâché (évite la réentrance).
enum Act {
    None,
    Hide,
    Launch(Item, bool),
}

struct App {
    hwnd: HWND,
    items: Vec<Item>,
    by_key: HashMap<String, usize>,
    results: Vec<Item>,
    query: String,
    sel: usize,
    brain: Brain,
    icons: HashMap<String, Option<Icon>>,
    icon_tx: Sender<icons::Request>,
    icon_rx: Receiver<icons::Reply>,
    index_tx: Sender<Vec<Item>>,
    index_rx: Receiver<Vec<Item>>,
    indexing: bool,
    last_index: Instant,
    scale: f32,
    fonts: Fonts,
    anchor: (i32, i32),
    visible: bool,
    caret_on: bool,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
    static LAST_HIDE: Cell<Option<Instant>> = const { Cell::new(None) };
    static TASKBAR_CREATED: Cell<u32> = const { Cell::new(0) };
}

fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|a| a.try_borrow_mut().ok().and_then(|mut g| g.as_mut().map(f)))
}

impl App {
    fn px(&self, v: f32) -> i32 {
        (v * self.scale).round() as i32
    }

    fn set_items(&mut self, items: Vec<Item>) {
        self.by_key = items.iter().enumerate().map(|(i, it)| (it.key.clone(), i)).collect();
        self.items = items;
        // Préchargement de toutes les icônes en arrière-plan.
        let px = self.px(ICON);
        for it in &self.items {
            if let Some(src) = &it.icon {
                if !self.icons.contains_key(&it.key) {
                    self.icons.insert(it.key.clone(), None);
                    let _ = self.icon_tx.send((it.key.clone(), src.clone(), px));
                }
            }
        }
    }

    fn spawn_index(&mut self) {
        if self.indexing {
            return;
        }
        self.indexing = true;
        let tx = self.index_tx.clone();
        let hwnd = self.hwnd.0 as isize;
        std::thread::spawn(move || {
            let items = index::scan();
            if tx.send(items).is_ok() {
                unsafe {
                    let _ = PostMessageW(HWND(hwnd as *mut c_void), WM_APP_INDEX, WPARAM(0), LPARAM(0));
                }
            }
        });
    }

    fn folded_query(&self) -> String {
        matcher::fold(self.query.trim())
    }

    fn refresh(&mut self) {
        let q = self.folded_query();
        self.results.clear();
        self.sel = 0;
        if q.is_empty() {
            // Requête vide : prédiction du modèle.
            for key in self.brain.predictions(MAX_ROWS) {
                if let Some(&i) = self.by_key.get(&key) {
                    self.results.push(self.items[i].clone());
                }
            }
        } else {
            let (hour, weekend) = brain::local_context();
            let mut scored: Vec<(f32, usize)> = self
                .items
                .iter()
                .enumerate()
                .filter_map(|(i, it)| matcher::score(&q, it).map(|s| (s + self.brain.boost(&q, &it.key, hour, weekend), i)))
                .collect();
            scored.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| self.items[a.1].name.len().cmp(&self.items[b.1].name.len())));
            self.results = scored.into_iter().take(MAX_ROWS).map(|(_, i)| self.items[i].clone()).collect();
        }
        self.resize();
    }

    fn height(&self) -> i32 {
        let n = self.results.len() as f32;
        let rows = if n > 0.0 { GAP + n * ROW_H } else { 0.0 };
        self.px(PAD + INPUT_H + rows + PAD)
    }

    fn resize(&self) {
        unsafe {
            let _ = SetWindowPos(self.hwnd, None, self.anchor.0, self.anchor.1, self.px(WIDTH), self.height(), SWP_NOZORDER | SWP_NOACTIVATE);
            let _ = InvalidateRect(self.hwnd, None, false);
        }
    }

    /// Positionne la fenêtre sur l'écran du curseur, adapte le DPI, réinitialise la requête.
    unsafe fn prepare_show(&mut self) {
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let mon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(mon, &mut mi);
        let (mut dx, mut dy) = (96u32, 96u32);
        let _ = GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        let scale = dx as f32 / 96.0;
        if (scale - self.scale).abs() > 0.01 {
            self.scale = scale;
            self.fonts.free();
            self.fonts = Fonts::new(scale);
            for icon in self.icons.values().flatten() {
                icon.free();
            }
            self.icons.clear();
            let items = std::mem::take(&mut self.items);
            self.set_items(items);
        }
        let wa = mi.rcWork;
        let w = self.px(WIDTH);
        self.anchor = (wa.left + (wa.right - wa.left - w) / 2, wa.top + (wa.bottom - wa.top) * 22 / 100);
        self.query.clear();
        self.refresh();
        if self.last_index.elapsed() > REINDEX_AFTER {
            self.spawn_index();
        }
    }

    fn launch(&mut self, idx: usize, reveal: bool) -> Act {
        let Some(item) = self.results.get(idx).cloned() else { return Act::None };
        let q = self.folded_query();
        self.brain.record(&item.key, &q);
        Act::Launch(item, reveal)
    }

    unsafe fn key_down(&mut self, vk: u32) -> Act {
        let ctrl = GetKeyState(VK_CONTROL.0 as i32) < 0;
        let n = self.results.len();
        match VIRTUAL_KEY(vk as u16) {
            VK_ESCAPE => return Act::Hide,
            VK_DOWN if n > 0 => self.sel = (self.sel + 1) % n,
            VK_UP if n > 0 => self.sel = (self.sel + n - 1) % n,
            VK_RETURN => return self.launch(self.sel, ctrl),
            VK_TAB => {
                if let Some(it) = self.results.get(self.sel) {
                    self.query = it.name.clone();
                    self.refresh();
                }
            }
            k if ctrl && (0x31..=0x39).contains(&k.0) => return self.launch((k.0 - 0x31) as usize, false),
            _ => return Act::None,
        }
        let _ = InvalidateRect(self.hwnd, None, false);
        Act::None
    }

    unsafe fn on_char(&mut self, c: u32) {
        match c {
            0x08 => {
                self.query.pop();
            }
            0x7F => {
                // Ctrl+Retour arrière : efface le dernier mot
                let t = self.query.trim_end().len();
                self.query.truncate(t);
                let cut = self.query.rfind(' ').map(|i| i + 1).unwrap_or(0);
                self.query.truncate(cut);
            }
            0x16 => {
                if let Some(t) = clipboard_text(self.hwnd) {
                    self.query.push_str(t.lines().next().unwrap_or(""));
                }
            }
            c if c >= 0x20 => {
                if let Some(ch) = char::from_u32(c) {
                    self.query.push(ch);
                }
            }
            _ => return,
        }
        self.caret_on = true;
        self.refresh();
    }

    fn click(&mut self, x: i32, y: i32) -> Act {
        let top = self.px(PAD + INPUT_H + GAP);
        if y < top || x < self.px(PAD) || x > self.px(WIDTH - PAD) {
            return Act::None;
        }
        let idx = ((y - top) / self.px(ROW_H)) as usize;
        if idx < self.results.len() {
            self.sel = idx;
            return self.launch(idx, false);
        }
        Act::None
    }

    unsafe fn paint(&mut self, hdc: HDC) {
        let mut rc = RECT::default();
        let _ = GetClientRect(self.hwnd, &mut rc);
        let (w, h) = (rc.right, rc.bottom);
        let mdc = CreateCompatibleDC(hdc);
        let bmp = CreateCompatibleBitmap(hdc, w, h);
        let old_bmp = SelectObject(mdc, HGDIOBJ(bmp.0));

        let bg = CreateSolidBrush(cref(BG));
        FillRect(mdc, &rc, bg);
        let _ = DeleteObject(HGDIOBJ(bg.0));

        let s = self.scale;
        let input_r = (PAD * s, PAD * s, (WIDTH - 2.0 * PAD - LOGO - 12.0) * s, INPUT_H * s);
        let logo_x = (WIDTH - PAD - LOGO) * s;
        let logo_y = (PAD + (INPUT_H - LOGO) / 2.0) * s;
        let rows_top = PAD + INPUT_H + GAP;

        // Formes lissées (GDI+)
        {
            let g = Gfx::new(mdc);
            g.round(input_r.0, input_r.1, input_r.2, input_r.3, 10.0 * s, argb(INPUT_BG));
            g.round(logo_x, logo_y, LOGO * s, LOGO * s, 13.0 * s, argb(SEL));
            if !self.results.is_empty() {
                let y = (rows_top + self.sel as f32 * ROW_H) * s;
                g.round(PAD * s, y, (WIDTH - 2.0 * PAD) * s, ROW_H * s, 10.0 * s, argb(SEL));
            }
            for (i, it) in self.results.iter().enumerate() {
                if !matches!(self.icons.get(&it.key), Some(Some(_))) {
                    let (ix, iy) = self.icon_pos(i);
                    g.round(ix as f32, iy as f32, ICON * s, ICON * s, 8.0 * s, argb(placeholder_color(&it.key)));
                }
            }
        }

        SetBkMode(mdc, TRANSPARENT);

        // Requête + curseur
        let text_left = (input_r.0 + 16.0 * s) as i32;
        let text_right = (input_r.0 + input_r.2 - 12.0 * s) as i32;
        let ir = RECT { left: text_left, top: input_r.1 as i32, right: text_right, bottom: (input_r.1 + input_r.3) as i32 };
        SelectObject(mdc, HGDIOBJ(self.fonts.query.0));
        if self.query.is_empty() {
            text(mdc, "Wayne", self.fonts.query, PLACEHOLDER, ir, DT_LEFT | DT_VCENTER);
        } else {
            // Si trop long, on n'affiche que la fin.
            let mut shown: &str = &self.query;
            while text_width(mdc, shown) > text_right - text_left - self.px(4.0) && !shown.is_empty() {
                let mut it = shown.char_indices();
                it.next();
                shown = it.next().map(|(i, _)| &shown[i..]).unwrap_or("");
            }
            text(mdc, shown, self.fonts.query, TITLE, ir, DT_LEFT | DT_VCENTER);
        }
        if self.caret_on {
            let tw = if self.query.is_empty() { 0 } else { text_width(mdc, &self.query).min(text_right - text_left - self.px(4.0)) };
            let ch = self.px(32.0);
            let cy = (input_r.1 + (input_r.3 - ch as f32) / 2.0) as i32;
            let caret = RECT { left: text_left + tw + self.px(1.0), top: cy, right: text_left + tw + self.px(3.0), bottom: cy + ch };
            let br = CreateSolidBrush(cref(TITLE));
            FillRect(mdc, &caret, br);
            let _ = DeleteObject(HGDIOBJ(br.0));
        }

        // Logo
        let lr = RECT { left: logo_x as i32, top: logo_y as i32, right: (logo_x + LOGO * s) as i32, bottom: (logo_y + LOGO * s) as i32 };
        text(mdc, "W", self.fonts.logo, (0xFF, 0xFF, 0xFF), lr, DT_CENTER | DT_VCENTER);

        // Lignes de résultats
        let icon_px = self.px(ICON);
        for (i, it) in self.results.iter().enumerate() {
            let selected = i == self.sel;
            let ry = self.px(rows_top + i as f32 * ROW_H);
            let (ix, iy) = self.icon_pos(i);
            match self.icons.get(&it.key) {
                Some(Some(icon)) => draw_icon(mdc, icon, ix, iy, icon_px),
                _ => {
                    let letter: String = it.name.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
                    let r = RECT { left: ix, top: iy, right: ix + icon_px, bottom: iy + icon_px };
                    text(mdc, &letter, self.fonts.glyph, (0xFF, 0xFF, 0xFF), r, DT_CENTER | DT_VCENTER);
                }
            }
            let tx = ix + icon_px + self.px(14.0);
            let right = self.px(WIDTH - PAD - 90.0);
            let tr = RECT { left: tx, top: ry + self.px(6.0), right, bottom: ry + self.px(31.0) };
            text(mdc, &it.name, self.fonts.title, TITLE, tr, DT_LEFT | DT_VCENTER | DT_END_ELLIPSIS);
            let sr = RECT { left: tx, top: ry + self.px(31.0), right, bottom: ry + self.px(49.0) };
            text(mdc, &it.sub, self.fonts.sub, if selected { SUB_SEL } else { SUB }, sr, DT_LEFT | DT_VCENTER | DT_PATH_ELLIPSIS);
            let kr = RECT { left: self.px(WIDTH - PAD - 90.0), top: ry, right: self.px(WIDTH - PAD - 16.0), bottom: ry + self.px(ROW_H) };
            if selected {
                text(mdc, "\u{21B5}", self.fonts.symbol, (0xFF, 0xFF, 0xFF), kr, DT_RIGHT | DT_VCENTER);
            } else {
                text(mdc, &format!("Ctrl+{}", i + 1), self.fonts.hint, HINT, kr, DT_RIGHT | DT_VCENTER);
            }
        }

        let _ = BitBlt(hdc, 0, 0, w, h, mdc, 0, 0, SRCCOPY);
        SelectObject(mdc, old_bmp);
        let _ = DeleteObject(HGDIOBJ(bmp.0));
        let _ = DeleteDC(mdc);
    }

    fn icon_pos(&self, i: usize) -> (i32, i32) {
        let ry = PAD + INPUT_H + GAP + i as f32 * ROW_H;
        (self.px(PAD + 12.0), self.px(ry + (ROW_H - ICON) / 2.0))
    }
}

fn placeholder_color(key: &str) -> (u8, u8, u8) {
    const P: [(u8, u8, u8); 6] = [(0x4A, 0x6F, 0xA5), (0x8E, 0x5B, 0xA8), (0x3E, 0x8E, 0x6B), (0xB0, 0x6A, 0x3B), (0x9C, 0x4B, 0x5E), (0x5A, 0x6A, 0x7A)];
    let h = key.bytes().fold(0u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32));
    P[h as usize % P.len()]
}

unsafe fn text(hdc: HDC, s: &str, f: HFONT, color: (u8, u8, u8), mut r: RECT, fmt: DRAW_TEXT_FORMAT) {
    SelectObject(hdc, HGDIOBJ(f.0));
    SetTextColor(hdc, cref(color));
    let mut w: Vec<u16> = s.encode_utf16().collect();
    DrawTextW(hdc, &mut w, &mut r, fmt | DT_SINGLELINE | DT_NOPREFIX);
}

unsafe fn text_width(hdc: HDC, s: &str) -> i32 {
    let w: Vec<u16> = s.encode_utf16().collect();
    let mut sz = SIZE::default();
    let _ = GetTextExtentPoint32W(hdc, &w, &mut sz);
    sz.cx
}

unsafe fn draw_icon(hdc: HDC, icon: &Icon, x: i32, y: i32, size: i32) {
    let src = CreateCompatibleDC(hdc);
    let old = SelectObject(src, HGDIOBJ(icon.bmp as *mut c_void));
    let bf = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
    let _ = AlphaBlend(hdc, x, y, size, size, src, 0, 0, icon.w, icon.h, bf);
    SelectObject(src, old);
    let _ = DeleteDC(src);
}

/// Petit enrobage GDI+ pour les rectangles arrondis anticrénelés.
struct Gfx(*mut GpGraphics);

impl Gfx {
    unsafe fn new(hdc: HDC) -> Gfx {
        let mut g = std::ptr::null_mut();
        GdipCreateFromHDC(hdc, &mut g);
        GdipSetSmoothingMode(g, SmoothingModeAntiAlias);
        GdipSetPixelOffsetMode(g, PixelOffsetModeHalf);
        Gfx(g)
    }
    unsafe fn round(&self, x: f32, y: f32, w: f32, h: f32, r: f32, color: u32) {
        let d = 2.0 * r;
        let mut path = std::ptr::null_mut();
        GdipCreatePath(FillModeAlternate, &mut path);
        GdipAddPathArc(path, x, y, d, d, 180.0, 90.0);
        GdipAddPathArc(path, x + w - d, y, d, d, 270.0, 90.0);
        GdipAddPathArc(path, x + w - d, y + h - d, d, d, 0.0, 90.0);
        GdipAddPathArc(path, x, y + h - d, d, d, 90.0, 90.0);
        GdipClosePathFigure(path);
        let mut brush = std::ptr::null_mut();
        GdipCreateSolidFill(color, &mut brush);
        GdipFillPath(self.0, brush as *mut GpBrush, path);
        GdipDeleteBrush(brush as *mut GpBrush);
        GdipDeletePath(path);
    }
}

impl Drop for Gfx {
    fn drop(&mut self) {
        unsafe {
            GdipDeleteGraphics(self.0);
        }
    }
}

unsafe fn clipboard_text(hwnd: HWND) -> Option<String> {
    OpenClipboard(hwnd).ok()?;
    let r = (|| {
        let h = GetClipboardData(13 /* CF_UNICODETEXT */).ok()?;
        let g = HGLOBAL(h.0);
        let p = GlobalLock(g) as *const u16;
        if p.is_null() {
            return None;
        }
        let mut n = 0;
        while *p.add(n) != 0 {
            n += 1;
        }
        let s = String::from_utf16_lossy(std::slice::from_raw_parts(p, n));
        let _ = GlobalUnlock(g);
        Some(s)
    })();
    let _ = CloseClipboard();
    r
}

// ---- Affichage / masquage / exécution (hors emprunt de l'état) ----

unsafe fn show(hwnd: HWND) {
    with_app(|a| a.prepare_show());
    let _ = SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW);
    let _ = SetForegroundWindow(hwnd);
    let _ = SetFocus(hwnd);
    with_app(|a| {
        a.visible = true;
        a.caret_on = true;
        SetTimer(hwnd, CARET_TIMER, 530, None);
    });
    let _ = InvalidateRect(hwnd, None, false);
}

/// État réel de la fenêtre : elle peut avoir été cachée sans nous (Win+D, etc.).
unsafe fn is_shown(hwnd: HWND) -> bool {
    let shown = IsWindowVisible(hwnd).as_bool();
    with_app(|a| a.visible = a.visible && shown);
    shown
}

unsafe fn hide(hwnd: HWND) {
    let was = with_app(|a| std::mem::replace(&mut a.visible, false)) == Some(true);
    if was || IsWindowVisible(hwnd).as_bool() {
        let _ = KillTimer(hwnd, CARET_TIMER);
        let _ = ShowWindow(hwnd, SW_HIDE);
        LAST_HIDE.with(|c| c.set(Some(Instant::now())));
    }
}

unsafe fn toggle_autostart() {
    index::set_autostart(!index::autostart_enabled());
    with_app(|a| {
        let mut items: Vec<Item> = a.items.drain(..).filter(|i| !i.key.starts_with("cmd:") && !i.key.starts_with("dir:")).collect();
        let mut all = index::builtins();
        all.append(&mut items);
        a.set_items(all);
    });
}

// ---- Icône de la zone de notification ----

unsafe fn tray(hwnd: HWND, op: NOTIFY_ICON_MESSAGE) {
    let hinst = GetModuleHandleW(None).unwrap_or_default();
    let size = GetSystemMetricsForDpi(SM_CXSMICON, GetDpiForSystem());
    // Ressource d'icône n° 1 insérée par tools/stamp ; icône générique en développement.
    let icon = LoadImageW(HINSTANCE(hinst.0), PCWSTR(1 as *const u16), IMAGE_ICON, size, size, LR_DEFAULTCOLOR)
        .map(|h| HICON(h.0))
        .unwrap_or_else(|_| LoadIconW(None, IDI_APPLICATION).unwrap_or_default());
    let mut nid = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: TRAY_ID,
        uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
        uCallbackMessage: WM_APP_TRAY,
        hIcon: icon,
        ..Default::default()
    };
    for (i, c) in "Wayne — Alt+Espace".encode_utf16().enumerate() {
        nid.szTip[i] = c;
    }
    let _ = Shell_NotifyIconW(op, &nid);
    let _ = DestroyIcon(icon);
}

unsafe fn tray_menu(hwnd: HWND) {
    let std::result::Result::Ok(menu) = CreatePopupMenu() else { return };
    let checked = if index::autostart_enabled() { MF_CHECKED } else { MF_UNCHECKED };
    let _ = AppendMenuW(menu, MF_STRING, MENU_OPEN, w!("Ouvrir Wayne\tAlt+Espace"));
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
    let _ = AppendMenuW(menu, MF_STRING | checked, MENU_AUTOSTART, w!("Lancer au démarrage de Windows"));
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
    let _ = AppendMenuW(menu, MF_STRING, MENU_QUIT, w!("Quitter Wayne"));
    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    // Nécessaire pour que le menu se ferme quand on clique ailleurs.
    let _ = SetForegroundWindow(hwnd);
    let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN, pt.x, pt.y, 0, hwnd, None);
    let _ = PostMessageW(hwnd, WM_NULL, WPARAM(0), LPARAM(0));
    let _ = DestroyMenu(menu);
    match cmd.0 as usize {
        MENU_OPEN => show(hwnd),
        MENU_AUTOSTART => toggle_autostart(),
        MENU_QUIT => {
            let _ = DestroyWindow(hwnd);
        }
        _ => {}
    }
}

unsafe fn perform(hwnd: HWND, act: Act) {
    match act {
        Act::None => {}
        Act::Hide => hide(hwnd),
        Act::Launch(item, reveal) => {
            hide(hwnd);
            match item.action {
                Action::Open(target) => {
                    if let (true, Some(p)) = (reveal, &item.path) {
                        let args = wide(&format!("/select,\"{p}\""));
                        ShellExecuteW(None, None, w!("explorer.exe"), PCWSTR(args.as_ptr()), None, SW_SHOWNORMAL);
                    } else {
                        // On délègue à l'Explorateur déjà ouvert, comme le menu Démarrer : l'app devient
                        // son enfant. Lancée directement par Wayne, elle hériterait d'un éventuel job
                        // Windows de Wayne, où Chrome, Brave ou Antidote se ferment aussitôt.
                        let args = if target.eq_ignore_ascii_case("explorer.exe") { String::new() } else { format!("\"{target}\"") };
                        let a = wide(&args);
                        let r = ShellExecuteW(None, None, w!("explorer.exe"), PCWSTR(a.as_ptr()), None, SW_SHOWNORMAL);
                        // > 32 = succès ; sinon code d'erreur ShellExecute.
                        log::log(&format!("lancement {} → {target} (ShellExecute = {})", item.key, r.0 as isize));
                    }
                }
                Action::ToggleAutostart => toggle_autostart(),
                Action::Quit => {
                    let _ = DestroyWindow(hwnd);
                }
            }
        }
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_HOTKEY => {
            if is_shown(hwnd) {
                hide(hwnd)
            } else {
                show(hwnd)
            }
        }
        WM_APP_SHOW => show(hwnd),
        WM_ACTIVATE => {
            if (wp.0 & 0xFFFF) as u32 == WA_INACTIVE {
                hide(hwnd);
            }
        }
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            with_app(|a| a.paint(hdc));
            let _ = EndPaint(hwnd, &ps);
        }
        WM_ERASEBKGND => return LRESULT(1),
        WM_KEYDOWN => {
            let act = with_app(|a| a.key_down(wp.0 as u32)).unwrap_or(Act::None);
            perform(hwnd, act);
        }
        WM_CHAR => {
            with_app(|a| a.on_char(wp.0 as u32));
        }
        WM_LBUTTONDOWN => {
            let (x, y) = ((lp.0 & 0xFFFF) as i16 as i32, ((lp.0 >> 16) & 0xFFFF) as i16 as i32);
            let act = with_app(|a| a.click(x, y)).unwrap_or(Act::None);
            perform(hwnd, act);
        }
        WM_TIMER => {
            with_app(|a| a.caret_on = !a.caret_on);
            let _ = InvalidateRect(hwnd, None, false);
        }
        WM_APP_INDEX => {
            with_app(|a| {
                if let Some(items) = a.index_rx.try_iter().last() {
                    a.indexing = false;
                    a.last_index = Instant::now();
                    a.set_items(items);
                    if a.visible {
                        let (q, sel) = (a.query.clone(), a.sel);
                        a.refresh();
                        if q.is_empty() || a.results.len() > sel {
                            a.sel = sel.min(a.results.len().saturating_sub(1));
                        }
                    }
                }
            });
        }
        WM_APP_ICON => {
            let vis = with_app(|a| {
                for (key, icon) in a.icon_rx.try_iter().collect::<Vec<_>>() {
                    if let Some(Some(old)) = a.icons.insert(key, icon) {
                        old.free();
                    }
                }
                a.visible
            });
            if vis == Some(true) {
                let _ = InvalidateRect(hwnd, None, false);
            }
        }
        WM_APP_TRAY => match (lp.0 & 0xFFFF) as u32 {
            WM_LBUTTONUP => {
                // Cliquer sur l'icône fait perdre le focus à Wayne (donc le masque) juste avant :
                // dans ce cas, le clic voulait fermer, pas rouvrir.
                let just_hidden = LAST_HIDE.with(|c| c.get()).is_some_and(|t| t.elapsed() < Duration::from_millis(400));
                if is_shown(hwnd) || just_hidden {
                    hide(hwnd)
                } else {
                    show(hwnd)
                }
            }
            WM_RBUTTONUP => tray_menu(hwnd),
            _ => {}
        },
        WM_DPICHANGED => {}
        WM_CLOSE => hide(hwnd),
        WM_DESTROY => {
            tray(hwnd, NIM_DELETE);
            PostQuitMessage(0)
        }
        // L'Explorateur a redémarré : on remet l'icône.
        m if m != 0 && m == TASKBAR_CREATED.with(|c| c.get()) => tray(hwnd, NIM_ADD),
        _ => return DefWindowProcW(hwnd, msg, wp, lp),
    }
    LRESULT(0)
}

fn main() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);

        // Instance unique : une 2e exécution affiche simplement la fenêtre existante.
        let _mutex = CreateMutexW(None, true, w!("Local\\WayneApp.Singleton"));
        if GetLastError() == ERROR_ALREADY_EXISTS {
            if let std::result::Result::Ok(h) = FindWindowW(CLASS, None) {
                let _ = PostMessageW(h, WM_APP_SHOW, WPARAM(0), LPARAM(0));
            }
            return;
        }
        let mut in_job = BOOL(0);
        let _ = windows::Win32::System::JobObjects::IsProcessInJob(windows::Win32::System::Threading::GetCurrentProcess(), None, &mut in_job);
        log::log(&format!(
            "démarrage {} {:?} (dans un job : {})",
            std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default(),
            std::env::args().skip(1).collect::<Vec<_>>(),
            in_job.as_bool()
        ));

        let mut token = 0usize;
        let input = GdiplusStartupInput { GdiplusVersion: 1, ..Default::default() };
        GdiplusStartup(&mut token, &input, std::ptr::null_mut());

        let hinst = GetModuleHandleW(None).unwrap_or_default();
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_DROPSHADOW,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst.into(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: CLASS,
            ..Default::default()
        };
        RegisterClassExW(&wc);
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            CLASS,
            w!("Wayne"),
            WS_POPUP,
            0,
            0,
            720,
            86,
            None,
            None,
            hinst,
            None,
        )
        .expect("création de la fenêtre");

        // Coins arrondis, mode sombre et bordure discrète (Windows 11).
        let round = DWMWCP_ROUND;
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &round as *const _ as *const c_void, 4);
        let dark: i32 = 1;
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &dark as *const _ as *const c_void, 4);
        let border = cref((0x3A, 0x3A, 0x3A));
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_BORDER_COLOR, &border as *const _ as *const c_void, 4);

        let (icon_tx, icon_rx) = icons::spawn_worker(hwnd.0 as isize, WM_APP_ICON);
        let (index_tx, index_rx) = channel();
        let mut app = App {
            hwnd,
            items: Vec::new(),
            by_key: HashMap::new(),
            results: Vec::new(),
            query: String::new(),
            sel: 0,
            brain: Brain::load(),
            icons: HashMap::new(),
            icon_tx,
            icon_rx,
            index_tx,
            index_rx,
            indexing: false,
            last_index: Instant::now(),
            scale: 1.0,
            fonts: Fonts::new(1.0),
            anchor: (0, 0),
            visible: false,
            caret_on: true,
        };
        app.set_items(index::builtins());
        app.spawn_index();
        APP.with(|a| *a.borrow_mut() = Some(app));

        TASKBAR_CREATED.with(|c| c.set(RegisterWindowMessageW(w!("TaskbarCreated"))));
        tray(hwnd, NIM_ADD);

        let hotkey = RegisterHotKey(hwnd, HOTKEY_ID, MOD_ALT | MOD_NOREPEAT, VK_SPACE.0 as u32);
        log::log(&format!("Alt+Espace : {}", if hotkey.is_ok() { "enregistré" } else { "DÉJÀ PRIS" }));
        if hotkey.is_err() {
            MessageBoxW(
                None,
                w!("Le raccourci Alt+Espace est déjà utilisé par une autre application (PowerToys Run, Copilot…).\n\nClique sur l'icône W dans la zone de notification pour ouvrir Wayne."),
                w!("Wayne"),
                MB_ICONWARNING,
            );
        }

        if !std::env::args().any(|a| a == "--hidden") {
            show(hwnd);
        }

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        GdiplusShutdown(token);
    }
}

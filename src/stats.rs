//! Fenêtre « Statistiques » : ce que dit l'historique local des lancements.
//! Ouverte depuis le menu de l'icône W (clic droit) ou la commande « stats ».

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::brain::{now_secs, Brain};
use crate::icons::Icon;
use crate::index::Item;
use crate::{argb, draw_icon, font, placeholder_color, text, text_width, with_app, Gfx};

const CLASS: PCWSTR = w!("WayneStats");
const WM_MOUSELEAVE: u32 = 0x02A3;

// Couleurs : une seule teinte pour les barres (une seule série), texte en encre neutre.
const BG: (u8, u8, u8) = (0x1c, 0x1c, 0x1c);
const CARD: (u8, u8, u8) = (0x26, 0x26, 0x26);
const BORDER: (u8, u8, u8) = (0x33, 0x33, 0x33);
const INK: (u8, u8, u8) = (0xf2, 0xf2, 0xf2);
const MUTED: (u8, u8, u8) = (0xa3, 0xa3, 0xa3);
const DIM: (u8, u8, u8) = (0x78, 0x78, 0x78);
const BAR: (u8, u8, u8) = (0x4f, 0xa9, 0xb3);
const BAR_HI: (u8, u8, u8) = (0x9a, 0xdc, 0xe3);
const TRACK: (u8, u8, u8) = (0x31, 0x33, 0x34);
const GRID: (u8, u8, u8) = (0x3a, 0x3a, 0x3a);
const CHIP: (u8, u8, u8) = (0x36, 0x39, 0x3b);

// Mise en page (pixels logiques)
const W: f32 = 880.0;
const CONTENT_H: f32 = 914.0;
const MARGIN: f32 = 24.0;
const COL2: f32 = 448.0;
const COL_W: f32 = 408.0;

const MONTHS: [&str; 12] = ["janv.", "févr.", "mars", "avr.", "mai", "juin", "juil.", "août", "sept.", "oct.", "nov.", "déc."];
const DAYS_SHORT: [&str; 7] = ["lun", "mar", "mer", "jeu", "ven", "sam", "dim"];
const DAYS_LONG: [&str; 7] = ["lundi", "mardi", "mercredi", "jeudi", "vendredi", "samedi", "dimanche"];

// Graphiques survolables
const CH_HOUR: u8 = 0;
const CH_DAY: u8 = 1;
const CH_30: u8 = 2;

// ---------- Données ----------

pub struct Data {
    total: usize,
    today: usize,
    avg30: f64,
    distinct: usize,
    since: String,
    top: Vec<(String, String, usize)>,
    hours: [u32; 24],
    weekdays: [u32; 7],
    days: [u32; 30],
    day_labels: Vec<String>,
    recent: Vec<(String, String, String)>,
    learned: Vec<(String, String, usize)>,
}

fn local(ts: u64) -> SYSTEMTIME {
    unsafe {
        let v = (ts + 11_644_473_600) * 10_000_000;
        let ft = FILETIME { dwLowDateTime: v as u32, dwHighDateTime: (v >> 32) as u32 };
        let mut utc = SYSTEMTIME::default();
        let _ = FileTimeToSystemTime(&ft, &mut utc);
        let mut loc = SYSTEMTIME::default();
        let _ = SystemTimeToTzSpecificLocalTime(None, &utc, &mut loc);
        loc
    }
}

/// Numéro de jour civil (jours depuis 1970-01-01) d'une date locale.
fn day_number(t: &SYSTEMTIME) -> i64 {
    let (y, m, d) = (t.wYear as i64, t.wMonth as i64, t.wDay as i64);
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
    era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468
}

fn month(t: &SYSTEMTIME) -> &'static str {
    MONTHS[(t.wMonth as usize).clamp(1, 12) - 1]
}

fn clock(t: &SYSTEMTIME) -> String {
    format!("{} h {:02}", t.wHour, t.wMinute)
}

fn when(ts: u64, now: u64) -> String {
    let diff = now.saturating_sub(ts);
    if diff < 60 {
        return "à l'instant".into();
    }
    if diff < 3600 {
        return format!("il y a {} min", diff / 60);
    }
    let (t, n) = (local(ts), local(now));
    match day_number(&n) - day_number(&t) {
        0 => format!("aujourd'hui, {}", clock(&t)),
        1 => format!("hier, {}", clock(&t)),
        _ => format!("{} {}, {}", t.wDay, month(&t), clock(&t)),
    }
}

fn fmt_int(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push('\u{202F}');
        }
        out.push(c);
    }
    out
}

fn fmt_dec(v: f64) -> String {
    format!("{v:.1}").replace('.', ",")
}

fn plural(n: u32) -> &'static str {
    if n > 1 {
        "lancements"
    } else {
        "lancement"
    }
}

fn fallback_name(key: &str) -> String {
    let k = key.split_once(':').map(|(_, r)| r).unwrap_or(key);
    let k = k.rsplit('\\').next().unwrap_or(k);
    k.split('!').next().unwrap_or(k).split('_').next().unwrap_or(k).to_string()
}

pub fn build(brain: &Brain, items: &[Item], by_key: &HashMap<String, usize>) -> Data {
    let name = |k: &str| by_key.get(k).map(|&i| items[i].name.clone()).unwrap_or_else(|| fallback_name(k));
    let now = now_secs();
    let today = day_number(&local(now));
    let events: Vec<(u64, &str, &str)> = brain.history().collect();

    let mut counts: HashMap<&str, usize> = HashMap::new();
    let mut by_query: HashMap<&str, HashMap<&str, usize>> = HashMap::new();
    let (mut hours, mut weekdays, mut days) = ([0u32; 24], [0u32; 7], [0u32; 30]);
    let mut today_n = 0;
    for &(ts, key, q) in &events {
        let t = local(ts);
        *counts.entry(key).or_default() += 1;
        hours[(t.wHour as usize) % 24] += 1;
        weekdays[(t.wDayOfWeek as usize + 6) % 7] += 1;
        let age = today - day_number(&t);
        if (0..30).contains(&age) {
            days[29 - age as usize] += 1;
        }
        if age == 0 {
            today_n += 1;
        }
        if !q.is_empty() && q.chars().count() <= 6 {
            *by_query.entry(q).or_default().entry(key).or_default() += 1;
        }
    }

    let first = events.first().map(|e| local(e.0));
    let since = first.map(|t| format!("{} {} {}", t.wDay, month(&t), t.wYear)).unwrap_or_default();
    let span = first.map(|t| (today - day_number(&t) + 1).clamp(1, 30)).unwrap_or(1);
    let avg30 = days.iter().sum::<u32>() as f64 / span as f64;

    let mut top: Vec<(&str, usize)> = counts.iter().map(|(k, v)| (*k, *v)).collect();
    top.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));

    let recent = events.iter().rev().take(6).map(|&(ts, key, _)| (key.to_string(), name(key), when(ts, now))).collect();

    // Requête → choix le plus fréquent, au moins deux fois ; on garde la requête la plus courte par élément.
    let mut learned: Vec<(&str, &str, usize)> = by_query
        .iter()
        .filter_map(|(q, m)| m.iter().max_by_key(|(_, c)| **c).map(|(k, c)| (*q, *k, *c)))
        .filter(|(_, _, c)| *c >= 2)
        .collect();
    learned.sort_by(|a, b| a.0.chars().count().cmp(&b.0.chars().count()).then(b.2.cmp(&a.2)));
    let mut seen = std::collections::HashSet::new();
    learned.retain(|(_, k, _)| seen.insert(*k));
    learned.sort_by(|a, b| b.2.cmp(&a.2).then(a.0.cmp(b.0)));

    let day_labels = (0..30)
        .map(|i| {
            let t = local(now - (29 - i as u64) * 86_400);
            format!("{} {}", t.wDay, month(&t))
        })
        .collect();

    Data {
        total: events.len(),
        today: today_n,
        avg30,
        distinct: counts.len(),
        since,
        top: top.into_iter().take(8).map(|(k, c)| (k.to_string(), name(k), c)).collect(),
        hours,
        weekdays,
        days,
        day_labels,
        recent,
        learned: learned.into_iter().take(6).map(|(q, k, c)| (q.to_string(), name(k), c)).collect(),
    }
}

// ---------- Fenêtre ----------

struct Fonts {
    title: HFONT,
    sub: HFONT,
    kpi: HFONT,
    label: HFONT,
    head: HFONT,
    body: HFONT,
    small: HFONT,
    glyph: HFONT,
}

impl Fonts {
    unsafe fn new(s: f32) -> Fonts {
        Fonts {
            title: font(26.0 * s, 700, "Segoe UI"),
            sub: font(13.5 * s, 400, "Segoe UI"),
            kpi: font(28.0 * s, 600, "Segoe UI"),
            label: font(12.5 * s, 400, "Segoe UI"),
            head: font(14.5 * s, 600, "Segoe UI"),
            body: font(13.5 * s, 400, "Segoe UI"),
            small: font(12.0 * s, 400, "Segoe UI"),
            glyph: font(11.0 * s, 700, "Segoe UI"),
        }
    }
    unsafe fn free(&self) {
        for f in [self.title, self.sub, self.kpi, self.label, self.head, self.body, self.small, self.glyph] {
            let _ = DeleteObject(HGDIOBJ(f.0));
        }
    }
}

struct Win {
    hwnd: HWND,
    data: Data,
    scale: f32,
    fonts: Fonts,
    scroll: i32,
    hover: Option<(u8, usize)>,
    /// Barres survolables, en coordonnées du contenu (sans défilement).
    bars: Vec<(u8, usize, RECT)>,
    tracking: bool,
}

thread_local! {
    static WIN: RefCell<Option<Win>> = const { RefCell::new(None) };
    static REGISTERED: Cell<bool> = const { Cell::new(false) };
    /// Mode `--preview-stats` : fermer la fenêtre quitte le programme.
    pub static PREVIEW: Cell<bool> = const { Cell::new(false) };
}

fn with_win<R>(f: impl FnOnce(&mut Win) -> R) -> Option<R> {
    WIN.with(|w| w.try_borrow_mut().ok().and_then(|mut g| g.as_mut().map(f)))
}

unsafe fn app_icon(size: i32) -> HICON {
    let hinst = GetModuleHandleW(None).unwrap_or_default();
    LoadImageW(HINSTANCE(hinst.0), PCWSTR(1 as *const u16), IMAGE_ICON, size, size, LR_DEFAULTCOLOR)
        .map(|h| HICON(h.0))
        .unwrap_or_else(|_| LoadIconW(None, IDI_APPLICATION).unwrap_or_default())
}

/// Ouvre la fenêtre (ou la ramène au premier plan avec des données fraîches).
pub unsafe fn open() {
    let Some(data) = with_app(|a| build(&a.brain, &a.items, &a.by_key)) else { return };

    if let Some(h) = WIN.with(|w| w.borrow().as_ref().map(|s| s.hwnd)) {
        with_win(|s| s.data = data);
        if IsIconic(h).as_bool() {
            let _ = ShowWindow(h, SW_RESTORE);
        }
        let _ = SetForegroundWindow(h);
        let _ = InvalidateRect(h, None, false);
        return;
    }

    let hinst = GetModuleHandleW(None).unwrap_or_default();
    if !REGISTERED.with(|r| r.replace(true)) {
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(proc_),
            hInstance: hinst.into(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hIcon: app_icon(32),
            hIconSm: app_icon(16),
            lpszClassName: CLASS,
            ..Default::default()
        };
        RegisterClassExW(&wc);
    }

    // Écran du curseur, à sa propre échelle.
    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    let mon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
    let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
    let _ = GetMonitorInfoW(mon, &mut mi);
    let (mut dx, mut dy) = (96u32, 96u32);
    let _ = GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
    let scale = dx as f32 / 96.0;
    let wa = mi.rcWork;

    let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX;
    let client_w = (W * scale).round() as i32;
    let client_h = ((CONTENT_H * scale).round() as i32).min(wa.bottom - wa.top - (80.0 * scale) as i32);
    let mut r = RECT { left: 0, top: 0, right: client_w, bottom: client_h };
    let _ = AdjustWindowRectExForDpi(&mut r, style, false, WINDOW_EX_STYLE::default(), dx);
    let (ow, oh) = (r.right - r.left, r.bottom - r.top);
    let x = wa.left + (wa.right - wa.left - ow) / 2;
    let y = wa.top + (wa.bottom - wa.top - oh) / 2;

    let Ok(hwnd) = CreateWindowExW(WINDOW_EX_STYLE::default(), CLASS, w!("Statistiques · Wayne"), style, x, y, ow, oh, None, None, hinst, None) else {
        return;
    };
    let dark: i32 = 1;
    let _ = DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &dark as *const _ as *const _, 4);

    // La fenêtre a pu être créée sur un autre écran : on se fie à son DPI réel.
    let real = GetDpiForWindow(hwnd) as f32 / 96.0;
    let scale = if real > 0.0 { real } else { scale };
    WIN.with(|w| {
        *w.borrow_mut() = Some(Win { hwnd, data, scale, fonts: Fonts::new(scale), scroll: 0, hover: None, bars: Vec::new(), tracking: false })
    });
    let _ = ShowWindow(hwnd, SW_SHOW);
    let _ = SetForegroundWindow(hwnd);
}

/// Nouvelles icônes disponibles : simple rafraîchissement de l'affichage.
pub unsafe fn invalidate() {
    if let Some(h) = WIN.with(|w| w.try_borrow().ok().and_then(|g| g.as_ref().map(|s| s.hwnd))) {
        let _ = InvalidateRect(h, None, false);
    }
}

/// Après un lancement ou une réindexation : met à jour la fenêtre si elle est ouverte.
pub unsafe fn refresh_if_open() {
    let Some(h) = WIN.with(|w| w.borrow().as_ref().map(|s| s.hwnd)) else { return };
    if let Some(data) = with_app(|a| build(&a.brain, &a.items, &a.by_key)) {
        with_win(|s| s.data = data);
        let _ = InvalidateRect(h, None, false);
    }
}

impl Win {
    fn px(&self, v: f32) -> i32 {
        (v * self.scale).round() as i32
    }
    /// Coordonnée verticale à l'écran (défilement appliqué).
    fn py(&self, v: f32) -> i32 {
        self.px(v) - self.scroll
    }
    fn fx(&self, v: f32) -> f32 {
        v * self.scale
    }
    fn fy(&self, v: f32) -> f32 {
        v * self.scale - self.scroll as f32
    }
    fn rect(&self, x: f32, y: f32, w: f32, h: f32) -> RECT {
        RECT { left: self.px(x), top: self.py(y), right: self.px(x + w), bottom: self.py(y + h) }
    }

    unsafe fn client_h(&self) -> i32 {
        let mut rc = RECT::default();
        let _ = GetClientRect(self.hwnd, &mut rc);
        rc.bottom
    }

    unsafe fn clamp_scroll(&mut self) {
        let max = (self.px(CONTENT_H) - self.client_h()).max(0);
        self.scroll = self.scroll.clamp(0, max);
    }

    unsafe fn paint(&mut self, hdc: HDC) {
        let mut rc = RECT::default();
        let _ = GetClientRect(self.hwnd, &mut rc);
        let mdc = CreateCompatibleDC(hdc);
        let bmp = CreateCompatibleBitmap(hdc, rc.right, rc.bottom);
        let old = SelectObject(mdc, HGDIOBJ(bmp.0));
        let bg = CreateSolidBrush(crate::cref(BG));
        FillRect(mdc, &rc, bg);
        let _ = DeleteObject(HGDIOBJ(bg.0));
        SetBkMode(mdc, TRANSPARENT);

        // Icônes des éléments affichés, prises dans le cache de Wayne.
        let keys: Vec<String> = self.data.top.iter().map(|t| t.0.clone()).chain(self.data.recent.iter().map(|r| r.0.clone())).collect();
        let icons: HashMap<String, (isize, i32, i32)> = with_app(|a| {
            keys.iter()
                .filter_map(|k| a.icons.get(k).and_then(|i| i.as_ref()).map(|i| (k.clone(), (i.bmp, i.w, i.h))))
                .collect()
        })
        .unwrap_or_default();

        // Deux passes sur la même mise en page : formes lissées (GDI+), puis texte et icônes (GDI).
        self.bars.clear();
        {
            let g = Gfx::new(mdc);
            self.draw(mdc, Some(&g), &icons);
        }
        self.draw(mdc, None, &icons);

        let _ = BitBlt(hdc, 0, 0, rc.right, rc.bottom, mdc, 0, 0, SRCCOPY);
        SelectObject(mdc, old);
        let _ = DeleteObject(HGDIOBJ(bmp.0));
        let _ = DeleteDC(mdc);
    }

    unsafe fn card(&self, dc: HDC, g: Option<&Gfx>, x: f32, y: f32, w: f32, h: f32, title: &str, note: &str) {
        match g {
            Some(g) => {
                g.round(self.fx(x), self.fy(y), self.fx(w), self.fx(h), self.fx(12.0), argb(BORDER));
                g.round(self.fx(x + 1.0), self.fy(y + 1.0), self.fx(w - 2.0), self.fx(h - 2.0), self.fx(11.0), argb(CARD));
            }
            None => {
                text(dc, title, self.fonts.head, INK, self.rect(x + 16.0, y + 12.0, w - 32.0, 22.0), DT_LEFT | DT_VCENTER);
                text(dc, note, self.fonts.small, MUTED, self.rect(x + 16.0, y + 12.0, w - 32.0, 22.0), DT_RIGHT | DT_VCENTER);
            }
        }
    }

    unsafe fn icon(&self, dc: HDC, g: Option<&Gfx>, icons: &HashMap<String, (isize, i32, i32)>, key: &str, name: &str, x: f32, y: f32, size: f32) {
        match (g, icons.get(key)) {
            (None, Some(&(bmp, w, h))) => draw_icon(dc, &Icon { bmp, w, h }, self.px(x), self.py(y), self.px(size)),
            (Some(g), None) => g.round(self.fx(x), self.fy(y), self.fx(size), self.fx(size), self.fx(5.0), argb(placeholder_color(key))),
            (None, None) => {
                let letter: String = name.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
                text(dc, &letter, self.fonts.glyph, INK, self.rect(x, y, size, size), DT_CENTER | DT_VCENTER);
            }
            _ => {}
        }
    }

    /// Graphique en colonnes : une barre par valeur, étiquettes sous l'axe.
    unsafe fn columns(&mut self, dc: HDC, g: Option<&Gfx>, chart: u8, x: f32, top: f32, w: f32, base: f32, values: &[u32], width_ratio: f32, labels: &[(usize, String)]) {
        let n = values.len() as f32;
        let slot = w / n;
        let bw = (slot * width_ratio).max(2.0);
        let max = values.iter().copied().max().unwrap_or(0).max(1) as f32;
        match g {
            Some(g) => {
                g.round(self.fx(x), self.fy(base), self.fx(w), self.fx(1.0).max(1.0), 0.0, argb(GRID));
                for (i, &v) in values.iter().enumerate() {
                    let bx = x + i as f32 * slot + (slot - bw) / 2.0;
                    let bh = if v > 0 { ((base - top) * v as f32 / max).max(3.0) } else { 0.0 };
                    let hover = self.hover == Some((chart, i));
                    g.bar(self.fx(bx), self.fy(base - bh), self.fx(bw), self.fx(bh), self.fx(3.0), argb(if hover { BAR_HI } else { BAR }), true);
                    // Zone de survol : toute la colonne, plus grande que la barre.
                    self.bars.push((chart, i, RECT { left: self.px(x + i as f32 * slot), top: self.px(top - 8.0), right: self.px(x + (i + 1) as f32 * slot), bottom: self.px(base + 18.0) }));
                }
            }
            None => {
                for (i, label) in labels {
                    // Aux extrémités, on aligne sur le bord du graphique pour ne pas déborder de la carte.
                    let r = if *i == 0 && slot < 40.0 {
                        (self.rect(x, base + 4.0, 120.0, 16.0), DT_LEFT)
                    } else if *i + 1 == values.len() && slot < 40.0 {
                        (self.rect(x + w - 120.0, base + 4.0, 120.0, 16.0), DT_RIGHT)
                    } else {
                        let cx = x + (*i as f32 + 0.5) * slot;
                        (self.rect(cx - 40.0, base + 4.0, 80.0, 16.0), DT_CENTER)
                    };
                    text(dc, label, self.fonts.small, DIM, r.0, r.1 | DT_TOP);
                }
            }
        }
    }

    unsafe fn draw(&mut self, dc: HDC, g: Option<&Gfx>, icons: &HashMap<String, (isize, i32, i32)>) {
        let d = &self.data;
        let (total, today, avg30, distinct) = (d.total, d.today, d.avg30, d.distinct);

        // En-tête
        if g.is_none() {
            text(dc, "Statistiques", self.fonts.title, INK, self.rect(MARGIN, 18.0, 600.0, 38.0), DT_LEFT | DT_VCENTER);
            let sub = if total == 0 {
                "Calculées sur ton PC. Rien n'est envoyé.".to_string()
            } else {
                format!("Depuis le {} · calculées sur ton PC, rien n'est envoyé", d.since)
            };
            text(dc, &sub, self.fonts.sub, MUTED, self.rect(MARGIN, 56.0, 820.0, 20.0), DT_LEFT | DT_VCENTER);
        }
        if total == 0 {
            if g.is_none() {
                text(dc, "Aucun lancement pour l'instant.", self.fonts.head, INK, self.rect(0.0, 200.0, W, 24.0), DT_CENTER | DT_VCENTER);
                text(dc, "Ouvre quelques apps avec Wayne (Alt+Espace) : tes statistiques apparaîtront ici.", self.fonts.body, MUTED, self.rect(0.0, 230.0, W, 22.0), DT_CENTER | DT_VCENTER);
            }
            return;
        }

        // Chiffres clés
        let tiles = [
            (fmt_int(total), "lancements au total"),
            (fmt_int(today), "aujourd'hui"),
            (fmt_dec(avg30), "par jour en moyenne (30 j)"),
            (fmt_int(distinct), "apps et dossiers différents"),
        ];
        let tw = (W - 2.0 * MARGIN - 3.0 * 12.0) / 4.0;
        for (i, (value, label)) in tiles.iter().enumerate() {
            let x = MARGIN + i as f32 * (tw + 12.0);
            match g {
                Some(g) => {
                    g.round(self.fx(x), self.fy(92.0), self.fx(tw), self.fx(78.0), self.fx(12.0), argb(BORDER));
                    g.round(self.fx(x + 1.0), self.fy(93.0), self.fx(tw - 2.0), self.fx(76.0), self.fx(11.0), argb(CARD));
                }
                None => {
                    text(dc, value, self.fonts.kpi, INK, self.rect(x + 16.0, 100.0, tw - 32.0, 36.0), DT_LEFT | DT_VCENTER);
                    text(dc, label, self.fonts.label, MUTED, self.rect(x + 16.0, 138.0, tw - 32.0, 20.0), DT_LEFT | DT_VCENTER | DT_END_ELLIPSIS);
                }
            }
        }

        // Les plus lancées
        let (x, y, w) = (MARGIN, 186.0, COL_W);
        self.card(dc, g, x, y, w, 296.0, "Les plus lancées", "");
        let top = self.data.top.clone();
        let max = top.first().map(|t| t.2).unwrap_or(1).max(1) as f32;
        for (i, (key, name, count)) in top.iter().enumerate() {
            let ry = y + 46.0 + i as f32 * 30.0;
            self.icon(dc, g, icons, key, name, x + 16.0, ry + 3.0, 20.0);
            let (bx, bw) = (x + 196.0, w - 196.0 - 56.0);
            match g {
                Some(g) => {
                    g.round(self.fx(bx), self.fy(ry + 9.0), self.fx(bw), self.fx(8.0), self.fx(4.0), argb(TRACK));
                    g.bar(self.fx(bx), self.fy(ry + 9.0), self.fx(bw * *count as f32 / max), self.fx(8.0), self.fx(4.0), argb(BAR), false);
                }
                None => {
                    text(dc, name, self.fonts.body, INK, self.rect(x + 46.0, ry, 140.0, 26.0), DT_LEFT | DT_VCENTER | DT_END_ELLIPSIS);
                    text(dc, &fmt_int(*count), self.fonts.body, MUTED, self.rect(x + w - 52.0, ry, 36.0, 26.0), DT_RIGHT | DT_VCENTER);
                }
            }
        }

        // Par heure
        let (x, y) = (COL2, 186.0);
        let hours = self.data.hours;
        let peak = hours.iter().enumerate().max_by_key(|(_, v)| **v).map(|(i, _)| i).unwrap_or(0);
        let note = match self.hover {
            Some((CH_HOUR, i)) => format!("{i} h · {} {}", hours[i], plural(hours[i])),
            _ => format!("pic vers {peak} h"),
        };
        self.card(dc, g, x, y, COL_W, 142.0, "Par heure", &note);
        let labels: Vec<(usize, String)> = [0, 6, 12, 18, 23].iter().map(|&h| (h, format!("{h} h"))).collect();
        self.columns(dc, g, CH_HOUR, x + 16.0, y + 46.0, COL_W - 32.0, y + 112.0, &hours, 0.72, &labels);

        // Par jour de la semaine
        let y = 340.0;
        let wd = self.data.weekdays;
        let best = wd.iter().enumerate().max_by_key(|(_, v)| **v).map(|(i, _)| i).unwrap_or(0);
        let note = match self.hover {
            Some((CH_DAY, i)) => format!("{} · {} {}", DAYS_LONG[i], wd[i], plural(wd[i])),
            _ => format!("plus actif le {}", DAYS_LONG[best]),
        };
        self.card(dc, g, x, y, COL_W, 142.0, "Par jour de la semaine", &note);
        let labels: Vec<(usize, String)> = DAYS_SHORT.iter().enumerate().map(|(i, s)| (i, s.to_string())).collect();
        self.columns(dc, g, CH_DAY, x + 16.0, y + 46.0, COL_W - 32.0, y + 112.0, &wd, 0.42, &labels);

        // Derniers lancements
        let (x, y) = (MARGIN, 494.0);
        self.card(dc, g, x, y, COL_W, 236.0, "Derniers lancements", "");
        let recent = self.data.recent.clone();
        for (i, (key, name, when)) in recent.iter().enumerate() {
            let ry = y + 46.0 + i as f32 * 30.0;
            self.icon(dc, g, icons, key, name, x + 16.0, ry + 4.0, 18.0);
            if g.is_none() {
                text(dc, name, self.fonts.body, INK, self.rect(x + 44.0, ry, 200.0, 26.0), DT_LEFT | DT_VCENTER | DT_END_ELLIPSIS);
                text(dc, when, self.fonts.small, MUTED, self.rect(x + 244.0, ry, COL_W - 260.0, 26.0), DT_RIGHT | DT_VCENTER);
            }
        }

        // 30 derniers jours
        let (x, y) = (COL2, 494.0);
        let days = self.data.days;
        let note = match self.hover {
            Some((CH_30, i)) => format!("{} · {} {}", self.data.day_labels[i], days[i], plural(days[i])),
            _ => format!("{} par jour en moyenne", fmt_dec(avg30)),
        };
        self.card(dc, g, x, y, COL_W, 236.0, "30 derniers jours", &note);
        let labels = vec![(0usize, self.data.day_labels[0].clone()), (29usize, "aujourd'hui".to_string())];
        self.columns(dc, g, CH_30, x + 16.0, y + 50.0, COL_W - 32.0, y + 202.0, &days, 0.78, &labels);

        // Ce que Wayne a appris
        let (x, y) = (MARGIN, 742.0);
        self.card(dc, g, x, y, W - 2.0 * MARGIN, 148.0, "Ce que Wayne a appris", "ce que tu tapes → ce que tu choisis le plus");
        let learned = self.data.learned.clone();
        if learned.is_empty() && g.is_none() {
            text(
                dc,
                "Pas encore assez de répétitions : choisis la même app deux fois avec les mêmes lettres.",
                self.fonts.body,
                MUTED,
                self.rect(x + 16.0, y + 50.0, W - 2.0 * MARGIN - 32.0, 24.0),
                DT_LEFT | DT_VCENTER,
            );
        }
        SelectObject(dc, HGDIOBJ(self.fonts.body.0));
        for (i, (q, name, count)) in learned.iter().enumerate() {
            let cx = x + 16.0 + (i / 3) as f32 * 408.0;
            let ry = y + 48.0 + (i % 3) as f32 * 30.0;
            SelectObject(dc, HGDIOBJ(self.fonts.body.0));
            let chip_w = text_width(dc, &format!("« {q} »")) as f32 / self.scale + 16.0;
            match g {
                Some(g) => g.round(self.fx(cx), self.fy(ry + 2.0), self.fx(chip_w), self.fx(22.0), self.fx(6.0), argb(CHIP)),
                None => {
                    text(dc, &format!("« {q} »"), self.fonts.body, INK, self.rect(cx, ry + 2.0, chip_w, 22.0), DT_CENTER | DT_VCENTER);
                    let tx = cx + chip_w + 10.0;
                    let line = format!("→  {name}");
                    text(dc, &line, self.fonts.body, INK, self.rect(tx, ry, 280.0, 26.0), DT_LEFT | DT_VCENTER | DT_END_ELLIPSIS);
                    SelectObject(dc, HGDIOBJ(self.fonts.body.0));
                    let lw = (text_width(dc, &line) as f32 / self.scale).min(280.0);
                    text(dc, &format!("· {count} fois"), self.fonts.small, MUTED, self.rect(tx + lw + 8.0, ry, 90.0, 26.0), DT_LEFT | DT_VCENTER);
                }
            }
        }
    }
}

unsafe fn scroll_by(hwnd: HWND, delta: i32) {
    if with_win(|s| {
        let before = s.scroll;
        s.scroll += delta;
        s.clamp_scroll();
        s.scroll != before
    }) == Some(true)
    {
        let _ = InvalidateRect(hwnd, None, false);
    }
}

unsafe extern "system" fn proc_(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            with_win(|s| s.paint(hdc));
            let _ = EndPaint(hwnd, &ps);
        }
        WM_ERASEBKGND => return LRESULT(1),
        WM_MOUSEWHEEL => {
            let delta = ((wp.0 >> 16) & 0xFFFF) as u16 as i16 as i32;
            let step = with_win(|s| s.px(64.0)).unwrap_or(64);
            scroll_by(hwnd, -delta * step / 120);
        }
        WM_KEYDOWN => {
            let page = with_win(|s| s.client_h() - s.px(60.0)).unwrap_or(400);
            let line = with_win(|s| s.px(48.0)).unwrap_or(48);
            match VIRTUAL_KEY(wp.0 as u16) {
                VK_ESCAPE => {
                    let _ = DestroyWindow(hwnd);
                }
                VK_DOWN => scroll_by(hwnd, line),
                VK_UP => scroll_by(hwnd, -line),
                VK_NEXT | VK_SPACE => scroll_by(hwnd, page),
                VK_PRIOR => scroll_by(hwnd, -page),
                VK_HOME => scroll_by(hwnd, -100_000),
                VK_END => scroll_by(hwnd, 100_000),
                _ => {}
            }
        }
        WM_MOUSEMOVE => {
            let (x, y) = ((lp.0 & 0xFFFF) as i16 as i32, ((lp.0 >> 16) & 0xFFFF) as i16 as i32);
            let changed = with_win(|s| {
                if !s.tracking {
                    let mut tme = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                    let _ = TrackMouseEvent(&mut tme);
                    s.tracking = true;
                }
                let cy = y + s.scroll;
                let hit = s.bars.iter().find(|(_, _, r)| x >= r.left && x < r.right && cy >= r.top && cy < r.bottom).map(|(c, i, _)| (*c, *i));
                let changed = hit != s.hover;
                s.hover = hit;
                changed
            });
            if changed == Some(true) {
                let _ = InvalidateRect(hwnd, None, false);
            }
        }
        WM_MOUSELEAVE => {
            with_win(|s| {
                s.tracking = false;
                s.hover = None;
            });
            let _ = InvalidateRect(hwnd, None, false);
        }
        WM_SIZE => {
            with_win(|s| s.clamp_scroll());
            let _ = InvalidateRect(hwnd, None, false);
        }
        WM_DPICHANGED => {
            let dpi = (wp.0 & 0xFFFF) as f32;
            with_win(|s| {
                s.fonts.free();
                s.scale = dpi / 96.0;
                s.fonts = Fonts::new(s.scale);
                s.scroll = 0;
            });
            let r = &*(lp.0 as *const RECT);
            let _ = SetWindowPos(hwnd, None, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
        }
        WM_DESTROY => {
            if let Some(s) = WIN.with(|w| w.borrow_mut().take()) {
                s.fonts.free();
            }
            if PREVIEW.with(|p| p.get()) {
                PostQuitMessage(0);
            }
        }
        _ => return DefWindowProcW(hwnd, msg, wp, lp),
    }
    LRESULT(0)
}

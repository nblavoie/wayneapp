//! Le « cerveau » de Wayne : un modèle d'apprentissage en ligne, 100 % local.
//!
//! Chaque lancement est enregistré (heure, jour, requête tapée, élément choisi).
//! On en tire, avec une décroissance exponentielle (demi-vie de 30 jours) :
//!   * P(app) : fréquence récente (« frécence ») ;
//!   * P(app | requête) : ce que tu choisis habituellement après avoir tapé « p », « pi »… ;
//!   * P(heure | app), P(semaine/fin de semaine | app) : habitudes horaires (Bayes naïf lissé).
//! Requête vide → prédiction bayésienne P(app | heure, jour) ; requête non vide → ces
//! probabilités viennent booster le score de correspondance texte.

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

const HALF_LIFE_DAYS: f64 = 30.0;
const MAX_EVENTS: usize = 5000;

struct Event {
    ts: u64,
    hour: usize,
    weekend: usize,
    key: String,
    query: String,
}

#[derive(Default)]
pub struct Brain {
    path: PathBuf,
    events: Vec<Event>,
    freq: HashMap<String, f64>,
    total: f64,
    hours: HashMap<String, [f64; 24]>,
    days: HashMap<String, [f64; 2]>,
    assoc: HashMap<String, HashMap<String, f64>>,
    assoc_tot: HashMap<String, f64>,
}

pub fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// (heure locale de 0 à 23, 1 si fin de semaine)
pub fn local_context() -> (usize, usize) {
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    let weekend = (t.wDayOfWeek == 0 || t.wDayOfWeek == 6) as usize;
    (t.wHour as usize % 24, weekend)
}

impl Brain {
    pub fn load() -> Brain {
        let dir = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(".")).join("Wayne");
        let _ = fs::create_dir_all(&dir);
        let mut b = Brain { path: dir.join("history.tsv"), ..Default::default() };
        if let Ok(txt) = fs::read_to_string(&b.path) {
            for line in txt.lines() {
                let f: Vec<&str> = line.split('\t').collect();
                if f.len() < 5 {
                    continue;
                }
                let (Ok(ts), Ok(hour), Ok(weekend)) = (f[0].parse(), f[1].parse::<usize>(), f[2].parse::<usize>()) else {
                    continue;
                };
                b.events.push(Event { ts, hour: hour % 24, weekend: weekend.min(1), key: f[3].to_string(), query: f[4].to_string() });
            }
            if b.events.len() > MAX_EVENTS {
                let cut = b.events.len() - MAX_EVENTS;
                b.events.drain(..cut);
                b.rewrite();
            }
        }
        b.compute();
        b
    }

    fn rewrite(&self) {
        let mut s = String::new();
        for e in &self.events {
            s.push_str(&format!("{}\t{}\t{}\t{}\t{}\n", e.ts, e.hour, e.weekend, e.key, e.query));
        }
        let _ = fs::write(&self.path, s);
    }

    /// Historique brut (horodatage, élément, requête tapée), du plus ancien au plus récent.
    pub fn history(&self) -> impl Iterator<Item = (u64, &str, &str)> {
        self.events.iter().map(|e| (e.ts, e.key.as_str(), e.query.as_str()))
    }

    /// Enregistre un lancement et réentraîne le modèle (quelques millisecondes).
    pub fn record(&mut self, key: &str, query: &str) {
        let (hour, weekend) = local_context();
        let query = query.replace(['\t', '\n', '\r'], " ");
        let e = Event { ts: now_secs(), hour, weekend, key: key.to_string(), query };
        let written = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .and_then(|mut f| writeln!(f, "{}\t{}\t{}\t{}\t{}", e.ts, e.hour, e.weekend, e.key, e.query));
        match written {
            Ok(()) => crate::log::log(&format!("appris : {} ← « {} »", e.key, e.query)),
            Err(err) => crate::log::log(&format!("ÉCHEC écriture historique {} : {err}", self.path.display())),
        }
        self.events.push(e);
        if self.events.len() > MAX_EVENTS * 6 / 5 {
            let cut = self.events.len() - MAX_EVENTS;
            self.events.drain(..cut);
            self.rewrite();
        }
        self.compute();
    }

    fn compute(&mut self) {
        self.freq.clear();
        self.hours.clear();
        self.days.clear();
        self.assoc.clear();
        self.assoc_tot.clear();
        self.total = 0.0;
        let now = now_secs();
        for e in &self.events {
            let age_days = now.saturating_sub(e.ts) as f64 / 86400.0;
            let w = 0.5f64.powf(age_days / HALF_LIFE_DAYS);
            *self.freq.entry(e.key.clone()).or_default() += w;
            self.total += w;
            self.hours.entry(e.key.clone()).or_insert([0.0; 24])[e.hour] += w;
            self.days.entry(e.key.clone()).or_insert([0.0; 2])[e.weekend] += w;
            let mut prefix = String::new();
            for c in e.query.chars() {
                prefix.push(c);
                *self.assoc.entry(prefix.clone()).or_default().entry(e.key.clone()).or_default() += w;
                *self.assoc_tot.entry(prefix.clone()).or_default() += w;
            }
        }
    }

    /// log( P(heure | app) / uniforme ) + log( P(jour | app) / base ), lissés.
    fn context_affinity(&self, key: &str, hour: usize, weekend: usize) -> f64 {
        let f = self.freq.get(key).copied().unwrap_or(0.0);
        let mut a = 0.0;
        if let Some(h) = self.hours.get(key) {
            let sm = 0.5 * h[hour] + 0.25 * h[(hour + 23) % 24] + 0.25 * h[(hour + 1) % 24];
            let p = (sm + 1.0 / 24.0) / (f + 1.0);
            a += (p * 24.0).ln();
        }
        if let Some(d) = self.days.get(key) {
            let base = if weekend == 1 { 2.0 / 7.0 } else { 5.0 / 7.0 };
            let p = (d[weekend] + base) / (f + 1.0);
            a += (p / base).ln();
        }
        a
    }

    /// Prédictions pour une requête vide : P(app | heure, jour) ∝ P(app)·P(heure|app)·P(jour|app).
    pub fn predictions(&self, n: usize) -> Vec<String> {
        let (hour, weekend) = local_context();
        let mut v: Vec<(f64, &String)> = self
            .freq
            .iter()
            .map(|(k, f)| (((f + 0.01) / (self.total + 0.01)).ln() + self.context_affinity(k, hour, weekend), k))
            .collect();
        v.sort_by(|a, b| b.0.total_cmp(&a.0));
        v.into_iter().take(n).map(|(_, k)| k.clone()).collect()
    }

    /// Bonus appris à ajouter au score texte pour une requête normalisée.
    pub fn boost(&self, q: &str, key: &str, hour: usize, weekend: usize) -> f32 {
        let f = match self.freq.get(key) {
            Some(f) => *f,
            None => return 0.0,
        };
        let p_query = match (self.assoc.get(q).and_then(|m| m.get(key)), self.assoc_tot.get(q)) {
            (Some(a), Some(t)) if *t > 0.0 => a / t,
            _ => 0.0,
        };
        let ctx = self.context_affinity(key, hour, weekend).clamp(-2.0, 2.0);
        (35.0 * p_query + 4.0 * (1.0 + f).ln() + 2.5 * ctx) as f32
    }
}

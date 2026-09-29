//! Correspondance texte : normalisation (minuscules, sans accents) et score flou.

use crate::index::Item;

/// Minuscules + suppression des accents courants, pour que « tele » trouve « Téléchargements ».
pub fn fold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        for lc in c.to_lowercase() {
            out.push(match lc {
                'à' | 'á' | 'â' | 'ä' | 'ã' | 'å' => 'a',
                'ç' => 'c',
                'è' | 'é' | 'ê' | 'ë' => 'e',
                'ì' | 'í' | 'î' | 'ï' => 'i',
                'ñ' => 'n',
                'ò' | 'ó' | 'ô' | 'ö' | 'õ' => 'o',
                'ù' | 'ú' | 'û' | 'ü' => 'u',
                'ý' | 'ÿ' => 'y',
                other => other,
            });
        }
    }
    out
}

/// Découpe un nom en mots : séparateurs non alphanumériques et transitions camelCase
/// (« PowerShell » → « power », « shell »).
pub fn words(name: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut prev_lower = false;
    for c in name.chars() {
        if !c.is_alphanumeric() {
            if !cur.is_empty() {
                out.push(fold(&cur));
                cur.clear();
            }
            prev_lower = false;
            continue;
        }
        if c.is_uppercase() && prev_lower && !cur.is_empty() {
            out.push(fold(&cur));
            cur.clear();
        }
        prev_lower = c.is_lowercase() || c.is_numeric();
        cur.push(c);
    }
    if !cur.is_empty() {
        out.push(fold(&cur));
    }
    out
}

/// Score de correspondance (0–100) d'une requête déjà normalisée, ou None.
pub fn score(q: &str, it: &Item) -> Option<f32> {
    if it.aliases.iter().any(|a| a == q) {
        return Some(100.0);
    }
    let n = &it.fname;
    if n == q {
        return Some(98.0);
    }
    if n.starts_with(q) {
        return Some(90.0 - ((n.len() - q.len()) as f32 * 0.1).min(8.0));
    }
    if it.aliases.iter().any(|a| a.starts_with(q)) {
        return Some(86.0);
    }
    let toks: Vec<&str> = q.split_whitespace().collect();
    if toks.len() == 1 {
        if let Some(i) = it.words.iter().position(|w| w.starts_with(q)) {
            return Some(78.0 - i as f32);
        }
    } else if toks.len() > 1 && toks.iter().all(|t| it.words.iter().any(|w| w.starts_with(t))) {
        return Some(76.0);
    }
    let compact: String = q.chars().filter(|c| !c.is_whitespace()).collect();
    let clen = compact.chars().count();
    if clen >= 2 && it.initials.starts_with(&compact) {
        return Some(72.0);
    }
    if n.contains(q) {
        return Some(55.0);
    }
    if clen >= 2 {
        if let Some(gap) = subsequence_gap(&compact, n) {
            return Some((45.0 - gap as f32 * 2.0).max(20.0));
        }
    }
    None
}

/// Les caractères de `q` apparaissent-ils dans l'ordre dans `n` ? Retourne le nombre de
/// caractères sautés entre la première et la dernière correspondance.
fn subsequence_gap(q: &str, n: &str) -> Option<usize> {
    let mut qi = q.chars().peekable();
    let mut started = false;
    let mut gap = 0;
    for c in n.chars() {
        match qi.peek() {
            None => break,
            Some(&qc) if qc == c => {
                started = true;
                qi.next();
            }
            Some(_) if started => gap += 1,
            _ => {}
        }
    }
    if qi.peek().is_none() {
        Some(gap)
    } else {
        None
    }
}

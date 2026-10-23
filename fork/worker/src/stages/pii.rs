//! First PII layer: regex plus checksums on text. Cheap and precise; the
//! GLiNER2 model is a later, second check. Only the kinds found and their
//! counts are kept, never the matched values.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{Value, json};

use crate::config::PiiKind;

/// Kinds that on their own make the worker suggest Hidden. Email and phone
/// numbers are reported but are too common in screenshots to suggest it.
pub const SUGGEST_HIDDEN: [PiiKind; 4] = [PiiKind::Card, PiiKind::Iban, PiiKind::Mrz, PiiKind::Nik];

#[expect(clippy::expect_used, reason = "constant patterns, covered by tests")]
fn re(p: &str) -> Regex {
    Regex::new(p).expect("valid regex")
}

static CARD: LazyLock<Regex> = LazyLock::new(|| re(r"\b\d(?:[ -]?\d){12,18}\b"));
static IBAN: LazyLock<Regex> = LazyLock::new(|| re(r"\b[A-Z]{2}\d{2}(?:[ ]?[A-Z0-9]){11,30}\b"));
static MRZ_TD3: LazyLock<Regex> = LazyLock::new(|| {
    re(r"[A-Z0-9<]{9}[0-9][A-Z<]{3}[0-9]{6}[0-9][MF<][0-9]{6}[0-9][A-Z0-9<]{14}[0-9<][0-9]")
});
static NIK: LazyLock<Regex> = LazyLock::new(|| re(r"\b\d{16}\b"));
static EMAIL: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)\b[a-z0-9._%+-]+@[a-z0-9.-]+\.[a-z]{2,}\b"));
static PHONE: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?:\+\d{1,3}[ .-]?|\b0)\(?\d{2,4}\)?(?:[ .-]?\d{2,4}){2,4}"));

pub type Hits = BTreeMap<PiiKind, usize>;

pub fn scan<'a>(texts: impl Iterator<Item = &'a str>, kinds: &[PiiKind]) -> Hits {
    let mut hits = Hits::new();
    for text in texts {
        for kind in kinds {
            let n = count(*kind, text);
            if n > 0 {
                *hits.entry(*kind).or_default() += n;
            }
        }
    }
    hits
}

fn count(kind: PiiKind, text: &str) -> usize {
    match kind {
        PiiKind::Card => CARD.find_iter(text).filter(|m| card_ok(m.as_str())).count(),
        PiiKind::Iban => IBAN.find_iter(text).filter(|m| iban_ok(m.as_str())).count(),
        PiiKind::Mrz => {
            let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
            MRZ_TD3
                .find_iter(&compact)
                .filter(|m| mrz_td3_ok(m.as_str()))
                .count()
        }
        PiiKind::Nik => NIK.find_iter(text).filter(|m| nik_ok(m.as_str())).count(),
        PiiKind::Email => EMAIL.find_iter(text).count(),
        PiiKind::Phone => PHONE
            .find_iter(text)
            .filter(|m| {
                let digits = m.as_str().chars().filter(char::is_ascii_digit).count();
                (9..=15).contains(&digits) && !card_ok(m.as_str())
            })
            .count(),
    }
}

fn digits(s: &str) -> Vec<u32> {
    s.chars().filter_map(|c| c.to_digit(10)).collect()
}

pub fn luhn(d: &[u32]) -> bool {
    let sum: u32 = d
        .iter()
        .rev()
        .enumerate()
        .map(|(i, &x)| {
            if i % 2 == 1 {
                if x * 2 > 9 { x * 2 - 9 } else { x * 2 }
            } else {
                x
            }
        })
        .sum();
    sum.is_multiple_of(10)
}

fn card_ok(s: &str) -> bool {
    let d = digits(s);
    // Known issuer prefixes keep random digit runs from matching.
    let prefix_ok = match d.first() {
        Some(4) => true,
        Some(5) => matches!(d.get(1), Some(1..=5)),
        Some(2) => matches!(d.get(1), Some(2..=7)),
        Some(3) => matches!(d.get(1), Some(4 | 7 | 5 | 6 | 0 | 8)),
        Some(6) => true,
        _ => false,
    };
    (13..=19).contains(&d.len()) && prefix_ok && luhn(&d)
}

pub fn iban_ok(s: &str) -> bool {
    let s: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    if !(15..=34).contains(&s.len()) {
        return false;
    }
    let (head, tail) = s.split_at(4);
    let mut rem = 0u32;
    for c in tail.chars().chain(head.chars()) {
        let v = match c.to_digit(36) {
            Some(v) => v,
            None => return false,
        };
        for d in if v >= 10 {
            vec![v / 10, v % 10]
        } else {
            vec![v]
        } {
            rem = (rem * 10 + d) % 97;
        }
    }
    rem == 1
}

fn mrz_value(c: char) -> u32 {
    match c {
        '0'..='9' => c as u32 - '0' as u32,
        'A'..='Z' => c as u32 - 'A' as u32 + 10,
        _ => 0,
    }
}

fn mrz_check(field: &str, check: char) -> bool {
    let sum: u32 = field
        .chars()
        .zip([7, 3, 1].iter().cycle())
        .map(|(c, w)| mrz_value(c) * w)
        .sum();
    check.to_digit(10) == Some(sum % 10) || (check == '<' && sum.is_multiple_of(10))
}

/// Line 2 of a passport MRZ (TD3, 44 characters): document number, birth
/// date, expiry and the composite check digit must all verify.
pub fn mrz_td3_ok(line: &str) -> bool {
    let c: Vec<char> = line.chars().collect();
    if c.len() != 44 {
        return false;
    }
    let s = |a: usize, b: usize| c[a..b].iter().collect::<String>();
    let composite = format!("{}{}{}", s(0, 10), s(13, 20), s(21, 43));
    mrz_check(&s(0, 9), c[9])
        && mrz_check(&s(13, 19), c[19])
        && mrz_check(&s(21, 27), c[27])
        && mrz_check(&composite, c[43])
}

// Indonesian province codes (first two NIK digits).
const PROVINCES: [u32; 38] = [
    11, 12, 13, 14, 15, 16, 17, 18, 19, 21, 31, 32, 33, 34, 35, 36, 51, 52, 53, 61, 62, 63, 64, 65,
    71, 72, 73, 74, 75, 76, 81, 82, 91, 92, 93, 94, 95, 96,
];

/// Indonesian NIK: a real province code, birth day 1..=31 (plus 40 for
/// women), month 1..=12, and a non-zero serial.
pub fn nik_ok(s: &str) -> bool {
    let d = digits(s);
    if d.len() != 16 {
        return false;
    }
    let n = |a: usize| d[a] * 10 + d[a + 1];
    let day = n(6);
    PROVINCES.contains(&n(0))
        && !card_ok(s)
        && ((1..=31).contains(&day) || (41..=71).contains(&day))
        && (1..=12).contains(&n(8))
        && d[12..].iter().any(|x| *x != 0)
}

pub fn to_json(hits: &Hits) -> Value {
    let suggest = hits.keys().any(|k| SUGGEST_HIDDEN.contains(k));
    json!({
        "method": "regex",
        "kinds": hits,
        "suggestHidden": suggest,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(text: &str) -> Vec<PiiKind> {
        scan(std::iter::once(text), &PiiKind::ALL)
            .into_keys()
            .collect()
    }

    #[test]
    fn checksums() {
        assert!(card_ok("4111 1111 1111 1111"));
        assert!(!card_ok("4111 1111 1111 1112"));
        assert!(iban_ok("GB82 WEST 1234 5698 7654 32"));
        assert!(!iban_ok("GB82 WEST 1234 5698 7654 33"));
        // ICAO 9303 specimen.
        assert!(mrz_td3_ok("L898902C36UTO7408122F1204159ZE184226B<<<<<10"));
        assert!(!mrz_td3_ok("L898902C37UTO7408122F1204159ZE184226B<<<<<10"));
        assert!(nik_ok("3174015507900003"));
        assert!(!nik_ok("9974015507900003"));
        assert!(!nik_ok("3174013207900003"));
    }

    #[test]
    fn scan_reports_kinds_not_values() {
        assert_eq!(
            found("pay to GB82 WEST 1234 5698 7654 32 today"),
            vec![PiiKind::Iban]
        );
        assert_eq!(found("card 4111-1111-1111-1111"), vec![PiiKind::Card]);
        assert_eq!(found("mail me: someone@example.com"), vec![PiiKind::Email]);
        assert_eq!(found("call +62 812 3456 7890"), vec![PiiKind::Phone]);
        assert_eq!(found("NIK 3174015507900003"), vec![PiiKind::Nik]);
        assert!(found("IMG_20240101_123456 sunset at the beach").is_empty());
        let hits = scan(
            ["x 4111111111111111", "and 4111111111111111"].into_iter(),
            &[PiiKind::Card],
        );
        let j = to_json(&hits);
        assert_eq!(j["kinds"]["card"], 2);
        assert_eq!(j["suggestHidden"], true);
        assert!(!j.to_string().contains("4111"));
    }

    #[test]
    fn email_and_phone_alone_do_not_suggest_hidden() {
        let hits = scan(std::iter::once("a@b.co"), &PiiKind::ALL);
        assert_eq!(to_json(&hits)["suggestHidden"], false);
    }

    #[test]
    fn disabled_kinds_are_skipped() {
        assert!(scan(std::iter::once("4111 1111 1111 1111"), &[PiiKind::Iban]).is_empty());
    }
}

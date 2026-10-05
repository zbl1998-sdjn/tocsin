//! The parser must never panic, whatever it is given.
//!
//! Every fixture URL is damaged in random ways (characters deleted, doubled or
//! cut off, and awkward ones like `%`, `[`, `\`, a line break or an emoji put in
//! its place). Parsing, preparing a request and printing the result must all
//! return normally. The generator is deterministic, so a failure repeats.
#![cfg(feature = "_services")]

use serde_json::Value;
use tocsin::{Notification, Service};

/// xorshift64, enough to pick positions and characters.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, limit: usize) -> usize {
        let limit = u64::try_from(limit).expect("small limit");
        usize::try_from(self.next() % limit).expect("below the limit")
    }
}

const PIECES: [&str; 22] = [
    "%", "%2", "%FF", "%00", "\\", "[", "]", ":", "@", "?", "#", "/", "&", ";", " ", "\n", "\t",
    "é", "🦀", "\u{0}", "=", "+",
];

fn damage(rng: &mut Rng, url: &str) -> String {
    let mut chars: Vec<char> = url.chars().collect();
    for _ in 0..=rng.below(4) {
        let at = rng.below(chars.len() + 1);
        match rng.below(4) {
            0 if at < chars.len() => {
                chars.remove(at);
            }
            1 => {
                for c in PIECES[rng.below(PIECES.len())].chars().rev() {
                    chars.insert(at, c);
                }
            }
            2 => chars.truncate(at),
            _ => {
                let end = (at + rng.below(8)).min(chars.len());
                let copy: Vec<char> = chars[at..end].to_vec();
                for (offset, c) in copy.into_iter().enumerate() {
                    chars.insert(at + offset, c);
                }
            }
        }
    }
    chars.into_iter().collect()
}

#[test]
fn damaged_fixtures_never_panic() {
    let fixtures: Value =
        serde_json::from_str(include_str!("../compat/fixtures.json")).expect("fixtures JSON");
    let notification = Notification::new("body").title("title");
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    // `TOCSIN_FUZZ_ROUNDS=5000 cargo test` digs deeper than the default.
    let rounds: u32 = std::env::var("TOCSIN_FUZZ_ROUNDS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(60);
    // Fixtures of a service this build lacks never parse, so the check that the
    // generator still makes valid URLs counts only those whose original parses.
    let (mut tried, mut parsed) = (0_u32, 0_u32);
    for fixture in fixtures.as_array().expect("fixtures") {
        let url = fixture["url"].as_str().expect("URL");
        let counted = Service::parse(url).is_ok();
        for _ in 0..rounds {
            let damaged = damage(&mut rng, url);
            let service = Service::parse(&damaged);
            if counted {
                tried += 1;
                parsed += u32::from(service.is_ok());
            }
            if let Ok(service) = service {
                let _ = service.prepare(&notification);
                let _ = format!("{service:?} {service}");
            }
        }
    }
    // A generator that only produced invalid URLs would prove nothing. Services
    // whose keys have a fixed length (Prowl, Zulip, ...) lose most URLs to the
    // first damaged character, so a few percent is enough.
    eprintln!("{parsed} of {tried} damaged URLs parse");
    assert!(
        tried == 0 || parsed * 50 > tried,
        "only {parsed} of {tried} URLs parsed"
    );
}

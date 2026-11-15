//! Breadboard's bench engine (ADR-010 D4) and the canonical bench CSV (D3).
//!
//! A bench knows its whole stimulus in advance. What it drives in the
//! circuit (analog playback, current profiles, switch presses, faults,
//! timed `set` actions) is handed to the analog engine once, as tables
//! (`bb_analog_ngspice::TableSource`), so no analog value crosses the kernel
//! boundary per solver point. What it drives into an MCU (a digital
//! playback, a `set` on a bound `digital_in` pin) and every action with a
//! trigger (`after = { edge, level, delay }`) is this engine's: an ordinary
//! engine of the kernel, with `t_safe = Time::MAX` (it reports every change
//! in its trace), an exact `quiet_until`, and an early stop on the lattice.
//!
//! Its identity is a content hash of these sources ([`identity`]), recorded
//! in `run.json` as `bench@1`.

pub mod csv;
mod engine;

pub use engine::{BenchConfig, BenchEngine, OutputSpec, TriggerSpec, Value};

use bbcore::Time;

/// The engine's name in events (`engine:bench`, `bench.<pin>`).
pub const ENGINE_NAME: &str = "bench";

/// The engine's identity (ADR-010 D4).
pub const ENGINE_ID: &str = "bench@1";

/// The sources that decide the engine's behaviour, in its content hash.
pub const SOURCES: [(&str, &str); 3] = [
    ("lib.rs", include_str!("lib.rs")),
    ("engine.rs", include_str!("engine.rs")),
    ("csv.rs", include_str!("csv.rs")),
];

/// The content hash of [`SOURCES`]: `sha256:<hex>` of each name, a NUL, the
/// text and a NUL, in order.
pub fn identity() -> String {
    let mut buf = Vec::new();
    for (n, s) in SOURCES {
        buf.extend_from_slice(n.as_bytes());
        buf.push(0);
        buf.extend_from_slice(s.as_bytes());
        buf.push(0);
    }
    format!("sha256:{}", bbpkg::sha256::hex_digest(&buf))
}

/// The contact bounce of a press (ADR-010 D2, `bounce = { count, max }`):
/// `2 * count` distinct offsets in `(0, max)`, sorted, at which the contact
/// toggles after it first changes (so it ends in the new state). A pure
/// function of the bench `seed` and `key` (the switch and the action), by
/// SplitMix64.
pub fn bounce_offsets(seed: u64, key: &str, count: u32, max: Time) -> Vec<Time> {
    let mut x = seed ^ 0x9e37_79b9_7f4a_7c15;
    for b in key.bytes() {
        x = splitmix(x ^ u64::from(b));
    }
    let span = max.as_fs().saturating_sub(1);
    let mut out: Vec<u64> = Vec::new();
    if span == 0 {
        return Vec::new();
    }
    let want = 2 * count as usize;
    let mut guard = 0u32;
    while out.len() < want && guard < 1_000_000 {
        guard += 1;
        x = splitmix(x);
        let o = 1 + x % span;
        if !out.contains(&o) {
            out.push(o);
        }
    }
    out.sort_unstable();
    out.into_iter().map(Time::from_fs).collect()
}

fn splitmix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounce_is_seeded_sorted_and_inside() {
        let a = bounce_offsets(1, "SW1/0", 3, Time::from_ms(1));
        assert_eq!(a.len(), 6);
        assert!(a.windows(2).all(|w| w[0] < w[1]));
        assert!(a.iter().all(|t| !t.is_zero() && *t < Time::from_ms(1)));
        assert_eq!(a, bounce_offsets(1, "SW1/0", 3, Time::from_ms(1)));
        assert_ne!(a, bounce_offsets(2, "SW1/0", 3, Time::from_ms(1)));
        assert_ne!(a, bounce_offsets(1, "SW1/1", 3, Time::from_ms(1)));
        assert!(bounce_offsets(1, "x", 0, Time::from_ms(1)).is_empty());
        assert!(identity().starts_with("sha256:"));
    }
}

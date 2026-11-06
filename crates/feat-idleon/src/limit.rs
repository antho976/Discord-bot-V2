//! Limits on checks, for a page anyone with the link can use: how many are under way at once,
//! in all and from one address, and how often one address may start one.
//!
//! A check is "under way" from the first byte of its paste to its answer: the paste is held in
//! memory while it arrives, while it waits for one of the two reading slots, and while it's
//! read. Counting from the start keeps a burst of pastes from piling up in memory, however
//! slowly they arrive.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::Instant;

/// Checks under way at once. Two are read at a time; the rest arrive or wait their turn. A real
/// check is read in ~15 ms, so 16 is a long queue; past it a check is turned away at once.
const UNDER_WAY: usize = 16;
/// Checks under way from one address. A double tap on Check sends two.
const PER_ADDRESS: usize = 3;
/// Checks one address can start in a burst (an account and its alts), then one per `REFILL`.
const BURST: f64 = 10.0;
const REFILL_SECS: f64 = 20.0;
/// Addresses kept at most. Past it, the ones back to a full burst are forgotten, then the one
/// idle longest: a flood of new addresses resets its own allowances, never locks others out.
const KEPT: usize = 4096;

static TOTAL: AtomicUsize = AtomicUsize::new(0);
static ADDRESSES: LazyLock<Mutex<HashMap<String, Allowance>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

struct Allowance {
    tokens: f64,
    at: Instant,
    under_way: usize,
}

impl Allowance {
    /// Tops the burst back up for the time since it was last used.
    fn refill(&mut self, now: Instant) {
        self.tokens = (self.tokens + now.saturating_duration_since(self.at).as_secs_f64() / REFILL_SECS).min(BURST);
        self.at = now;
    }
}

#[derive(Debug, PartialEq)]
pub enum Refused {
    /// Too many under way, in all or from this address.
    Busy,
    /// This address started too many lately.
    TooOften,
}

impl Refused {
    pub fn message(&self) -> &'static str {
        match self {
            Refused::Busy => "A lot of saves are being read right now. Try again in a few seconds.",
            Refused::TooOften => "You've checked a lot of saves in a short time. Try again in a minute.",
        }
    }
}

/// A check under way. Dropping it ends the check's count, so it goes wherever the work goes.
#[must_use]
pub struct Pass {
    key: String,
}

impl Drop for Pass {
    fn drop(&mut self) {
        TOTAL.fetch_sub(1, Ordering::AcqRel);
        let mut map = ADDRESSES.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(a) = map.get_mut(&self.key) {
            a.under_way = a.under_way.saturating_sub(1);
        }
    }
}

/// Starts a check for `key` (the address's limit key), or says why not.
pub fn enter(key: String) -> Result<Pass, Refused> {
    if TOTAL.fetch_add(1, Ordering::AcqRel) >= UNDER_WAY {
        TOTAL.fetch_sub(1, Ordering::AcqRel);
        return Err(Refused::Busy);
    }
    let refused = |why| {
        TOTAL.fetch_sub(1, Ordering::AcqRel);
        Err(why)
    };
    let now = Instant::now();
    let mut map = ADDRESSES.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if map.len() >= KEPT && !map.contains_key(&key) {
        map.retain(|_, a| {
            a.refill(now);
            a.under_way > 0 || a.tokens < BURST
        });
        if map.len() >= KEPT {
            // At most `UNDER_WAY` are under way, so there's always an idle one.
            let oldest = map.iter().filter(|(_, a)| a.under_way == 0).min_by_key(|(_, a)| a.at).map(|(k, _)| k.clone());
            match oldest {
                Some(k) => {
                    map.remove(&k);
                }
                None => return refused(Refused::Busy),
            }
        }
    }
    let a = map.entry(key.clone()).or_insert(Allowance { tokens: BURST, at: now, under_way: 0 });
    a.refill(now);
    if a.under_way >= PER_ADDRESS {
        return refused(Refused::Busy);
    }
    if a.tokens < 1.0 {
        return refused(Refused::TooOften);
    }
    a.tokens -= 1.0;
    a.under_way += 1;
    Ok(Pass { key })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_address_gets_a_burst_and_a_few_at_once() {
        // Its own address, so other tests' counts don't mix in. The total is shared: other
        // tests running at once hold a few passes at most.
        let key = "test:burst".to_string();
        let held: Vec<Pass> = (0..PER_ADDRESS).map(|_| enter(key.clone()).unwrap()).collect();
        assert_eq!(enter(key.clone()).err(), Some(Refused::Busy));
        drop(held);
        let mut started = PER_ADDRESS;
        while started < BURST as usize {
            drop(enter(key.clone()).unwrap());
            started += 1;
        }
        assert_eq!(enter(key.clone()).err(), Some(Refused::TooOften));
        // Another address is untouched.
        assert!(enter("test:other".into()).is_ok());
    }

    #[test]
    fn a_pass_gives_its_place_back() {
        let p = enter("test:give-back".into()).unwrap();
        drop(p);
        let map = ADDRESSES.lock().unwrap();
        assert_eq!(map.get("test:give-back").map(|a| a.under_way), Some(0));
    }
}

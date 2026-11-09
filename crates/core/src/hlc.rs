//! Hybrid logical clock: orders changes across devices without trusting
//! wall clocks. Ordering is (wall ms, counter, device id); the encoded form
//! sorts lexicographically in the same order, so SQLite can compare it as text.

use std::fmt;
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::{Error, Result};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DeviceId(pub String);

impl DeviceId {
    pub fn new_random() -> Self {
        DeviceId(uuid::Uuid::now_v7().to_string())
    }
}

impl fmt::Display for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for DeviceId {
    fn from(value: &str) -> Self {
        DeviceId(value.to_string())
    }
}

/// Field order matters: derived `Ord` compares wall, then counter, then device.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Hlc {
    pub wall_ms: u64,
    pub counter: u32,
    pub device: DeviceId,
}

impl fmt::Display for Hlc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:016x}-{:08x}-{}",
            self.wall_ms, self.counter, self.device
        )
    }
}

impl FromStr for Hlc {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        let bad = || Error::Hlc(format!("malformed `{s}`"));
        let (wall, rest) = s.split_once('-').ok_or_else(bad)?;
        let (counter, device) = rest.split_once('-').ok_or_else(bad)?;
        if wall.len() != 16 || counter.len() != 8 || device.is_empty() {
            return Err(bad());
        }
        Ok(Hlc {
            wall_ms: u64::from_str_radix(wall, 16).map_err(|_| bad())?,
            counter: u32::from_str_radix(counter, 16).map_err(|_| bad())?,
            device: DeviceId(device.to_string()),
        })
    }
}

impl Serialize for Hlc {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Hlc {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

pub trait TimeSource: Send {
    fn now_ms(&self) -> u64;
}

pub struct SystemTime;

impl TimeSource for SystemTime {
    fn now_ms(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }
}

/// Settable time for tests and simulations; clones share the same instant.
#[derive(Clone, Default)]
pub struct ManualTime(Arc<AtomicU64>);

impl ManualTime {
    pub fn new(ms: u64) -> Self {
        ManualTime(Arc::new(AtomicU64::new(ms)))
    }
    pub fn set(&self, ms: u64) {
        self.0.store(ms, Ordering::SeqCst);
    }
    pub fn advance(&self, ms: u64) {
        self.0.fetch_add(ms, Ordering::SeqCst);
    }
}

impl TimeSource for ManualTime {
    fn now_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

pub struct Clock {
    device: DeviceId,
    last_wall: u64,
    last_counter: u32,
    time: Box<dyn TimeSource>,
}

impl Clock {
    pub fn new(device: DeviceId, time: Box<dyn TimeSource>) -> Self {
        Clock {
            device,
            last_wall: 0,
            last_counter: 0,
            time,
        }
    }

    pub fn device(&self) -> &DeviceId {
        &self.device
    }

    pub fn now_ms(&self) -> u64 {
        self.time.now_ms()
    }

    /// Next timestamp for a local event; always greater than anything seen.
    pub fn tick(&mut self) -> Hlc {
        let now = self.time.now_ms();
        if now > self.last_wall {
            self.last_wall = now;
            self.last_counter = 0;
        } else {
            self.last_counter += 1;
        }
        Hlc {
            wall_ms: self.last_wall,
            counter: self.last_counter,
            device: self.device.clone(),
        }
    }

    /// Account for a remote timestamp so later local ticks sort after it.
    pub fn observe(&mut self, remote: &Hlc) {
        if (remote.wall_ms, remote.counter) > (self.last_wall, self.last_counter) {
            self.last_wall = remote.wall_ms;
            self.last_counter = remote.counter;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoding_round_trips_and_sorts_like_ord() {
        let a = Hlc {
            wall_ms: 5,
            counter: 2,
            device: "b".into(),
        };
        let b = Hlc {
            wall_ms: 5,
            counter: 10,
            device: "a".into(),
        };
        let c = Hlc {
            wall_ms: 300,
            counter: 0,
            device: "a".into(),
        };
        for h in [&a, &b, &c] {
            assert_eq!(&h.to_string().parse::<Hlc>().unwrap(), h);
        }
        assert!(a < b && b < c);
        assert!(a.to_string() < b.to_string() && b.to_string() < c.to_string());
    }

    #[test]
    fn ticks_are_monotonic_even_when_wall_clock_goes_back() {
        let time = ManualTime::new(1_000);
        let mut clock = Clock::new("d".into(), Box::new(time.clone()));
        let first = clock.tick();
        time.set(500);
        let second = clock.tick();
        assert!(second > first);
    }

    #[test]
    fn observe_moves_clock_past_remote() {
        let mut clock = Clock::new("d".into(), Box::new(ManualTime::new(10)));
        let remote = Hlc {
            wall_ms: 9_999,
            counter: 4,
            device: "x".into(),
        };
        clock.observe(&remote);
        assert!(clock.tick() > remote);
    }
}

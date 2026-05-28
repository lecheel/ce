// keybind/brief_trackers.rs
//! Brief-mode Home/End double-tap detection.

use std::sync::Mutex;
use std::time::{Duration, Instant};

struct PressTracker {
    last_press: Option<Instant>,
    count: usize,
}

static HOME_TRACKER: Mutex<PressTracker> = Mutex::new(PressTracker {
    last_press: None,
    count: 0,
});
static END_TRACKER: Mutex<PressTracker> = Mutex::new(PressTracker {
    last_press: None,
    count: 0,
});

pub fn reset() {
    if let Ok(mut t) = HOME_TRACKER.lock() {
        t.last_press = None;
        t.count = 0;
    }
    if let Ok(mut t) = END_TRACKER.lock() {
        t.last_press = None;
        t.count = 0;
    }
}

pub fn home_tap() -> usize {
    let now = Instant::now();
    let mut t = HOME_TRACKER.lock().unwrap();
    let consecutive = t.last_press.map_or(false, |l| {
        now.duration_since(l) < Duration::from_millis(500)
    });
    t.count = if consecutive { (t.count + 1) % 3 } else { 0 };
    t.last_press = Some(now);
    t.count
}

pub fn end_tap() -> usize {
    let now = Instant::now();
    let mut t = END_TRACKER.lock().unwrap();
    let consecutive = t.last_press.map_or(false, |l| {
        now.duration_since(l) < Duration::from_millis(500)
    });
    t.count = if consecutive { (t.count + 1) % 3 } else { 0 };
    t.last_press = Some(now);
    t.count
}

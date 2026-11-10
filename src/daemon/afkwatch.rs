//! AFK watcher (standalone Mac app): every couple of seconds look at the AFK
//! switch (`afk::dir()/on`) and do what changed: keep the Mac awake, tell the
//! leads, end AFK when the person is back or the battery is low. The rules
//! live in `crate::afk`; this is only the clock and the lead notes.

use std::time::Duration;

use super::SharedEngine;
use crate::afk::{Event, Machine, RealSys};

const TICK: Duration = Duration::from_secs(2);

pub fn start_afk_watch(engine: SharedEngine) {
    if !crate::claude_check::starts_claude_here() {
        return;
    }
    std::thread::Builder::new()
        .name("seance-afk".into())
        .spawn(move || {
            let mut m = Machine::start(RealSys, crate::afk::dir());
            loop {
                std::thread::sleep(TICK);
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|t| t.as_secs())
                    .unwrap_or(0);
                // The admin prompt can block here for as long as it is open;
                // the engine is not locked while it does.
                for ev in m.tick(now) {
                    if ev == Event::On {
                        if let Ok(mut eng) = engine.lock() {
                            eng.tell_leads(crate::afk::NOTE_FOR_LEADS);
                        }
                    }
                }
            }
        })
        .ok();
}

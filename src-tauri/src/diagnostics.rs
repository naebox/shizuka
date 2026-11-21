use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

const MAX_DIAG_EVENTS: usize = 64;

#[derive(Debug, Clone)]
struct DiagEvent {
    elapsed_ms: u64,
    level: &'static str,
    message: String,
}

struct DiagStore {
    started_at: Instant,
    events: Mutex<VecDeque<DiagEvent>>,
}

static STORE: OnceLock<DiagStore> = OnceLock::new();

fn store() -> &'static DiagStore {
    STORE.get_or_init(|| DiagStore {
        started_at: Instant::now(),
        events: Mutex::new(VecDeque::with_capacity(MAX_DIAG_EVENTS)),
    })
}

/// Records an operational diagnostic event into the bounded in-memory ring buffer
/// AND forwards it to the `log` facade.
///
/// NOTE: Never call from real-time `cpal` audio callbacks or the 10ms DSP hot loop.
pub fn record(level: &'static str, message: impl Into<String>) {
    let msg = message.into();
    match level {
        "ERROR" => log::error!("{}", msg),
        "WARN" => log::warn!("{}", msg),
        _ => log::info!("{}", msg),
    }
    let st = store();
    let elapsed_ms = st.started_at.elapsed().as_millis() as u64;
    if let Ok(mut guard) = st.events.lock() {
        if guard.len() >= MAX_DIAG_EVENTS {
            guard.pop_front();
        }
        guard.push_back(DiagEvent {
            elapsed_ms,
            level,
            message: msg,
        });
    }
}

pub fn info(message: impl Into<String>) {
    record("INFO", message);
}

pub fn warn(message: impl Into<String>) {
    record("WARN", message);
}

pub fn error(message: impl Into<String>) {
    record("ERROR", message);
}

pub fn format_recent_events() -> String {
    let st = store();
    let Ok(guard) = st.events.lock() else {
        return String::new();
    };
    if guard.is_empty() {
        return "  (none)\n".to_string();
    }
    let mut out = String::new();
    for ev in guard.iter() {
        let secs = ev.elapsed_ms / 1000;
        let tenths = (ev.elapsed_ms % 1000) / 100;
        out.push_str(&format!("  [+{}.{}s] {}: {}\n", secs, tenths, ev.level, ev.message));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_diagnostics_ring_buffer_is_bounded() {
        for i in 0..(MAX_DIAG_EVENTS + 10) {
            info(format!("diag-test-event-{}", i));
        }
        let text = format_recent_events();
        assert!(text.contains(&format!("diag-test-event-{}", MAX_DIAG_EVENTS + 9)));
        let line_count = text.lines().count();
        assert!(line_count <= MAX_DIAG_EVENTS);
    }
}

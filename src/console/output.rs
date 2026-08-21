//! Console output routing shared by the classic prompt and the TUI.
//!
//! The server has one operator console, so a process-wide sink is a good fit:
//! without a sink messages retain their historical stdout behaviour; while
//! the dashboard is active they become entries in its scrollback buffer.

use parking_lot::RwLock;
use std::sync::{Arc, OnceLock};

pub type LineSink = Arc<dyn Fn(String) + Send + Sync + 'static>;

fn sink_slot() -> &'static RwLock<Option<LineSink>> {
    static SINK: OnceLock<RwLock<Option<LineSink>>> = OnceLock::new();
    SINK.get_or_init(|| RwLock::new(None))
}

pub fn set_line_sink(sink: Option<LineSink>) {
    *sink_slot().write() = sink;
}

pub fn write_line(line: String) {
    // Clone the Arc before invoking user code, so a sink can never be called
    // while the global routing lock is held.
    let sink = sink_slot().read().clone();
    if let Some(sink) = sink {
        sink(line);
    } else {
        std::println!("{line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn installed_sink_receives_complete_lines() {
        let lines = Arc::new(Mutex::new(Vec::new()));
        let captured = lines.clone();
        set_line_sink(Some(Arc::new(move |line| {
            captured.lock().expect("capture lock").push(line);
        })));

        write_line("server ready".to_string());
        set_line_sink(None);

        assert!(lines
            .lock()
            .expect("capture lock")
            .iter()
            .any(|line| line == "server ready"));
    }
}

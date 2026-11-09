//! In-memory ring buffer of recent log events for the web interface.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex};

use kitsunarr_domain::api::system::LogEntry;
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::Layer;
use tracing_subscriber::layer::Context;

pub const DEFAULT_CAPACITY: usize = 2000;

#[derive(Clone)]
pub struct LogBuffer {
    inner: Arc<Mutex<VecDeque<(Level, LogEntry)>>>,
    capacity: usize,
}

impl Default for LogBuffer {
    fn default() -> Self {
        Self::new(DEFAULT_CAPACITY)
    }
}

impl LogBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: Arc::new(Mutex::new(VecDeque::with_capacity(capacity))),
            capacity: capacity.max(1),
        }
    }

    fn push(&self, level: Level, entry: LogEntry) {
        let mut q = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if q.len() == self.capacity {
            q.pop_front();
        }
        q.push_back((level, entry));
    }

    /// Entries at `min` or more severe, newest first.
    pub fn snapshot(&self, min: Level, limit: usize) -> Vec<LogEntry> {
        let q = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        // `tracing::Level` orders TRACE > DEBUG > INFO > WARN > ERROR.
        q.iter()
            .rev()
            .filter(|(l, _)| *l <= min)
            .take(limit)
            .map(|(_, e)| e.clone())
            .collect()
    }

    pub fn layer(&self) -> LogLayer {
        LogLayer {
            buffer: self.clone(),
        }
    }
}

pub struct LogLayer {
    buffer: LogBuffer,
}

#[derive(Default)]
struct MessageVisitor {
    message: String,
    fields: String,
}

impl Visit for MessageVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message.push_str(value);
        } else {
            let _ = write!(self.fields, " {}={value}", field.name());
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            let _ = write!(self.message, "{value:?}");
        } else {
            let _ = write!(self.fields, " {}={value:?}", field.name());
        }
    }
}

impl<S: Subscriber> Layer<S> for LogLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let meta = event.metadata();
        let mut visitor = MessageVisitor::default();
        event.record(&mut visitor);
        let mut message = visitor.message;
        message.push_str(&visitor.fields);
        // Never keep secrets that might be logged accidentally.
        if message.to_ascii_lowercase().contains("apikey=") {
            message = message
                .split(' ')
                .map(|w| {
                    if w.to_ascii_lowercase().starts_with("apikey=") {
                        "apikey=***"
                    } else {
                        w
                    }
                })
                .collect::<Vec<_>>()
                .join(" ");
        }
        self.buffer.push(
            *meta.level(),
            LogEntry {
                time: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                level: meta.level().to_string(),
                target: meta.target().to_string(),
                message,
            },
        );
    }
}

/// Parses `error|warn|info|debug|trace` (default `info`).
pub fn parse_level(s: Option<&str>) -> Level {
    match s.map(str::to_ascii_lowercase).as_deref() {
        Some("error") => Level::ERROR,
        Some("warn" | "warning") => Level::WARN,
        Some("debug") => Level::DEBUG,
        Some("trace") => Level::TRACE,
        _ => Level::INFO,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing_subscriber::layer::SubscriberExt;

    #[test]
    fn captures_and_filters_events() {
        let buffer = LogBuffer::new(3);
        let subscriber = tracing_subscriber::registry().with(buffer.layer());
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(task = "Backup", "task completed");
            tracing::warn!("disk almost full");
            tracing::error!("boom");
            tracing::debug!("noise");
            tracing::info!("request apikey=secret123 done");
        });
        let all = buffer.snapshot(Level::TRACE, 10);
        assert_eq!(all.len(), 3, "capacity bounds the buffer");
        assert_eq!(all[0].message, "request apikey=*** done");
        let errors = buffer.snapshot(Level::ERROR, 10);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].message, "boom");
        assert_eq!(parse_level(Some("WARN")), Level::WARN);
    }
}

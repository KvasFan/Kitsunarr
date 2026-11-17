//! Sonarr's command queue as `/api/v3/command` reports it (`CommandResource`): each command
//! the API starts is recorded as `started`, then `completed` or `failed` with its outcome, so
//! tools that poll a command (nzb360, LunaSea, scripts) see when it ends and how. The last 100
//! commands are kept in memory.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::Instant;

use serde_json::{Value, json};

const KEPT: usize = 100;

#[derive(Debug, Clone)]
pub struct CommandRecord {
    pub id: i64,
    pub name: String,
    pub body: Value,
    /// `started`, `completed` or `failed`.
    pub status: &'static str,
    pub message: String,
    pub queued: String,
    pub ended: Option<String>,
    started_at: Instant,
    pub duration_ms: Option<u128>,
}

#[derive(Default)]
pub struct CommandRegistry(Mutex<(i64, VecDeque<CommandRecord>)>);

impl CommandRegistry {
    /// Records a command that has started; returns its id.
    pub fn start(&self, name: &str, body: Value, message: &str) -> i64 {
        let mut g = self.0.lock().unwrap_or_else(|e| e.into_inner());
        g.0 += 1;
        let id = g.0;
        g.1.push_back(CommandRecord {
            id,
            name: name.to_string(),
            body,
            status: "started",
            message: message.to_string(),
            queued: kitsunarr_db::now(),
            ended: None,
            started_at: Instant::now(),
            duration_ms: None,
        });
        while g.1.len() > KEPT {
            g.1.pop_front();
        }
        id
    }

    /// Records a command's outcome: its summary, or why it failed.
    pub fn finish(&self, id: i64, outcome: Result<String, String>) {
        let mut g = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(r) = g.1.iter_mut().find(|r| r.id == id) {
            let (status, message) = match outcome {
                Ok(m) => ("completed", m),
                Err(m) => ("failed", m),
            };
            r.status = status;
            r.message = message;
            r.ended = Some(kitsunarr_db::now());
            r.duration_ms = Some(r.started_at.elapsed().as_millis());
        }
    }

    pub fn get(&self, id: i64) -> Option<CommandRecord> {
        let g = self.0.lock().unwrap_or_else(|e| e.into_inner());
        g.1.iter().find(|r| r.id == id).cloned()
    }

    /// The kept commands, newest first.
    pub fn list(&self) -> Vec<CommandRecord> {
        let g = self.0.lock().unwrap_or_else(|e| e.into_inner());
        g.1.iter().rev().cloned().collect()
    }
}

/// Sonarr's `CommandResource`.
pub fn resource(r: &CommandRecord) -> Value {
    let duration = r.duration_ms.map(|ms| {
        let ms = ms as u64;
        format!(
            "{:02}:{:02}:{:02}.{:03}",
            ms / 3_600_000,
            ms / 60_000 % 60,
            ms / 1000 % 60,
            ms % 1000
        )
    });
    json!({
        "id": r.id,
        "name": r.name,
        "commandName": r.name,
        "message": r.message,
        "body": r.body,
        "priority": "normal",
        "status": r.status,
        "result": match r.status {
            "completed" => "successful",
            "failed" => "unsuccessful",
            _ => "unknown",
        },
        "queued": r.queued,
        "started": r.queued,
        "ended": r.ended,
        "duration": duration,
        "trigger": "manual",
        "stateChangeTime": r.ended.clone().unwrap_or_else(|| r.queued.clone()),
        "sendUpdatesToClient": true,
        "updateScheduledTask": true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_are_tracked_until_they_end_and_old_ones_dropped() {
        let reg = CommandRegistry::default();
        let a = reg.start("RssSync", json!({"name": "RssSync"}), "RSS sync started");
        let b = reg.start("RefreshSeries", json!({}), "Refreshing");
        assert_eq!(resource(&reg.get(a).unwrap())["status"], "started");
        assert_eq!(resource(&reg.get(a).unwrap())["result"], "unknown");
        reg.finish(a, Ok("3 releases found".into()));
        reg.finish(b, Err("The series does not exist.".into()));
        let done = resource(&reg.get(a).unwrap());
        assert_eq!(
            (done["status"].as_str(), done["result"].as_str()),
            (Some("completed"), Some("successful"))
        );
        assert_eq!(done["message"], "3 releases found");
        assert!(done["duration"].as_str().unwrap().starts_with("00:00:00."));
        assert_eq!(resource(&reg.get(b).unwrap())["status"], "failed");
        assert_eq!(reg.list()[0].id, b, "newest first");
        for _ in 0..KEPT {
            reg.start("x", json!({}), "");
        }
        assert!(reg.get(a).is_none());
        assert_eq!(reg.list().len(), KEPT);
    }
}

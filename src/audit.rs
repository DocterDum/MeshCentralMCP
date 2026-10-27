//! Append-only JSONL audit log of every device-touching call.

use serde_json::{json, Value};
use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn log(path: &str, entry: Value) {
    let ts_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let mut line = json!({ "ts_ms": ts_ms });
    if let (Some(l), Some(e)) = (line.as_object_mut(), entry.as_object()) {
        l.extend(e.clone());
    }
    match std::fs::OpenOptions::new().create(true).append(true).open(path) {
        Ok(mut f) => {
            let _ = writeln!(f, "{line}");
        }
        Err(e) => eprintln!("audit log write failed: {e}"),
    }
}

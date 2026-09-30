// A written record of every request the provider is sent and everything
// it streams back: one file per call under the project folder, appended
// as the stream arrives, so a slow or failed call can be read while it is
// still running and two turns' requests can be compared byte for byte.
//
// The record never fails the call it describes: a directory that cannot
// be created or a line that cannot be written is logged and the request
// carries on unrecorded.
//
// The credential never reaches the file: every line is redacted before it
// is written, so an endpoint that echoes the bearer token in an error
// body or a streamed error event leaves "[REDACTED]" in its place.

use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use crate::backend::{ProviderUsage, RequestPurpose};
use crate::openai_compatible::Credential;

/// How many call files are kept. The oldest go when a new call begins,
/// so a busy project's folder stays bounded.
const KEPT_CALLS: usize = 60;

/// Where a project's calls are recorded.
#[derive(Debug, Clone)]
pub struct RequestLog {
    dir: PathBuf,
    /// Calls begun through this log and its clones. Part of every record's
    /// name, so two calls begun in the same millisecond — a refusal and
    /// its retry against a fast provider — never share a file.
    calls: Arc<std::sync::atomic::AtomicU64>,
}

impl RequestLog {
    /// Record calls under `dir`, which is created when the first call
    /// begins.
    pub fn in_directory(dir: PathBuf) -> Self {
        Self {
            dir,
            calls: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        }
    }

    pub fn directory(&self) -> &Path {
        &self.dir
    }

    /// Open the record of one call, its first line the request as sent.
    /// `credential` is the secret the call authenticates with, redacted
    /// from every line the record writes.
    pub(crate) fn begin(
        &self,
        purpose: RequestPurpose,
        body: &serde_json::Value,
        credential: Option<Arc<Credential>>,
    ) -> CallRecord {
        let call = self
            .calls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let name = format!(
            "{}-{call:04}-{}.jsonl",
            chrono::Utc::now().format("%Y%m%dT%H%M%S%.3fZ"),
            purpose.slug()
        );
        let path = self.dir.join(name);
        let writer = fs::create_dir_all(&self.dir)
            .and_then(|()| File::create(&path))
            .map(BufWriter::new)
            .map_err(|error| {
                log::warn!(
                    "Not recording the AI request at {}: {error}",
                    path.display()
                )
            })
            .ok();
        if writer.is_some() {
            self.prune();
        }
        let mut record = CallRecord {
            path,
            writer,
            credential,
            started: Instant::now(),
            events: 0,
        };
        record.line(serde_json::json!({
            "kind": "request",
            "at": chrono::Utc::now().to_rfc3339(),
            "purpose": purpose,
            "body": body,
        }));
        record
    }

    /// Remove the oldest records beyond the kept count. File names begin
    /// with a UTC timestamp, so name order is age order.
    fn prune(&self) {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return;
        };
        let mut names: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "jsonl")
            })
            .collect();
        names.sort();
        // The file just created is among the names and is the newest.
        let excess = names.len().saturating_sub(KEPT_CALLS);
        for path in names.into_iter().take(excess) {
            let _ = fs::remove_file(path);
        }
    }
}

/// The record of one call while it runs.
pub(crate) struct CallRecord {
    path: PathBuf,
    writer: Option<BufWriter<File>>,
    /// The secret redacted from every line written.
    credential: Option<Arc<Credential>>,
    started: Instant,
    events: usize,
}

/// Replace the secret wherever it appears in a string of the value.
fn redact(value: &mut serde_json::Value, secret: &str) {
    match value {
        serde_json::Value::String(text) => {
            if text.contains(secret) {
                *text = text.replace(secret, "[REDACTED]");
            }
        }
        serde_json::Value::Array(items) => items.iter_mut().for_each(|item| redact(item, secret)),
        serde_json::Value::Object(fields) => {
            fields.values_mut().for_each(|field| redact(field, secret))
        }
        _ => {}
    }
}

impl CallRecord {
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// Milliseconds since the request was sent.
    pub(crate) fn elapsed_ms(&self) -> u128 {
        self.started.elapsed().as_millis()
    }

    fn line(&mut self, mut value: serde_json::Value) {
        let Some(writer) = self.writer.as_mut() else {
            return;
        };
        if let Some(secret) = self
            .credential
            .as_ref()
            .map(|credential| credential.value())
            .filter(|secret| !secret.is_empty())
        {
            redact(&mut value, secret);
        }
        // Flushed per line: the file is read while the call runs.
        let written = serde_json::to_writer(&mut *writer, &value)
            .map_err(std::io::Error::other)
            .and_then(|()| writer.write_all(b"\n"))
            .and_then(|()| writer.flush());
        if let Err(error) = written {
            log::warn!(
                "Stopped recording the AI request at {}: {error}",
                self.path.display()
            );
            self.writer = None;
        }
    }

    /// One event as the provider streamed it.
    pub(crate) fn event(&mut self, event: &serde_json::Value) {
        self.events += 1;
        self.line(serde_json::json!({
            "kind": "event",
            "at_ms": self.started.elapsed().as_millis() as u64,
            "event": event,
        }));
    }

    /// The provider refused the request outright.
    pub(crate) fn rejected(&mut self, status: u16, body: &str) {
        self.line(serde_json::json!({
            "kind": "rejected",
            "at_ms": self.started.elapsed().as_millis() as u64,
            "http_status": status,
            "body": body,
        }));
    }

    /// How the call ended, and what the provider said it cost.
    pub(crate) fn ended(mut self, outcome: &str, usage: Option<ProviderUsage>) {
        self.line(serde_json::json!({
            "kind": "outcome",
            "at_ms": self.started.elapsed().as_millis() as u64,
            "events": self.events,
            "outcome": outcome,
            "usage": usage,
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(path: &Path) -> Vec<serde_json::Value> {
        fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    #[test]
    fn a_call_is_recorded_as_its_request_its_events_and_its_outcome() {
        let dir = tempfile::tempdir().unwrap();
        let log = RequestLog::in_directory(dir.path().join("requests"));
        let mut record = log.begin(
            RequestPurpose::Turn,
            &serde_json::json!({"model": "m", "input": []}),
            None,
        );
        record.event(&serde_json::json!({"type": "response.output_text.delta", "delta": "hi"}));
        let path = record.path().to_path_buf();
        record.ended(
            "completed",
            Some(ProviderUsage {
                input_tokens: 10,
                cached_input_tokens: Some(4),
                output_tokens: 2,
                reasoning_tokens: Some(1),
            }),
        );

        let recorded = lines(&path);
        assert_eq!(recorded.len(), 3);
        assert_eq!(recorded[0]["kind"], "request");
        assert_eq!(recorded[0]["purpose"], "turn");
        assert_eq!(recorded[0]["body"]["model"], "m");
        assert_eq!(recorded[1]["kind"], "event");
        assert_eq!(recorded[1]["event"]["delta"], "hi");
        assert_eq!(recorded[2]["kind"], "outcome");
        assert_eq!(recorded[2]["outcome"], "completed");
        assert_eq!(recorded[2]["events"], 1);
        assert_eq!(recorded[2]["usage"]["cached_input_tokens"], 4);
        assert!(
            path.file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .ends_with("-turn.jsonl"),
            "{}",
            path.display()
        );
    }

    #[test]
    fn two_calls_begun_in_the_same_instant_are_two_records() {
        let dir = tempfile::tempdir().unwrap();
        let log = RequestLog::in_directory(dir.path().to_path_buf());
        let first = log.begin(RequestPurpose::Turn, &serde_json::json!({}), None);
        let second = log
            .clone()
            .begin(RequestPurpose::Turn, &serde_json::json!({}), None);
        assert_ne!(first.path(), second.path());
        first.ended("done", None);
        second.ended("done", None);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);
    }

    #[test]
    fn a_refused_call_records_the_status_and_body() {
        let dir = tempfile::tempdir().unwrap();
        let log = RequestLog::in_directory(dir.path().to_path_buf());
        let mut record = log.begin(RequestPurpose::DocLookup, &serde_json::json!({}), None);
        record.rejected(429, "cooling down");
        let path = record.path().to_path_buf();
        record.ended("error: refused", None);
        let recorded = lines(&path);
        assert_eq!(recorded[1]["kind"], "rejected");
        assert_eq!(recorded[1]["http_status"], 429);
        assert_eq!(recorded[2]["usage"], serde_json::Value::Null);
    }

    #[test]
    fn only_the_newest_records_are_kept() {
        let dir = tempfile::tempdir().unwrap();
        let log = RequestLog::in_directory(dir.path().to_path_buf());
        for i in 0..(KEPT_CALLS + 5) {
            fs::write(
                dir.path()
                    .join(format!("20260101T000000.{i:03}Z-turn.jsonl")),
                "",
            )
            .unwrap();
        }
        log.begin(RequestPurpose::Turn, &serde_json::json!({}), None)
            .ended("completed", None);
        let kept = fs::read_dir(dir.path()).unwrap().count();
        assert_eq!(kept, KEPT_CALLS);
        assert!(!dir.path().join("20260101T000000.000Z-turn.jsonl").exists());
        assert!(dir.path().join("20260101T000000.064Z-turn.jsonl").exists());
    }

    #[test]
    fn an_unwritable_directory_records_nothing_and_does_not_fail() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("not-a-directory");
        fs::write(&file, "").unwrap();
        let log = RequestLog::in_directory(file.join("requests"));
        let mut record = log.begin(RequestPurpose::Turn, &serde_json::json!({}), None);
        record.event(&serde_json::json!({}));
        record.ended("completed", None);
    }

    #[test]
    fn the_credential_is_redacted_from_a_refusal_body_and_from_streamed_events() {
        let dir = tempfile::tempdir().unwrap();
        let log = RequestLog::in_directory(dir.path().to_path_buf());
        let secret = "fake-provider-secret";
        let mut record = log.begin(
            RequestPurpose::Turn,
            &serde_json::json!({"model": "m"}),
            Some(Arc::new(Credential::new(secret.to_string()))),
        );
        record.rejected(
            401,
            &format!("{{\"error\":{{\"message\":\"bad key {secret}\"}}}}"),
        );
        record.event(&serde_json::json!({"type": "error",
            "error": {"message": format!("rejected {secret}"), "headers": [format!("Bearer {secret}")]}}));
        let path = record.path().to_path_buf();
        record.ended("error: refused", None);

        let text = fs::read_to_string(&path).unwrap();
        assert!(!text.contains(secret), "{text}");
        let recorded = lines(&path);
        assert_eq!(
            recorded[1]["body"],
            "{\"error\":{\"message\":\"bad key [REDACTED]\"}}"
        );
        assert_eq!(
            recorded[2]["event"]["error"]["message"],
            "rejected [REDACTED]"
        );
        assert_eq!(
            recorded[2]["event"]["error"]["headers"][0],
            "Bearer [REDACTED]"
        );
    }
}

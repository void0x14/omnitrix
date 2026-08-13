//! Kanıt deposu: aşama kanıtlarının kalıcı JSONL kaydı (makine + insan okur).

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use super::definition::ArtifactId;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FlowRecord {
    pub seq: u64,
    pub artifact: String,
    pub ok: bool,
    pub detail: String,
}

pub struct FlowStore {
    path: PathBuf,
    records: Vec<FlowRecord>,
}

const FLOW_EVENTS_FILE: &str = "flow_events.jsonl";

impl FlowStore {
    pub fn open(session_dir: &Path) -> Self {
        let path = session_dir.join(FLOW_EVENTS_FILE);
        let records = std::fs::read_to_string(&path)
            .map(|s| {
                s.lines()
                    .filter_map(|l| serde_json::from_str::<FlowRecord>(l).ok())
                    .collect()
            })
            .unwrap_or_default();
        Self { path, records }
    }

    pub fn record(&mut self, artifact: ArtifactId, ok: bool, detail: String) {
        let seq = self.records.len() as u64 + 1;
        let rec = FlowRecord {
            seq,
            artifact: artifact.as_str().to_string(),
            ok,
            detail,
        };
        if let Ok(mut f) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            let _ = writeln!(f, "{}", serde_json::to_string(&rec).unwrap_or_default());
        }
        self.records.push(rec);
    }

    pub fn has(&self, artifact: ArtifactId) -> bool {
        self.records
            .iter()
            .any(|r| r.artifact == artifact.as_str() && r.ok)
    }

    pub fn progress(&self) -> Vec<FlowRecord> {
        self.records.clone()
    }
}

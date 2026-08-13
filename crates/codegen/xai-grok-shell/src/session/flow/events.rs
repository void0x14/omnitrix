//! Akış olay yayını: unified_log + flow_events.jsonl (FlowStore ile aynı dosya).
//! Arayüz gürültüsü üretmez; denetim kaydı üretir.

use std::path::Path;

use super::definition::StageId;

pub struct FlowEvents {
    session_dir: std::path::PathBuf,
}

impl FlowEvents {
    pub fn new(session_dir: &Path) -> Self {
        Self {
            session_dir: session_dir.to_path_buf(),
        }
    }

    fn emit(&self, tag: &str, payload: serde_json::Value) {
        xai_grok_telemetry::unified_log::info(
            tag,
            None,
            Some(serde_json::json!({ "flow": true, "detail": payload })),
        );
        let path = self.session_dir.join("flow_events.jsonl");
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            use std::io::Write;
            let _ = writeln!(
                f,
                "{}",
                serde_json::json!({
                    "event": tag, "detail": payload
                })
            );
        }
    }

    pub fn phase_changed(&self, stage: StageId) {
        self.emit(
            "flow.phase_changed",
            serde_json::json!({ "stage": stage.as_str() }),
        );
    }

    pub fn checkpoint_rejected(&self, stage: &str, reason: &str) {
        self.emit(
            "flow.checkpoint_rejected",
            serde_json::json!({ "stage": stage, "reason": reason }),
        );
    }

    pub fn violation(&self, tool: &str, reason: &str) {
        self.emit(
            "flow.violation",
            serde_json::json!({ "tool": tool, "reason": reason }),
        );
    }

    pub fn completed(&self) {
        self.emit("flow.completed", serde_json::json!({}));
    }
}

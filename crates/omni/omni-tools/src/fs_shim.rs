//! Diff akisi fs-shim'i (MASTER-PLAN 5.2).
//!
//! `AgentBuilder::with_fs` kancasindan gecen her dosya dokunusu once alt
//! katmana devredilir, sonra akisa yazilir. Kapsam uyarisi: yalnizca
//! `SessionContext.fs` uzerinden gecen tool'lar (read/write/edit/apply_patch)
//! gorunur; terminal uzerinden yapilan yazmalar bu dikisi atlar.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use xai_grok_tools::computer::types::{AsyncFileSystem, ComputerError};

use crate::diff::{FileTouch, line_delta};

/// Dokunus akisinin gonderici ucu.
pub type TouchSink = UnboundedSender<FileTouch>;

/// Dokunus akisinin alici ucu; TUI/WebUI ve event-log bunu tuketir.
pub type TouchStream = UnboundedReceiver<FileTouch>;

/// Yeni bir dokunus akisi kanali.
pub fn touch_channel() -> (TouchSink, TouchStream) {
    unbounded_channel()
}

/// Alt katmani sarmalayip her yazma/silme islemini akisa bildiren dosya sistemi.
pub struct DiffShimFs {
    inner: Arc<dyn AsyncFileSystem>,
    workspace_root: PathBuf,
    sink: TouchSink,
    prompt_index: usize,
}

impl DiffShimFs {
    /// `inner` gercek dosya sistemi, `workspace_root` dizin-disi tespiti icin.
    pub fn new(inner: Arc<dyn AsyncFileSystem>, workspace_root: PathBuf, sink: TouchSink) -> Self {
        Self {
            inner,
            workspace_root,
            sink,
            prompt_index: 0,
        }
    }

    /// Dokunusu hangi ajan turuna atfedecegimizi belirler.
    pub fn with_prompt_index(mut self, prompt_index: usize) -> Self {
        self.prompt_index = prompt_index;
        self
    }

    /// Yol calisma dizininin disinda mi?
    fn outside_workspace(&self, path: &Path) -> bool {
        !path.starts_with(&self.workspace_root)
    }

    /// Akisa yaz. Alici dusmusse akis sessizce durur; dosya islemi engellenmez.
    fn emit(&self, touch: FileTouch) {
        if self.sink.send(touch).is_err() {
            tracing::debug!("diff akisi alicisi kapali, dokunus dusuruldu");
        }
    }

    /// Onceki icerigi en iyi cabayla okur; dosya yoksa bos kabul edilir.
    async fn snapshot_before(&self, path: &Path) -> Vec<u8> {
        self.inner.read_file(path).await.unwrap_or_default()
    }

    fn record(&self, path: &Path, before: &[u8], after: &[u8]) {
        let baseline = String::from_utf8_lossy(before);
        let current = String::from_utf8_lossy(after);
        let (added, removed) = line_delta(path, &baseline, &current, self.prompt_index);
        self.emit(FileTouch::new(
            path.to_path_buf(),
            self.outside_workspace(path),
            added,
            removed,
        ));
    }
}

#[async_trait::async_trait]
impl AsyncFileSystem for DiffShimFs {
    async fn read_file(&self, path: &Path) -> Result<Vec<u8>, ComputerError> {
        self.inner.read_file(path).await
    }

    async fn write_file(&self, path: &Path, data: &[u8]) -> Result<(), ComputerError> {
        let before = self.snapshot_before(path).await;
        self.inner.write_file(path, data).await?;
        self.record(path, &before, data);
        Ok(())
    }

    async fn delete_file(&self, path: &Path) -> Result<(), ComputerError> {
        let before = self.snapshot_before(path).await;
        self.inner.delete_file(path).await?;
        self.record(path, &before, &[]);
        Ok(())
    }
}

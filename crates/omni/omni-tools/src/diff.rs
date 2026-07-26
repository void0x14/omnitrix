//! Dosya dokunusu muhasebesi (MASTER-PLAN 5.2).
//!
//! `+/-` satir sayiminin TEK gercek kaynagi `xai_hunk_tracker::diff::compute_hunks`
//! olmalidir; burada kendi diff algoritmamiz yoktur.

use std::path::{Path, PathBuf};

use xai_hunk_tracker::{Hunk, HunkSource};

/// Tek bir dosya dokunusunun ozeti — `0008` semasindaki `file_touches`
/// satirinin bellek karsiligi.
#[derive(Debug, Clone)]
pub struct FileTouch {
    /// Dokunulan yol (shim'e geldigi haliyle).
    pub path: PathBuf,
    /// Calisma dizininin disinda mi? Dizin-disi dokunus kullaniciya gorunur.
    pub outside_workspace: bool,
    /// Eklenen satir sayisi.
    pub added: usize,
    /// Silinen satir sayisi.
    pub removed: usize,
    /// Onceki icerigin CAS atfi (yazilmadiysa `None`).
    pub pre_ref: Option<String>,
    /// Sonraki icerigin CAS atfi (yazilmadiysa `None`).
    pub post_ref: Option<String>,
}

impl FileTouch {
    /// Sayimlari verilmis, CAS atiflari henuz baglanmamis bir dokunus.
    pub fn new(path: PathBuf, outside_workspace: bool, added: usize, removed: usize) -> Self {
        Self {
            path,
            outside_workspace,
            added,
            removed,
            pre_ref: None,
            post_ref: None,
        }
    }

    /// Icerik atiflarini baglar (omni-storage CAS yazimindan sonra).
    pub fn with_refs(mut self, pre_ref: Option<String>, post_ref: Option<String>) -> Self {
        self.pre_ref = pre_ref;
        self.post_ref = post_ref;
        self
    }
}

/// `(eklenen, silinen)` satir sayimi.
///
/// Uyari: `compute_hunks` sessizce bos vec dondurebilir (ayni icerik, 1 MiB
/// ustu dosya, zaman asimi). Bos sonuc "degisiklik yok" anlamina GELMEZ.
pub fn line_delta(
    path: &Path,
    baseline: &str,
    current: &str,
    prompt_index: usize,
) -> (usize, usize) {
    let hunks: Vec<Hunk> = xai_hunk_tracker::diff::compute_hunks(
        path,
        baseline,
        current,
        HunkSource::AgentEdit { prompt_index },
    );
    let added = hunks.iter().map(|h| h.line_info.new_count).sum();
    let removed = hunks.iter().map(|h| h.line_info.old_count).sum();
    (added, removed)
}

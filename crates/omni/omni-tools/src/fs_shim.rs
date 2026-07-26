//! Diff akisi fs-shim'i (MASTER-PLAN 5.2 / 9.3).
//!
//! `AgentBuilder::with_fs` kancasindan gecen her dosya dokunusu once alt
//! katmana devredilir, sonra akisa yazilir. Kapsam uyarisi: yalnizca
//! `SessionContext.fs` uzerinden gecen tool'lar (read/write/edit/apply_patch)
//! gorunur; terminal uzerinden yapilan yazmalar bu dikisi atlar.
//!
//! Kanca dosya sistemi trait'i duzeyinde durdugu icin path-agnostiktir: yol
//! calisma dizininin disinda olsa bile islem engellenmez, yalnizca
//! `outside_workspace` bayragiyla isaretlenip kullaniciya gorunur kilinir
//! (K5 — gorunurluk vardir, kisitlama yoktur).
//!
//! I6: bu modulde panik yoktur. Alt katman hatalari `ComputerError` olarak
//! oldugu gibi cagirana geri verilir; muhasebe (CAS/diff/akis) hatalari ise
//! dosya islemini asla dusurmez, yalnizca kayda gecer.

use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use omni_storage::cas::CasBlobStore;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use xai_grok_tools::computer::types::{AsyncFileSystem, ComputerError};

use crate::diff::FileTouch;

/// Dokunus akisinin gonderici ucu.
pub type TouchSink = UnboundedSender<FileTouch>;

/// Dokunus akisinin alici ucu; TUI/WebUI ve event-log bunu tuketir.
pub type TouchStream = UnboundedReceiver<FileTouch>;

/// Yeni bir dokunus akisi kanali.
pub fn touch_channel() -> (TouchSink, TouchStream) {
    unbounded_channel()
}

/// `.` ve `..` bilesenlerini diske dokunmadan eriten sozel sadelestirme.
///
/// Mutlak yollarda kok bileseni korunur; kok'un ustune cikmaya calisan `..`
/// bilesenleri yutulur.
fn lexical_normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Alt katmani sarmalayip her yazma/silme islemini akisa bildiren dosya sistemi.
///
/// Yazim yolu her zaman su sirayla ilerler:
/// 1. yazim oncesi icerik okunur (dosya yoksa "yok" kabul edilir) -> `pre_ref`
/// 2. islem alt katmana devredilir
/// 3. yazim sonrasi icerik CAS'a yazilir -> `post_ref`
/// 4. `+/-` sayimi `crate::diff` uzerinden alinir
/// 5. `FileTouch` uretilip kurucuda alinan kanaldan yayinlanir
pub struct DiffShimFs {
    inner: Arc<dyn AsyncFileSystem>,
    workspace_root: PathBuf,
    cas: Option<CasBlobStore>,
    sink: TouchSink,
    prompt_index: usize,
}

impl DiffShimFs {
    /// `inner` gercek dosya sistemi, `workspace_root` dizin-disi tespiti icin.
    ///
    /// `workspace_root` kurulusta bir kez gercek yola cevrilir; cevrilemezse
    /// (dizin henuz yoksa) verildigi haliyle kullanilir.
    pub fn new(inner: Arc<dyn AsyncFileSystem>, workspace_root: PathBuf, sink: TouchSink) -> Self {
        let workspace_root =
            dunce::canonicalize(&workspace_root).unwrap_or_else(|_| workspace_root.clone());
        Self {
            inner,
            workspace_root,
            cas: None,
            sink,
            prompt_index: 0,
        }
    }

    /// Icerik atiflarinin yazilacagi CAS deposunu baglar.
    ///
    /// Baglanmazsa dokunuslar `pre_ref`/`post_ref` alanlari `None` olarak akar;
    /// sayim ve akis yine calisir.
    pub fn with_cas(mut self, cas: CasBlobStore) -> Self {
        self.cas = Some(cas);
        self
    }

    /// Dokunusu hangi ajan turuna atfedecegimizi belirler.
    pub fn with_prompt_index(mut self, prompt_index: usize) -> Self {
        self.prompt_index = prompt_index;
        self
    }

    /// Calisma dizini koku (gercek yola cevrilmis haliyle).
    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    /// Yol calisma dizininin disinda mi?
    ///
    /// Yol once mutlaklastirilir, sonra var olan en uzun onegi uzerinden
    /// gercek yola cevrilir (symlink kacislari boylece yakalanir); kalan
    /// bilesenler sozel olarak sadelestirilir. Kok'un ALTINDA degilse `true`.
    pub fn outside_workspace(&self, path: &Path) -> bool {
        !self.resolve(path).starts_with(&self.workspace_root)
    }

    /// Yolu, var olmayan bilesenleri de tolere ederek gercek yola cevirir.
    fn resolve(&self, path: &Path) -> PathBuf {
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.workspace_root.join(path)
        };
        // Once `.`/`..` sozel olarak eritilir: aksi halde `kok/../komsu` gibi
        // bir kacis yol, bilesen karsilastirmasinda kok'un altinda gorunur.
        let absolute = lexical_normalize(&absolute);

        // Var olan en uzun onek bulunana kadar sondan bilesen kirp; sozel
        // sadelestirmeden sonra tum bilesenler normaldir, dolayisiyla
        // `file_name()` kok disinda daima doludur ve dongu sonlanir.
        let mut cursor = absolute.as_path();
        let mut tail: Vec<OsString> = Vec::new();
        let mut anchor = loop {
            match dunce::canonicalize(cursor) {
                Ok(real) => break real,
                Err(_) => match (cursor.file_name(), cursor.parent()) {
                    (Some(name), Some(parent)) => {
                        tail.push(name.to_os_string());
                        cursor = parent;
                    }
                    _ => break absolute.clone(),
                },
            }
        };

        // Kirpilan bilesenleri geri ekle.
        for name in tail.iter().rev() {
            anchor.push(name);
        }
        anchor
    }

    /// Akisa yaz. Alici dusmusse akis sessizce durur; dosya islemi engellenmez.
    fn emit(&self, touch: FileTouch) {
        if self.sink.send(touch).is_err() {
            tracing::debug!(
                prompt_index = self.prompt_index,
                "diff akisi alicisi kapali, dokunus dusuruldu"
            );
        }
    }

    /// Onceki icerigi en iyi cabayla okur; dosya yoksa `None`.
    async fn snapshot_before(&self, path: &Path) -> Option<Vec<u8>> {
        self.inner.read_file(path).await.ok()
    }

    /// Icerigi CAS'a yazip atfini dondurur. CAS bagli degilse veya yazim
    /// basarisizsa `None` doner — muhasebe hatasi dosya islemini dusurmez.
    async fn store_blob(&self, data: &[u8]) -> Option<String> {
        let cas = self.cas.clone()?;
        let payload = data.to_vec();
        match tokio::task::spawn_blocking(move || cas.store(&payload, true)).await {
            Ok(Ok(reference)) => Some(reference),
            Ok(Err(err)) => {
                tracing::warn!(error = %err, "CAS yazimi basarisiz, atif dusuruldu");
                None
            }
            Err(err) => {
                tracing::warn!(error = %err, "CAS gorevi tamamlanamadi, atif dusuruldu");
                None
            }
        }
    }

    /// Yazim oncesi/sonrasi ikilisinden bir dokunus kaydi uretip yayinlar.
    async fn record(&self, path: &Path, before: Option<&[u8]>, after: Option<&[u8]>) {
        let pre_ref = match before {
            Some(bytes) => self.store_blob(bytes).await,
            None => None,
        };
        let post_ref = match after {
            Some(bytes) => self.store_blob(bytes).await,
            None => None,
        };

        let baseline = String::from_utf8_lossy(before.unwrap_or_default()).into_owned();
        let current = String::from_utf8_lossy(after.unwrap_or_default()).into_owned();
        let (added, removed) = crate::diff::count_hunks(&baseline, &current);

        let touch = FileTouch::new(
            path.to_path_buf(),
            self.outside_workspace(path),
            // Sayim genisligi diff modulunun sozlesmesine birakilir; tasma
            // pratikte imkansiz, yine de I6 geregi panik yerine sifire duser.
            added.try_into().unwrap_or_default(),
            removed.try_into().unwrap_or_default(),
        )
        .with_refs(pre_ref, post_ref);

        self.emit(touch);
    }
}

#[async_trait::async_trait]
impl AsyncFileSystem for DiffShimFs {
    /// Okuma dokunus uretmez; hata `ErrorKind` korunarak aynen gecer.
    async fn read_file(&self, path: &Path) -> Result<Vec<u8>, ComputerError> {
        self.inner.read_file(path).await
    }

    /// Yazim: once eski icerik, sonra devir, sonra yeni icerik ve sayim.
    async fn write_file(&self, path: &Path, data: &[u8]) -> Result<(), ComputerError> {
        let before = self.snapshot_before(path).await;
        self.inner.write_file(path, data).await?;
        self.record(path, before.as_deref(), Some(data)).await;
        Ok(())
    }

    /// Silme: sonrasi "icerik yok" kabul edilir, `post_ref` bos kalir.
    async fn delete_file(&self, path: &Path) -> Result<(), ComputerError> {
        let before = self.snapshot_before(path).await;
        self.inner.delete_file(path).await?;
        self.record(path, before.as_deref(), None).await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// Bellek ici sahte dosya sistemi — testler diske dokunmaz.
    #[derive(Default)]
    struct MemFs {
        files: Mutex<HashMap<PathBuf, Vec<u8>>>,
    }

    impl MemFs {
        fn locked(&self) -> Result<std::sync::MutexGuard<'_, HashMap<PathBuf, Vec<u8>>>, ComputerError>
        {
            self.files
                .lock()
                .map_err(|_| ComputerError::io("test fs kilidi zehirlendi"))
        }
    }

    #[async_trait::async_trait]
    impl AsyncFileSystem for MemFs {
        async fn read_file(&self, path: &Path) -> Result<Vec<u8>, ComputerError> {
            let files = self.locked()?;
            files.get(path).cloned().ok_or_else(|| {
                ComputerError::io_with_kind("yok", std::io::ErrorKind::NotFound)
            })
        }

        async fn write_file(&self, path: &Path, data: &[u8]) -> Result<(), ComputerError> {
            let mut files = self.locked()?;
            files.insert(path.to_path_buf(), data.to_vec());
            Ok(())
        }

        async fn delete_file(&self, path: &Path) -> Result<(), ComputerError> {
            let mut files = self.locked()?;
            files.remove(path);
            Ok(())
        }
    }

    fn shim(root: PathBuf) -> (DiffShimFs, TouchStream) {
        let (sink, stream) = touch_channel();
        (DiffShimFs::new(Arc::new(MemFs::default()), root, sink), stream)
    }

    #[tokio::test]
    async fn yazim_dokunus_yayinlar() {
        let root = std::env::temp_dir();
        let (fs, mut stream) = shim(root.clone());
        let path = root.join("omni-fs-shim-test.txt");

        let written = fs.write_file(&path, b"bir\niki\n").await;
        assert!(written.is_ok());

        let touch = stream.try_recv();
        assert!(touch.is_ok());
        if let Ok(touch) = touch {
            assert_eq!(touch.path, path);
            assert!(!touch.outside_workspace);
            // CAS baglanmadigi icin atiflar bostur.
            assert!(touch.pre_ref.is_none());
            assert!(touch.post_ref.is_none());
        }
    }

    #[tokio::test]
    async fn silme_dokunus_yayinlar() {
        let root = std::env::temp_dir();
        let (fs, mut stream) = shim(root.clone());
        let path = root.join("omni-fs-shim-test-2.txt");

        assert!(fs.write_file(&path, b"veri\n").await.is_ok());
        let _ = stream.try_recv();
        assert!(fs.delete_file(&path).await.is_ok());

        let touch = stream.try_recv();
        assert!(touch.is_ok());
    }

    #[test]
    fn dizin_disi_yol_isaretlenir() {
        let root = std::env::temp_dir().join("omni-workspace-kok");
        let (sink, _stream) = touch_channel();
        let fs = DiffShimFs::new(Arc::new(MemFs::default()), root.clone(), sink);

        assert!(!fs.outside_workspace(&root.join("src/main.rs")));
        assert!(fs.outside_workspace(Path::new("/etc/hosts")));
        assert!(fs.outside_workspace(&root.join("../komsu/dosya.txt")));
    }

    #[test]
    fn goreli_yol_kok_altinda_sayilir() {
        let root = std::env::temp_dir().join("omni-workspace-kok");
        let (sink, _stream) = touch_channel();
        let fs = DiffShimFs::new(Arc::new(MemFs::default()), root, sink);

        assert!(!fs.outside_workspace(Path::new("alt/dizin/dosya.rs")));
    }
}

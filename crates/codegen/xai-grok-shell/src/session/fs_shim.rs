//! Diff akisi fs-shim'i (MASTER-PLAN 5.2 / 9.3) — omni-tools'tan tasindi.
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
//!
//! ## Omni-tools'tan tasima notu
//!
//! `omni-tools::fs_shim::DiffShimFs` ve `omni-tools::diff::FileTouch` birebir
//! tasindi; `omni-storage::cas::CasBlobStore` yerine ayni moduldeki sade
//! `CasBlobStore` kullanilir (blake3 atifli, zstd'siz dosya deposu). Hunk
//! sayimi `xai_hunk_tracker` uzerinden yapilir — `+/-` sayiminin tek gercek
//! kaynagi degismedi.

use std::ffi::OsString;
use std::fs;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use xai_grok_tools::computer::types::{AsyncFileSystem, ComputerError};
use xai_hunk_tracker::{FileHunkData, Hunk, HunkSource};

// ============================================================================
// CAS (icerik atiflari) — omni-storage tasimasi, zstd'siz
// ============================================================================

/// CAS islem hatasi. Dosya islemini dusurmeyen muhasebe hatasidir; yalnizca
/// kayda gecer (I6).
#[derive(Debug, thiserror::Error)]
#[error("CAS: {0}")]
pub struct CasError(String);

/// blake3 atifli, klasor bazli basit icerik deposu.
///
/// `store` icerigi blake3 ile ozetler ve ozetle adlandirilmis dosyaya yazar;
/// `load` ozetten dosyayi bulup okur. Omni-storage'dan tasinirken zstd
/// sikistirmasi ATLANDI: yazim duz dosyadir, atif birebir ayni sekilde
/// calisir (I6 — davranis korunur).
#[derive(Debug, Clone)]
pub struct CasBlobStore {
    base_path: PathBuf,
}

/// Ozeti `base` altinda 2-2 yuvali dizin yapisine oturtur:
/// `<base>/<ilk2>/<sonraki2>/<kalan>`. Tasima oncesi omni-storage duzeniyle
/// aynidir; uzanti atlanir (sikistirma yok).
fn hash_to_path(hash: &str, base: &Path) -> PathBuf {
    let (prefix, rest) = hash.split_at(2.min(hash.len()));
    let (mid, file) = if rest.len() > 2 {
        rest.split_at(2)
    } else {
        (rest, "")
    };
    let mut dir = base.join(prefix);
    if !mid.is_empty() {
        dir = dir.join(mid);
    }
    let name = if file.is_empty() { prefix } else { file };
    dir.join(name)
}

impl CasBlobStore {
    /// Depo kokunu kurar; dizin yoksa kendisi olusturur.
    ///
    /// Cagri yolu `~/.grok/omnitrix-cas/` gibi bir yoldur. Dizin kurulamazsa
    /// `Err` doner — cagiran (spawn) akisi atifsiz surdurur (I6).
    pub fn new(base_path: &Path) -> Result<Self, CasError> {
        fs::create_dir_all(base_path).map_err(|e| {
            CasError(format!(
                "depo dizini kurulamadi ({}): {e}",
                base_path.display()
            ))
        })?;
        Ok(Self {
            base_path: base_path.to_path_buf(),
        })
    }

    /// Icerigi blake3 ile ozetler, dosyaya yazar ve atfi dondurur.
    ///
    /// `compress` omni-storage imzasindan tasima geregi korunur; zstd
    /// atlandigi icin yok sayilir.
    pub fn store(&self, data: &[u8], _compress: bool) -> Result<String, CasError> {
        let hash = blake3::hash(data).to_hex().to_string();
        let path = hash_to_path(&hash, &self.base_path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                CasError(format!("CAS dizini kurulamadi ({}): {e}", parent.display()))
            })?;
        }
        let mut f = fs::File::create(&path)
            .map_err(|e| CasError(format!("CAS dosyasi acilamadi ({}): {e}", path.display())))?;
        f.write_all(data)
            .map_err(|e| CasError(format!("CAS yazim hatasi ({}): {e}", path.display())))?;
        Ok(hash)
    }

    /// Atiftan icerigi okur; yoksa `Ok(None)`.
    pub fn load(&self, hash: &str) -> Result<Option<Vec<u8>>, CasError> {
        let path = hash_to_path(hash, &self.base_path);
        if !path.exists() {
            return Ok(None);
        }
        let mut f = fs::File::open(&path)
            .map_err(|e| CasError(format!("CAS dosyasi acilamadi ({}): {e}", path.display())))?;
        let mut raw = Vec::new();
        f.read_to_end(&mut raw)
            .map_err(|e| CasError(format!("CAS okuma hatasi ({}): {e}", path.display())))?;
        Ok(Some(raw))
    }

    /// Atiftan dosyayi siler; yoksa sessizce basarili sayilir.
    pub fn delete(&self, hash: &str) -> Result<(), CasError> {
        let path = hash_to_path(hash, &self.base_path);
        if path.exists() {
            fs::remove_file(&path)
                .map_err(|e| CasError(format!("CAS silme hatasi ({}): {e}", path.display())))?;
        }
        Ok(())
    }
}

// ============================================================================
// Dokunus muhasebesi (FileTouch + +/- sayimi) — omni-tools::diff tasimasi
// ============================================================================

/// Bellekteki iki metni karsilastirirken `Hunk.path`'e yazilan yer tutucu.
///
/// `compute_hunks` bu yolu DISKTEN OKUMAZ; yalnizca uretilen hunk'in etiketine
/// ve log satirina gider.
const IN_MEMORY_PATH: &str = "<memory>";

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

    /// Icerik atiflarini baglar (CAS yazimindan sonra).
    pub fn with_refs(mut self, pre_ref: Option<String>, post_ref: Option<String>) -> Self {
        self.pre_ref = pre_ref;
        self.post_ref = post_ref;
        self
    }

    /// Hunk izleyicisinden gelen dosya verisinden dokunus kurar.
    ///
    /// `FileHunkData.hunks` yolundan gelen hunk'larda `patch` doludur; burada
    /// yalnizca `line_info` sayimlari kullanilir (`summary()` gosterim icindir).
    /// Dikkat: `FileHunkData::default()` ile "dosya izlenmiyor" durumu ayirt
    /// edilemez; ikisi de `(0, 0)` verir.
    pub fn from_hunk_data(path: PathBuf, outside_workspace: bool, data: &FileHunkData) -> Self {
        let (added, removed) = hunk_data_delta(data);
        Self::new(path, outside_workspace, added, removed)
    }

    /// `file_touches` sutunlarina yazilacak `(eklenen, silinen)` cifti.
    ///
    /// Sema `INTEGER` tuttugu icin sayimlar 32 bit'e sikistirilir; tasma
    /// panige degil doyuma gider (I6).
    pub fn counts_u32(&self) -> (u32, u32) {
        (clamp_u32(self.added), clamp_u32(self.removed))
    }

    /// Hicbir satir degismemis mi? Dikkat: `compute_hunks` sessizce bos
    /// donebildigi icin bu "degisiklik yok" GARANTISI vermez (bkz. [`line_delta`]).
    pub fn is_empty_delta(&self) -> bool {
        self.added == 0 && self.removed == 0
    }
}

/// `usize` sayimini tasma yasamadan `u32`'ye indirger.
fn clamp_u32(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

/// Hunk listesinden `(eklenen, silinen)` toplamini cikarir.
///
/// `line_info.new_count` = `+` satir, `line_info.old_count` = `-` satir.
/// Toplama `saturating_add` ile yapilir; patolojik girdi tasma paniki uretmez.
pub fn hunks_delta(hunks: &[Hunk]) -> (usize, usize) {
    hunks.iter().fold((0usize, 0usize), |(added, removed), h| {
        (
            added.saturating_add(h.line_info.new_count),
            removed.saturating_add(h.line_info.old_count),
        )
    })
}

/// Hunk izleyicisinin dosya verisinden `(eklenen, silinen)` toplami.
///
/// `FileHunkData.hunks` alani `Vec<Arc<Hunk>>` oldugu icin [`hunks_delta`]
/// dilim imzasina uymaz; toplama burada tekrarlanir.
pub fn hunk_data_delta(data: &FileHunkData) -> (usize, usize) {
    data.hunks
        .iter()
        .fold((0usize, 0usize), |(added, removed), h| {
            (
                added.saturating_add(h.line_info.new_count),
                removed.saturating_add(h.line_info.old_count),
            )
        })
}

/// Atfi cagiranin sectigi genel sayim yolu.
fn line_delta_with_source(
    path: &Path,
    baseline: &str,
    current: &str,
    source: HunkSource,
) -> (usize, usize) {
    let hunks: Vec<Hunk> = xai_hunk_tracker::diff::compute_hunks(path, baseline, current, source);
    hunks_delta(&hunks)
}

/// Iki metin arasindaki `(eklenen, silinen)` satir sayimi.
///
/// Disk yolu olmayan (bellekte tutulan) icerikler icin kisa yol; sayim yine
/// `xai_hunk_tracker` uzerinden yapilir. Atif bilgisi tasinmadigi icin sonuc
/// [`HunkSource::External`] ile etiketlenir — etiket sayimi etkilemez.
///
/// Uyari: `pre == post`, 1 MiB ustu girdi veya diff zaman asiminda sonuc
/// `(0, 0)`'dir; bu "degisiklik yok" GARANTISI degildir.
pub fn count_hunks(pre: &str, post: &str) -> (u32, u32) {
    let (added, removed) =
        line_delta_with_source(Path::new(IN_MEMORY_PATH), pre, post, HunkSource::External);
    (clamp_u32(added), clamp_u32(removed))
}

// ============================================================================
// DiffShimFs — omni-tools::fs_shim tasimasi (birebir)
// ============================================================================

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
/// 4. `+/-` sayimi [`count_hunks`] uzerinden alinir
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

    /// Dokunusu hangi ajan turune atfedecegimizi belirler.
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
        let (added, removed) = count_hunks(&baseline, &current);

        let touch = FileTouch::new(
            path.to_path_buf(),
            self.outside_workspace(path),
            // Sayim genisligi `count_hunks` sozlesmesine birakilir; tasma
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
        fn locked(
            &self,
        ) -> Result<std::sync::MutexGuard<'_, HashMap<PathBuf, Vec<u8>>>, ComputerError> {
            self.files
                .lock()
                .map_err(|_| ComputerError::io("test fs kilidi zehirlendi"))
        }
    }

    #[async_trait::async_trait]
    impl AsyncFileSystem for MemFs {
        async fn read_file(&self, path: &Path) -> Result<Vec<u8>, ComputerError> {
            let files = self.locked()?;
            files
                .get(path)
                .cloned()
                .ok_or_else(|| ComputerError::io_with_kind("yok", std::io::ErrorKind::NotFound))
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
        (
            DiffShimFs::new(Arc::new(MemFs::default()), root, sink),
            stream,
        )
    }

    #[tokio::test]
    async fn yazim_dokunus_yayinlar() {
        let root = std::env::temp_dir();
        let (fs, mut stream) = shim(root.clone());
        let path = root.join("fs-shim-test.txt");

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
        let path = root.join("fs-shim-test-2.txt");

        assert!(fs.write_file(&path, b"veri\n").await.is_ok());
        let _ = stream.try_recv();
        assert!(fs.delete_file(&path).await.is_ok());

        let touch = stream.try_recv();
        assert!(touch.is_ok());
    }

    #[test]
    fn dizin_disi_yol_isaretlenir() {
        let root = std::env::temp_dir().join("workspace-kok");
        let (sink, _stream) = touch_channel();
        let fs = DiffShimFs::new(Arc::new(MemFs::default()), root.clone(), sink);

        assert!(!fs.outside_workspace(&root.join("src/main.rs")));
        assert!(fs.outside_workspace(Path::new("/etc/hosts")));
        assert!(fs.outside_workspace(&root.join("../komsu/dosya.txt")));
    }

    #[test]
    fn goreli_yol_kok_altinda_sayilir() {
        let root = std::env::temp_dir().join("workspace-kok");
        let (sink, _stream) = touch_channel();
        let fs = DiffShimFs::new(Arc::new(MemFs::default()), root, sink);

        assert!(!fs.outside_workspace(Path::new("alt/dizin/dosya.rs")));
    }

    #[test]
    fn cas_duz_yazim_okuma_cevrimi() {
        let tmp = tempfile::tempdir().expect("temp dizin");
        let cas = CasBlobStore::new(tmp.path()).expect("CAS kurulur");

        let reference = cas.store(b"icerik", true).expect("yazim basarili");
        assert_eq!(reference.len(), 64, "blake3 hex ozeti 64 karakterdir");

        let loaded = cas.load(&reference).expect("okuma basarili");
        assert_eq!(loaded.as_deref(), Some(b"icerik".as_slice()));

        assert!(
            cas.load("yok-boyle-bir-ozet")
                .expect("yok olan None doner")
                .is_none()
        );
    }

    #[test]
    fn cas_duplike_icerik_ayni_atfi_verir() {
        let tmp = tempfile::tempdir().expect("temp dizin");
        let cas = CasBlobStore::new(tmp.path()).expect("CAS kurulur");

        let first = cas.store(b"ayni-icerik", false).expect("ilk yazim");
        let second = cas.store(b"ayni-icerik", false).expect("ikinci yazim");
        assert_eq!(
            first, second,
            "ozet tabanli CAS ayni icerige ayni atfi verir"
        );
    }
}

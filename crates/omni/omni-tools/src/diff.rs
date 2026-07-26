//! Dosya dokunusu muhasebesi (MASTER-PLAN 9.3 / 5.2).
//!
//! `+/-` satir sayiminin TEK gercek kaynagi `xai_hunk_tracker::diff::compute_hunks`
//! olmalidir; burada kendi diff algoritmamiz yoktur. Repo-durumu taramasinin
//! is-parcacigi butcesi ise `xai_gix_status`'tan cozulur — MASTER-PLAN 9.3'un
//! "+/- hunk `xai-hunk-tracker` ile, repo-durumu `xai-gix-status`" cumlesi
//! bu modulde birebir karsilanir.
//!
//! I6: bu modulde panik yoktur; tasma `saturating_add` / `try_from` ile emilir.

use std::path::{Path, PathBuf};

use xai_hunk_tracker::{FileHunkData, Hunk, HunkSource};

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

    /// Icerik atiflarini baglar (omni-storage CAS yazimindan sonra).
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

/// Bir dokunusun kime atfedilecegini belirler (MASTER-PLAN 9.3 gorunurluk).
///
/// - Ajanin kendi turunda yazdigi dosya → [`HunkSource::AgentEdit`].
/// - Ajanin daha once dokundugu dosyaya disaridan gelen degisiklik →
///   [`HunkSource::ExternalEditOnAgentFile`].
/// - Ajanin hic dokunmadigi dosya → [`HunkSource::External`].
pub fn attribution(agent_file: bool, prompt_index: Option<usize>) -> HunkSource {
    match (agent_file, prompt_index) {
        (_, Some(prompt_index)) => HunkSource::AgentEdit { prompt_index },
        (true, None) => HunkSource::ExternalEditOnAgentFile,
        (false, None) => HunkSource::External,
    }
}

/// `(eklenen, silinen)` satir sayimi.
///
/// Uyari: `compute_hunks` sessizce bos vec dondurebilir (ayni icerik, 1 MiB
/// ustu dosya, zaman asimi). Bos sonuc "degisiklik yok" anlamina GELMEZ.
///
/// Hesap senkrondur (`similar` crate'i); sicak bir tokio calisma zamanindan
/// cagriliyorsa `spawn_blocking` icine alinmalidir.
pub fn line_delta(
    path: &Path,
    baseline: &str,
    current: &str,
    prompt_index: usize,
) -> (usize, usize) {
    line_delta_with_source(
        path,
        baseline,
        current,
        HunkSource::AgentEdit { prompt_index },
    )
}

/// Atfi cagiranin sectigi genel sayim yolu.
pub fn line_delta_with_source(
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

/// Repo-durumu taramasinin is-parcacigi butcesi (MASTER-PLAN 9.3).
///
/// `gix` calisma agaci taramasi kaynak acgozludur; butce vendored
/// `xai-gix-status`'tan cozulur. Deger asla 0 degildir — `gix` tarafinda
/// `Some(0)` "sinirsiz" anlamina gelir ve butceyi tamamen devre disi birakirdi.
/// Ust sinir vendored tarafta 8'dir.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RepoStatusBudget {
    threads: usize,
}

impl RepoStatusBudget {
    /// Calisma zamani ortamindan (cekirdek sayisi + yumusak nproc) cozulen butce.
    pub fn resolve() -> Self {
        Self::from_limit(xai_gix_status::compute_gix_status_thread_limit())
    }

    /// Uclu girdiden butce. ORTA parametre `Option<usize>`'tir: yumusak nproc
    /// okunamadiysa `None` gecilir.
    pub fn resolve_from(cores: usize, soft_nproc: Option<usize>, threads_used: usize) -> Self {
        Self::from_limit(xai_gix_status::compute_gix_status_thread_limit_from(
            cores,
            soft_nproc,
            threads_used,
        ))
    }

    /// Vendored hesabin ciktisini normalize eder; 0 asla disari sizmaz.
    fn from_limit(limit: usize) -> Self {
        Self {
            threads: limit.max(1),
        }
    }

    /// Butcelenen is-parcacigi sayisi (her zaman >= 1).
    pub fn threads(self) -> usize {
        self.threads
    }

    /// `gix` `thread_limit` alanina verilecek deger.
    ///
    /// `Some(0)` "sinirsiz" demek oldugu icin bu fonksiyon `Some(0)` uretmez.
    pub fn thread_limit(self) -> Option<usize> {
        Some(self.threads)
    }

    /// Tarama tek parcacikli mi yurutulecek?
    pub fn is_serial(self) -> bool {
        self.threads == 1
    }
}

impl Default for RepoStatusBudget {
    fn default() -> Self {
        Self::resolve()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ayni_icerik_sifir_sayim_verir() {
        assert_eq!(count_hunks("a\nb\n", "a\nb\n"), (0, 0));
    }

    #[test]
    fn saf_ekleme_sadece_arti_sayar() {
        let (added, removed) = count_hunks("a\n", "a\nb\nc\n");
        assert_eq!(removed, 0);
        assert!(added >= 2, "beklenen en az 2 eklenen satir, bulunan {added}");
    }

    #[test]
    fn saf_silme_sadece_eksi_sayar() {
        let (added, removed) = count_hunks("a\nb\nc\n", "a\n");
        assert_eq!(added, 0);
        assert!(
            removed >= 2,
            "beklenen en az 2 silinen satir, bulunan {removed}"
        );
    }

    #[test]
    fn line_delta_ve_count_hunks_ayni_sayimi_verir() {
        let pre = "bir\niki\n";
        let post = "bir\nuc\ndort\n";
        let (a1, r1) = line_delta(Path::new("x.rs"), pre, post, 0);
        let (a2, r2) = count_hunks(pre, post);
        assert_eq!((clamp_u32(a1), clamp_u32(r1)), (a2, r2));
    }

    #[test]
    fn atif_varyantlari_dogru_secilir() {
        assert_eq!(
            attribution(true, Some(3)),
            HunkSource::AgentEdit { prompt_index: 3 }
        );
        assert_eq!(attribution(true, None), HunkSource::ExternalEditOnAgentFile);
        assert_eq!(attribution(false, None), HunkSource::External);
    }

    #[test]
    fn bos_hunk_verisi_sifir_delta() {
        let data = FileHunkData::default();
        assert_eq!(hunk_data_delta(&data), (0, 0));
        let touch = FileTouch::from_hunk_data(PathBuf::from("a.rs"), false, &data);
        assert!(touch.is_empty_delta());
    }

    #[test]
    fn bos_hunk_dilimi_sifir_delta() {
        assert_eq!(hunks_delta(&[]), (0, 0));
    }

    #[test]
    fn dokunus_sayimlari_u32e_sikisir() {
        let touch = FileTouch::new(PathBuf::from("a.rs"), true, 3, 1)
            .with_refs(Some("pre".to_string()), None);
        assert_eq!(touch.counts_u32(), (3, 1));
        assert!(!touch.is_empty_delta());
        assert_eq!(touch.pre_ref.as_deref(), Some("pre"));
        assert_eq!(clamp_u32(usize::MAX), u32::MAX);
    }

    #[test]
    fn butce_asla_sifir_degil() {
        let budget = RepoStatusBudget::resolve();
        assert!(budget.threads() >= 1);
        assert_ne!(budget.thread_limit(), Some(0));

        let tight = RepoStatusBudget::resolve_from(16, Some(20), 10);
        assert!(tight.threads() >= 1);

        let none_nproc = RepoStatusBudget::resolve_from(4, None, 0);
        assert!(none_nproc.threads() >= 1);
        assert_eq!(RepoStatusBudget::default().threads(), budget.threads());
        assert_eq!(budget.is_serial(), budget.threads() == 1);
    }
}

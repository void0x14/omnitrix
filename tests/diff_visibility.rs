//! MASTER-PLAN 5.2 kapisi (Faz 1): "ajan calisma dizini DISINA yazar; dokunus
//! CLI/UI diff akisinda GORUNUR."
//!
//! K5 sozlesmesi: dizin-disi yazim ENGELLENMEZ, yalnizca isaretlenir. Bu dosya
//! tam da bunu olcer — islem basarili olmali VE `outside_workspace = true`
//! bayrakli bir `FileTouch` akisa dusmeli.
//!
//! I1 (girdi / beklenen cikti / esik) her testin basinda acikca yazilidir.
//! Sayim esikleri `xai_hunk_tracker` satir muhasebesine gore TAM esitliktir:
//! bos taban -> N satirlik icerik = `added == N`, `removed == 0`; silme ise
//! `added == 0`, `removed == N`.

use std::path::{Path, PathBuf};

use omni_storage::cas::CasBlobStore;
use omni_tools::AsyncFileSystem;
use omni_tools::diff::FileTouch;
use omni_tools::fs_shim::{DiffShimFs, TouchStream, touch_channel};
use tempfile::TempDir;

/// Uc satirlik ilk icerik: `added == 3` beklenir.
const UC_SATIR: &[u8] = b"alfa\nbeta\ngama\n";
/// Ayni icerige tek satir eklenmis hali: `added == 1`, `removed == 0` beklenir.
const DORT_SATIR: &[u8] = b"alfa\nbeta\ngama\ndelta\n";

/// Kapi kosumu icin kurulum: calisma dizini koku, ONDAN AYRI bir dis dizin ve
/// gercek bir CAS deposu.
///
/// `TempDir` alanlari dusmemek icin tutulur — dusrlerse dizinler silinir.
struct Harness {
    _workspace_dir: TempDir,
    _outside_dir: TempDir,
    _cas_dir: TempDir,
    workspace_root: PathBuf,
    outside_root: PathBuf,
    cas: CasBlobStore,
    fs: DiffShimFs,
    stream: TouchStream,
}

impl Harness {
    fn new() -> Self {
        let workspace_dir = tempfile::tempdir().expect("workspace tempdir");
        let outside_dir = tempfile::tempdir().expect("outside tempdir");
        let cas_dir = tempfile::tempdir().expect("cas tempdir");

        let workspace_root = workspace_dir.path().to_path_buf();
        let outside_root = outside_dir.path().to_path_buf();
        let cas = CasBlobStore::new(cas_dir.path()).expect("cas store");

        let (sink, stream) = touch_channel();
        let fs = DiffShimFs::new(omni_tools::local_fs(), workspace_root.clone(), sink)
            .with_cas(cas.clone())
            .with_prompt_index(0);

        Self {
            _workspace_dir: workspace_dir,
            _outside_dir: outside_dir,
            _cas_dir: cas_dir,
            workspace_root,
            outside_root,
            cas,
            fs,
            stream,
        }
    }

    /// Akista bekleyen bir sonraki dokunus. Kayit `write_file` donmeden once
    /// yayinlandigi icin bloklamayan okuma yeterlidir.
    fn next_touch(&mut self) -> FileTouch {
        self.stream
            .try_recv()
            .expect("dokunus diff akisinda gorunmeliydi")
    }

    /// Akista baska dokunus kalmadigini dogrular.
    fn assert_drained(&mut self) {
        assert!(
            self.stream.try_recv().is_err(),
            "akista beklenmeyen fazladan dokunus var"
        );
    }

    /// CAS atfinin gercekten cozulup verilen icerigi verdigini dogrular.
    fn assert_cas_holds(&self, reference: &str, expected: &[u8]) {
        let blob = self
            .cas
            .load(reference)
            .expect("CAS okumasi hata vermemeli")
            .expect("CAS atfi coplukte olmamali");
        assert_eq!(blob, expected, "CAS icerigi yazilanla ayni olmali");
    }
}

/// Verilen dokunusun beklenen yol/bayrak/sayim uclusunu tasidigini dogrular.
fn assert_touch(touch: &FileTouch, path: &Path, outside: bool, added: u32, removed: u32) {
    assert_eq!(touch.path, path, "dokunus yolu");
    assert_eq!(
        touch.outside_workspace, outside,
        "dizin-disi bayragi ({})",
        touch.path.display()
    );
    assert_eq!(
        (touch.counts_u32().0, touch.counts_u32().1),
        (added, removed),
        "(+{}, -{}) sayimi beklenenden farkli",
        touch.added,
        touch.removed
    );
}

/// KAPI: calisma dizini DISINA yazim.
///
/// Girdi: workspace koku `W`, yazim hedefi `O/rapor.txt` (O != W), icerik 3 satir.
/// Beklenen cikti: yazim BASARILI (K5 — engelleme yok) + akisa tek `FileTouch`
///   dusuyor, `outside_workspace == true`, `+3 / -0`, `pre_ref == None`
///   (dosya yoktu), `post_ref` CAS'ta ve icerigi birebir cozuluyor.
/// Esik: sayimlarda TAM esitlik; dokunus sayisi tam 1.
#[tokio::test]
async fn dizin_disi_yazim_diff_akisinda_gorunur() {
    let mut h = Harness::new();
    let hedef = h.outside_root.join("rapor.txt");

    h.fs.write_file(&hedef, UC_SATIR)
        .await
        .expect("dizin-disi yazim engellenmemeli (K5)");

    let touch = h.next_touch();
    assert_touch(&touch, &hedef, true, 3, 0);
    assert!(
        touch.pre_ref.is_none(),
        "yeni dosyanin yazim-oncesi atfi olmamali"
    );
    let post_ref = touch.post_ref.clone().expect("post_ref CAS'a yazilmaliydi");
    h.assert_cas_holds(&post_ref, UC_SATIR);
    h.assert_drained();

    // Dokunus yalnizca gorunurluk uretir; dosya gercekten diske yazilmis olmali.
    let diskten = std::fs::read(&hedef).expect("dizin-disi dosya diskte olmali");
    assert_eq!(diskten, UC_SATIR);
}

/// KONTROL GRUBU: calisma dizini ICINE yazim.
///
/// Girdi: workspace koku `W`, yazim hedefi `W/src/main.rs`, icerik 3 satir.
/// Beklenen cikti: dokunus yayinlanir ama `outside_workspace == false`.
/// Esik: bayrak TAM `false`; sayimlar `+3 / -0`.
#[tokio::test]
async fn dizin_ici_yazim_disari_isaretlenmez() {
    let mut h = Harness::new();
    let hedef = h.workspace_root.join("src/main.rs");

    h.fs.write_file(&hedef, UC_SATIR)
        .await
        .expect("dizin-ici yazim basarili olmali");

    let touch = h.next_touch();
    assert_touch(&touch, &hedef, false, 3, 0);
    h.assert_drained();
}

/// Ustune yazim: yazim-oncesi icerik `pre_ref` olarak CAS'a girer ve sayim
/// yalnizca farki gosterir.
///
/// Girdi: `O/rapor.txt` once 3 satir, sonra ayni icerik + 1 satir.
/// Beklenen cikti: ikinci dokunus `+1 / -0`, `pre_ref` = ilk icerik,
///   `post_ref` = yeni icerik, `outside_workspace == true`.
/// Esik: sayimlarda TAM esitlik; iki atif da CAS'tan cozulebilir olmali.
#[tokio::test]
async fn dizin_disi_ustune_yazim_farki_sayar() {
    let mut h = Harness::new();
    let hedef = h.outside_root.join("rapor.txt");

    h.fs.write_file(&hedef, UC_SATIR).await.expect("ilk yazim");
    let ilk = h.next_touch();
    let ilk_post = ilk.post_ref.clone().expect("ilk post_ref");

    h.fs.write_file(&hedef, DORT_SATIR)
        .await
        .expect("ikinci yazim");
    let ikinci = h.next_touch();

    assert_touch(&ikinci, &hedef, true, 1, 0);
    let pre_ref = ikinci.pre_ref.clone().expect("pre_ref yazim-oncesi icerik");
    let post_ref = ikinci.post_ref.clone().expect("post_ref yazim-sonrasi icerik");
    // Icerik adresli depo: ayni bayt dizisi ayni atfi vermeli.
    assert_eq!(
        pre_ref, ilk_post,
        "ustune yazimin pre_ref'i onceki post_ref ile ayni olmali"
    );
    h.assert_cas_holds(&pre_ref, UC_SATIR);
    h.assert_cas_holds(&post_ref, DORT_SATIR);
    h.assert_drained();
}

/// Silme: `-` sayiminin dogrulandigi yol.
///
/// Girdi: `O/rapor.txt` 3 satirla yazilir, sonra silinir.
/// Beklenen cikti: silme dokunusu `+0 / -3`, `outside_workspace == true`,
///   `pre_ref` = silinen icerik, `post_ref == None`.
/// Esik: sayimlarda TAM esitlik; dosya diskten gercekten kalkmis olmali.
#[tokio::test]
async fn dizin_disi_silme_eksi_satirlari_sayar() {
    let mut h = Harness::new();
    let hedef = h.outside_root.join("rapor.txt");

    h.fs.write_file(&hedef, UC_SATIR).await.expect("yazim");
    let _ = h.next_touch();

    h.fs.delete_file(&hedef).await.expect("silme");
    let touch = h.next_touch();

    assert_touch(&touch, &hedef, true, 0, 3);
    let pre_ref = touch.pre_ref.clone().expect("silinen icerik CAS'ta olmali");
    h.assert_cas_holds(&pre_ref, UC_SATIR);
    assert!(
        touch.post_ref.is_none(),
        "silme sonrasi icerik yoktur, post_ref bos olmali"
    );
    assert!(!hedef.exists(), "dosya diskten kalkmis olmali");
    h.assert_drained();
}

/// Kok'un ustune tirmanan goreli kacis da dizin-disi sayilir.
///
/// Girdi: workspace koku `W`, hedef `W/../<W-adi>-komsu.txt`.
/// Beklenen cikti: `outside_workspace == true` ve dokunus akisa duser.
/// Esik: bayrak TAM `true`.
#[tokio::test]
async fn parent_kacisi_dizin_disi_sayilir() {
    let mut h = Harness::new();
    let kok_adi = h
        .workspace_root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "kok".to_string());
    let hedef = h.workspace_root.join(format!("../{kok_adi}-komsu.txt"));

    h.fs.write_file(&hedef, UC_SATIR)
        .await
        .expect("kacis yazimi engellenmemeli (K5)");

    let touch = h.next_touch();
    assert!(
        touch.outside_workspace,
        "`..` ile kok disina cikan yol dizin-disi isaretlenmeli: {}",
        touch.path.display()
    );
    assert_eq!(touch.counts_u32(), (3, 0));
    h.assert_drained();

    // Sizinti birakma: kacis dosyasi tempdir'in disinda kaldi, elle temizlenir.
    let _ = std::fs::remove_file(&hedef);
}

/// CAS baglanmadan da akis ve sayim calisir (atiflar bos olur).
///
/// Girdi: CAS'siz shim, dizin-disi yazim, 3 satir.
/// Beklenen cikti: dokunus yayinlanir, `outside_workspace == true`, `+3 / -0`,
///   `pre_ref`/`post_ref` ikisi de `None`.
/// Esik: sayimlarda TAM esitlik; atiflarin ikisi de bos.
#[tokio::test]
async fn cas_bagli_degilken_dokunus_yine_gorunur() {
    let workspace_dir = tempfile::tempdir().expect("workspace tempdir");
    let outside_dir = tempfile::tempdir().expect("outside tempdir");
    let (sink, mut stream) = touch_channel();
    let fs = DiffShimFs::new(
        omni_tools::local_fs(),
        workspace_dir.path().to_path_buf(),
        sink,
    );
    let hedef = outside_dir.path().join("rapor.txt");

    fs.write_file(&hedef, UC_SATIR).await.expect("yazim");

    let touch = stream.try_recv().expect("dokunus akisa dusmeliydi");
    assert_touch(&touch, &hedef, true, 3, 0);
    assert!(touch.pre_ref.is_none() && touch.post_ref.is_none());
}

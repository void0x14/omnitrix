//! K5/AS5 — gecici izolasyon branch'i (MASTER-PLAN Bolum 2, 8.3, 17.3, 17.4).
//!
//! Bu modul bir korumali kafes DEGILDIR; bir **izolasyon + geri-alma sinridir**.
//! Tehlikeli native islemler (computer-use, genis FS yazimi, self-modify) ana
//! calisma agacinda degil, `xai-fast-worktree` ile acilan gecici bir git
//! calisma agacinda ve ona ait gecici bir branch uzerinde kosar:
//!
//! - Is yolunda giderse [`Worktree::promote`] — yalnizca `self_modify` yetkisi
//!   icin verilmis, **kullanici onayli** bir `capability_audit` karari ile —
//!   agacin tam durumunu ana depodaki dayanikli branch'e tasir.
//! - Herhangi bir hata olursa [`Worktree::discard`] (ya da deger dusurulunce
//!   `Drop`) gecici agaci ve branch'i **atar**; ana agac hic dokunulmamis kalir.
//!
//! MASTER-PLAN 17.4: "Calisan binary'ye otomatik merge yok." Bu yuzden
//! `promote` merge YAPMAZ — yalnizca aday branch'i ana depoda dayanikli hale
//! getirir; terfi (merge) kullanicinin isidir, geri-alma = git.
//!
//! I6: bu modulde uretim yolunda `unwrap`/`expect`/`panic!` yoktur; her hata
//! [`ToolsError`] olarak tasinir. `Drop` yolunda hatalar yalnizca loglanir.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use xai_fast_worktree::{
    CreationMode, WorkingTreeMode, WorktreeBuilder, remove_worktree, snapshot_worktree_to_ref,
    transfer_snapshot_to_repo,
};
use xai_tool_runtime::{ToolError, ToolErrorKind};

use crate::broker::CapabilityDecision;
use crate::error::ToolsError;

/// `capability_audit.capability` icin self-modify yetkisinin adi (MASTER-PLAN 17.4).
pub const CAPABILITY_SELF_MODIFY: &str = "self_modify";

/// Gecici izolasyon branch'lerinin ad onu: `omni/k5/<ad>`.
pub const BRANCH_PREFIX: &str = "omni/k5";

/// Terfi anini yakalayan dayanikli aday ref'inin onu: `refs/omni/k5-candidate/<ad>`.
///
/// `refs/heads/` ALTINDA DEGIL: gecici agac kendi branch'ini "checked out"
/// tuttugu icin git ayni branch'e fetch etmeyi reddeder. Aday ref'i ayri bir
/// ad uzayinda durur, bu yuzden tasima her zaman calisir.
pub const CANDIDATE_REF_PREFIX: &str = "refs/omni/k5-candidate";

/// Gecici agaclarin depo ile ayni dosya sisteminde tutuldugu kardes dizin.
///
/// Depo agacinin ICINDE degil (ana agaci kirletmemek icin), ama ayni dosya
/// sisteminde (CoW/reflink kopyasinin calismasi icin).
pub const ISOLATION_DIR: &str = ".omni-k5";

/// Gecici agac adinin ust siniri; git ref adi ve dizin adi olarak kullanilir.
const MAX_NAME_LEN: usize = 64;

/// Gecici izolasyon calisma agaci — `xai-fast-worktree` sarmalayicisi.
///
/// Deger yasadigi surece agac diskte durur. `discard`/`promote` cagrilmadan
/// deger dusurulurse `Drop` guvenlik icin `discard` davranisini uygular:
/// **unutulan bir izolasyon agaci ana agaci kirletemez.**
#[derive(Debug)]
pub struct Worktree {
    /// Kaynak deponun kok dizini (cozulmus yol).
    repo: PathBuf,
    /// Gecici calisma agacinin yolu.
    path: PathBuf,
    /// Cagiranin verdigi kisa ad (dizin + ref adi bileseni).
    name: String,
    /// Gecici branch'in tam adi (`omni/k5/<ad>`).
    branch: String,
    /// Agacin uzerine kuruldugu taban commit'i.
    base_commit: String,
    /// `discard`/`promote` tamamlandiysa `true`; `Drop` bu durumda bir sey yapmaz.
    settled: bool,
}

impl Worktree {
    /// `repo` deposundan `omni/k5/<name>` branch'i ve ona bagli gecici bir
    /// calisma agaci uretir.
    ///
    /// Agac HEAD commit'inden **temiz** (izlenen dosyalar checkout edilmis,
    /// yerel degisiklikler tasinmamis) acilir; boylece izolasyonun tabani
    /// deterministiktir. Islem blokandir; async baglamda `spawn_blocking`
    /// icinden cagrilmalidir.
    pub fn create(repo: &Path, name: &str) -> Result<Self, ToolsError> {
        validate_name(name)?;

        let given = dunce::canonicalize(repo).map_err(|e| {
            ToolsError::InvalidConfig(format!("depo yolu cozulemedi ({}): {e}", repo.display()))
        })?;

        // Alt dizinden cagrilmis olabiliriz; her zaman depo kokune calis.
        let top = git(&given, &["rev-parse", "--show-toplevel"])?;
        let repo_root = dunce::canonicalize(Path::new(top.trim())).map_err(|e| {
            ToolsError::InvalidConfig(format!("depo koku cozulemedi ({}): {e}", top.trim()))
        })?;

        let branch = format!("{BRANCH_PREFIX}/{name}");
        if git_ok(&repo_root, &["show-ref", "--verify", "--quiet", &head_ref(&branch)]) {
            return Err(ToolsError::InvalidConfig(format!(
                "gecici izolasyon branch'i zaten var: {branch}"
            )));
        }

        // Taban commit: HEAD. Commit'i olmayan depoda izolasyon anlamsizdir.
        let base_commit = git(&repo_root, &["rev-parse", "--verify", "HEAD^{commit}"])?
            .trim()
            .to_owned();

        let dest = isolation_root(&repo_root)?.join(name);
        if dest.exists() {
            return Err(ToolsError::InvalidConfig(format!(
                "gecici agac yolu zaten dolu: {}",
                dest.display()
            )));
        }
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                runtime_err(format!(
                    "izolasyon dizini olusturulamadi ({}): {e}",
                    parent.display()
                ))
            })?;
        }

        // Once branch: agac bu ref'e baglanacak.
        git(&repo_root, &["branch", &branch, &base_commit])?;

        let built = WorktreeBuilder::new(&repo_root, &dest)
            .git_ref(&branch)
            .creation_mode(CreationMode::Linked)
            .working_tree_mode(WorkingTreeMode::CleanTracked)
            .create();

        let report = match built {
            Ok(report) => report,
            Err(e) => {
                // Yarim kalan olusturmayi geri sar: ne dizin ne branch kalsin.
                rollback_create(&repo_root, &dest, &branch);
                return Err(runtime_err(format!(
                    "gecici calisma agaci olusturulamadi ({}): {e:#}",
                    dest.display()
                )));
            }
        };

        let worktree = Self {
            repo: repo_root,
            path: report.worktree_path,
            name: name.to_owned(),
            branch,
            base_commit: report.commit,
            settled: false,
        };

        // `xai-fast-worktree` agaci ayrik (detached) HEAD ile acar. Icerik zaten
        // branch commit'i oldugu icin HEAD'i sembolik olarak branch'e baglamak
        // calisma agacina dokunmaz, ama agacta atilan commit'ler branch'i ilerletir.
        git(
            &worktree.path,
            &["symbolic-ref", "HEAD", &head_ref(&worktree.branch)],
        )?;

        tracing::info!(
            repo = %worktree.repo.display(),
            path = %worktree.path.display(),
            branch = %worktree.branch,
            base_commit = %worktree.base_commit,
            "K5 gecici izolasyon agaci acildi"
        );

        Ok(worktree)
    }

    /// Gecici calisma agacinin yolu — tehlikeli islemler burada kosar.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Kaynak deponun koku.
    pub fn repo(&self) -> &Path {
        &self.repo
    }

    /// Gecici branch'in tam adi (`omni/k5/<ad>`).
    pub fn branch(&self) -> &str {
        &self.branch
    }

    /// Cagiranin verdigi kisa ad.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Agacin uzerine kuruldugu taban commit'i.
    pub fn base_commit(&self) -> &str {
        &self.base_commit
    }

    /// Terfi onayinin `capability_audit.target` alaninda tasimasi gereken deger.
    ///
    /// Onay tam olarak BU agaca baglanir; baska bir agac icin verilmis bir karar
    /// burada yeniden kullanilamaz.
    pub fn approval_target(&self) -> &str {
        &self.branch
    }

    /// Gecici agaci ve branch'i **atar**; ana agac bozulmaz (K5 geri-alma).
    ///
    /// Islem blokandir.
    pub fn discard(mut self) -> Result<(), ToolsError> {
        let outcome = self.teardown();
        // Basarisiz olsa bile tekrar denemeyelim: hata cagirana dondu.
        self.settled = true;
        match &outcome {
            Ok(()) => tracing::info!(
                path = %self.path.display(),
                branch = %self.branch,
                "K5 gecici izolasyon agaci atildi"
            ),
            Err(e) => tracing::warn!(
                path = %self.path.display(),
                branch = %self.branch,
                error = %e,
                "K5 gecici izolasyon agaci atilamadi"
            ),
        }
        outcome
    }

    /// Agacin tam durumunu ana depodaki dayanikli `omni/k5/<ad>` branch'ine
    /// tasir — **yalnizca kullanici onayli `self_modify` karari ile**.
    ///
    /// MERGE YAPMAZ (MASTER-PLAN 17.4): ana agac ve ana branch'ler dokunulmaz.
    /// Uretilen tek sey, kullanicinin inceleyip terfi ettirebilecegi aday
    /// branch ve onun dayanikli `refs/omni/k5-candidate/<ad>` yedegidir.
    ///
    /// Islem blokandir.
    pub fn promote(mut self, approval: &PromotionApproval) -> Result<(), ToolsError> {
        if approval.target() != self.branch {
            return Err(ToolsError::Denied(format!(
                "self_modify onayi bu izolasyon branch'i icin verilmemis \
                 (onay hedefi: {}, agac: {})",
                approval.target(),
                self.branch
            )));
        }

        let candidate_ref = format!("{CANDIDATE_REF_PREFIX}/{}", self.name);
        let message = format!(
            "omnitrix K5 aday: {} (taban {}, onaylayan {})",
            self.branch,
            self.base_commit,
            approval.approver()
        );

        // 1) Agacin tam durumunu (commit'lenmemis degisiklikler dahil) bir
        //    commit'e sabitle.
        let commit = snapshot_worktree_to_ref(&self.path, &candidate_ref, &message)
            .map_err(|e| runtime_err(format!("aday anlik goruntusu alinamadi: {e:#}")))?;

        // 2) Ref'i ana depoya tasi. Bu adim gecmeden agac SILINMEZ; aksi halde
        //    aday kaybolabilirdi.
        transfer_snapshot_to_repo(&self.path, &self.repo, &candidate_ref)
            .map_err(|e| runtime_err(format!("aday ref'i ana depoya tasinamadi: {e:#}")))?;

        // Buradan sonrasi dayanikli: bir hata olursa agaci ATMAYIZ (veri kaybi
        // yerine sizinti tercih edilir), yalnizca raporlariz.
        self.settled = true;

        // 3) Gecici agaci kaldir; branch ref'i serbest kalsin.
        remove_worktree(&self.path)
            .map_err(|e| runtime_err(format!("gecici agac kaldirilamadi: {e:#}")))?;
        let _ = git(&self.repo, &["worktree", "prune"]);

        // 4) Aday branch'i ana depoda anlik goruntuye tasi.
        git(
            &self.repo,
            &["update-ref", &head_ref(&self.branch), &commit],
        )?;

        tracing::info!(
            repo = %self.repo.display(),
            branch = %self.branch,
            candidate_ref = %candidate_ref,
            commit = %commit,
            approver = %approval.approver(),
            "K5 aday branch'i dayanikli hale getirildi (merge YOK)"
        );

        Ok(())
    }

    /// Diskteki agaci ve gecici branch'i kaldirir.
    ///
    /// `worktree prune` ve `branch -D` en-iyi-caba: BTRFS anlik goruntusu
    /// yolunda agac bagimsiz bir depo olabilir, o durumda kayit zaten yoktur.
    fn teardown(&self) -> Result<(), ToolsError> {
        remove_worktree(&self.path)
            .map_err(|e| runtime_err(format!("gecici agac kaldirilamadi: {e:#}")))?;
        let _ = git(&self.repo, &["worktree", "prune"]);
        let _ = git(&self.repo, &["branch", "-D", &self.branch]);
        Ok(())
    }
}

impl Drop for Worktree {
    fn drop(&mut self) {
        if self.settled {
            return;
        }
        tracing::warn!(
            path = %self.path.display(),
            branch = %self.branch,
            "K5 izolasyon agaci karara baglanmadan dusuruldu; atiliyor"
        );
        if let Err(e) = self.teardown() {
            tracing::warn!(
                path = %self.path.display(),
                error = %e,
                "K5 izolasyon agaci Drop sirasinda temizlenemedi"
            );
        }
    }
}

/// Terfi bileti: yalnizca **kullanici onayli** bir `self_modify` karari ile
/// uretilebilen kanit (MASTER-PLAN 17.4).
///
/// [`Worktree::promote`] baska turlu cagrilamaz; onay kontrolu tip sisteminde
/// yasar, cagri yerinde tekrarlanmaz.
#[derive(Debug, Clone)]
pub struct PromotionApproval {
    agent_id: i64,
    target: String,
    approver: String,
    ts: String,
}

impl PromotionApproval {
    /// Bir `capability_audit` kararindan bilet uretir.
    ///
    /// Kabul kosullari (hepsi zorunlu):
    /// - karar izin veriyor,
    /// - yetki adi [`CAPABILITY_SELF_MODIFY`],
    /// - kararin bir **onaylayani** var (otomatik karar yetmez).
    pub fn from_decision(decision: &CapabilityDecision) -> Result<Self, ToolsError> {
        if decision.capability != CAPABILITY_SELF_MODIFY {
            return Err(ToolsError::Denied(format!(
                "terfi icin '{CAPABILITY_SELF_MODIFY}' yetkisi gerekir, gelen: '{}'",
                decision.capability
            )));
        }
        if !decision.is_allowed() {
            let gerekce = decision
                .decision
                .reason()
                .unwrap_or("karar izin vermiyor")
                .to_owned();
            return Err(ToolsError::Denied(format!("self_modify reddedildi: {gerekce}")));
        }
        let approver = match decision.approver.as_deref() {
            Some(a) if !a.trim().is_empty() => a.trim().to_owned(),
            _ => {
                return Err(ToolsError::Denied(
                    "self_modify terfisi otomatik kararla yapilamaz: kullanici onayi zorunlu"
                        .to_owned(),
                ));
            }
        };
        Ok(Self {
            agent_id: decision.agent_id,
            target: decision.target.clone(),
            approver,
            ts: decision.ts.clone(),
        })
    }

    /// Onaya konu ajan.
    pub fn agent_id(&self) -> i64 {
        self.agent_id
    }

    /// Onayin bagli oldugu hedef — [`Worktree::approval_target`] ile eslesmeli.
    pub fn target(&self) -> &str {
        &self.target
    }

    /// Onayi veren merci.
    pub fn approver(&self) -> &str {
        &self.approver
    }

    /// Onayin zaman damgasi (RFC 3339).
    pub fn ts(&self) -> &str {
        &self.ts
    }
}

/// Gecici agac adi dogrulamasi.
///
/// Yalnizca ASCII harf/rakam/`-`/`_`. Nokta ve ayirici kabul edilmedigi icin
/// `..`, mutlak yol ve git ref kacislari bastan imkansizdir; bas karakter
/// `-` olamaz ki argumana donusmesin.
fn validate_name(name: &str) -> Result<(), ToolsError> {
    if name.is_empty() {
        return Err(ToolsError::InvalidConfig(
            "izolasyon agaci adi bos olamaz".to_owned(),
        ));
    }
    if name.len() > MAX_NAME_LEN {
        return Err(ToolsError::InvalidConfig(format!(
            "izolasyon agaci adi cok uzun ({} > {MAX_NAME_LEN})",
            name.len()
        )));
    }
    if name.starts_with('-') {
        return Err(ToolsError::InvalidConfig(
            "izolasyon agaci adi '-' ile baslayamaz".to_owned(),
        ));
    }
    if let Some(bad) = name
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || *c == '-' || *c == '_'))
    {
        return Err(ToolsError::InvalidConfig(format!(
            "izolasyon agaci adinda gecersiz karakter: {bad:?}"
        )));
    }
    Ok(())
}

/// `<depo-adi>` icin gecici agaclarin toplandigi kardes dizin.
fn isolation_root(repo_root: &Path) -> Result<PathBuf, ToolsError> {
    let parent = repo_root.parent().ok_or_else(|| {
        ToolsError::InvalidConfig(format!(
            "depo kokunun ust dizini yok, izolasyon agaci yerlestirilemez: {}",
            repo_root.display()
        ))
    })?;
    let repo_name = repo_root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "repo".to_owned());
    Ok(parent.join(ISOLATION_DIR).join(repo_name))
}

/// Branch adini tam ref adina cevirir.
fn head_ref(branch: &str) -> String {
    format!("refs/heads/{branch}")
}

/// Yarim kalan `create` cagrisini geri sarar (en-iyi-caba).
fn rollback_create(repo_root: &Path, dest: &Path, branch: &str) {
    if dest.exists() {
        let _ = remove_worktree(dest);
    }
    let _ = git(repo_root, &["worktree", "prune"]);
    let _ = git(repo_root, &["branch", "-D", branch]);
}

/// Calisma zamani hatasini tool katmaninin hata tipine sarar.
fn runtime_err(detail: impl Into<String>) -> ToolsError {
    ToolsError::Runtime(ToolError::new(ToolErrorKind::Execution, detail))
}

/// `git` calistirir; cikis kodu 0 degilse hata dondurur.
fn git(cwd: &Path, args: &[&str]) -> Result<String, ToolsError> {
    let output = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .output()
        .map_err(|e| runtime_err(format!("git calistirilamadi ({}): {e}", args.join(" "))))?;

    if !output.status.success() {
        return Err(runtime_err(format!(
            "git {} basarisiz ({}): {}",
            args.join(" "),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// `git` calistirir; yalnizca basarili olup olmadigini dondurur.
fn git_ok(cwd: &Path, args: &[&str]) -> bool {
    git(cwd, args).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::broker::Decision;

    fn onay(target: &str) -> CapabilityDecision {
        CapabilityDecision::new(7, CAPABILITY_SELF_MODIFY, target, Decision::Allow)
            .with_approver("void0x14")
    }

    #[test]
    fn ad_dogrulamasi_kacis_denemelerini_reddeder() {
        assert!(validate_name("gorev-42_a").is_ok());
        assert!(validate_name("").is_err());
        assert!(validate_name("../escape").is_err());
        assert!(validate_name("a/b").is_err());
        assert!(validate_name("-rf").is_err());
        assert!(validate_name("nokta.li").is_err());
        assert!(validate_name(&"x".repeat(MAX_NAME_LEN + 1)).is_err());
    }

    #[test]
    fn otomatik_karar_terfi_bileti_uretmez() {
        let karar = CapabilityDecision::new(1, CAPABILITY_SELF_MODIFY, "omni/k5/x", Decision::Allow);
        assert!(PromotionApproval::from_decision(&karar).is_err());
    }

    #[test]
    fn yanlis_yetki_terfi_bileti_uretmez() {
        let karar =
            CapabilityDecision::new(1, "tool", "omni/k5/x", Decision::Allow).with_approver("kim");
        assert!(PromotionApproval::from_decision(&karar).is_err());
    }

    #[test]
    fn red_karari_terfi_bileti_uretmez() {
        let karar = CapabilityDecision::new(
            1,
            CAPABILITY_SELF_MODIFY,
            "omni/k5/x",
            Decision::Deny("kullanici reddetti".to_string()),
        )
        .with_approver("kim");
        assert!(PromotionApproval::from_decision(&karar).is_err());
    }

    #[test]
    fn onayli_karar_bilet_uretir() {
        let bilet = PromotionApproval::from_decision(&onay("omni/k5/x")).expect("bilet");
        assert_eq!(bilet.target(), "omni/k5/x");
        assert_eq!(bilet.approver(), "void0x14");
        assert_eq!(bilet.agent_id(), 7);
    }

    /// Test deposu kurar; `git` yoksa `None` doner (CI ortami degisebilir).
    fn test_deposu(dir: &Path) -> Option<PathBuf> {
        let repo = dir.join("depo");
        std::fs::create_dir_all(&repo).ok()?;
        let calistir = |args: &[&str]| -> bool {
            Command::new("git")
                .current_dir(&repo)
                .args(args)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_SYSTEM", "/dev/null")
                .stdin(Stdio::null())
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
        };
        if !calistir(&["init", "-q", "-b", "ana"]) {
            return None;
        }
        calistir(&["config", "user.email", "t@example.invalid"]);
        calistir(&["config", "user.name", "test"]);
        std::fs::write(repo.join("a.txt"), "bir\n").ok()?;
        calistir(&["add", "-A"]);
        if !calistir(&["commit", "-qm", "ilk"]) {
            return None;
        }
        Some(repo)
    }

    #[test]
    fn olustur_ve_at_ana_agaci_bozmaz() {
        let tmp = match tempfile::tempdir() {
            Ok(t) => t,
            Err(_) => return,
        };
        let Some(repo) = test_deposu(tmp.path()) else {
            return;
        };

        let wt = match Worktree::create(&repo, "gorev1") {
            Ok(wt) => wt,
            // Dosya sistemi CoW/worktree desteklemiyorsa testi atla.
            Err(_) => return,
        };
        assert!(wt.path().join("a.txt").exists());
        assert_eq!(wt.branch(), "omni/k5/gorev1");

        // Ayni ad ikinci kez acilamaz.
        assert!(Worktree::create(&repo, "gorev1").is_err());

        let yol = wt.path().to_path_buf();
        wt.discard().expect("at");
        assert!(!yol.exists());

        // Branch de gitmis olmali, ana agac dosyasi yerinde durmali.
        assert!(!git_ok(
            &repo,
            &["show-ref", "--verify", "--quiet", "refs/heads/omni/k5/gorev1"]
        ));
        assert!(repo.join("a.txt").exists());
    }

    #[test]
    fn terfi_baska_agacin_onayini_kabul_etmez() {
        let tmp = match tempfile::tempdir() {
            Ok(t) => t,
            Err(_) => return,
        };
        let Some(repo) = test_deposu(tmp.path()) else {
            return;
        };
        let wt = match Worktree::create(&repo, "gorev2") {
            Ok(wt) => wt,
            Err(_) => return,
        };

        let yabanci = PromotionApproval::from_decision(&onay("omni/k5/baska")).expect("bilet");
        let hata = wt.promote(&yabanci).expect_err("reddedilmeli");
        assert!(matches!(hata, ToolsError::Denied(_)));
    }

    #[test]
    fn terfi_aday_branch_uretir_merge_etmez() {
        let tmp = match tempfile::tempdir() {
            Ok(t) => t,
            Err(_) => return,
        };
        let Some(repo) = test_deposu(tmp.path()) else {
            return;
        };
        let wt = match Worktree::create(&repo, "gorev3") {
            Ok(wt) => wt,
            Err(_) => return,
        };

        // Agacta commit'lenmemis bir degisiklik birak.
        std::fs::write(wt.path().join("a.txt"), "iki\n").expect("yaz");
        let branch = wt.branch().to_owned();
        let bilet = PromotionApproval::from_decision(&onay(&branch)).expect("bilet");

        let yol = wt.path().to_path_buf();
        wt.promote(&bilet).expect("terfi");

        assert!(!yol.exists());
        assert!(git_ok(
            &repo,
            &["show-ref", "--verify", "--quiet", &head_ref(&branch)]
        ));
        // Ana agac degismedi: merge yok.
        assert_eq!(
            std::fs::read_to_string(repo.join("a.txt")).expect("oku"),
            "bir\n"
        );
        // Aday commit'i degisikligi tasiyor.
        let icerik = git(&repo, &["show", &format!("{branch}:a.txt")]).expect("goster");
        assert_eq!(icerik, "iki\n");
    }
}

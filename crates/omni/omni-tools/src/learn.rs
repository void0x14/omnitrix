//! AS1 — soru-gudumlu kanit getirme (MASTER-PLAN 9.2).
//!
//! Reddedilen iki yol burada TEKRARLANMAZ:
//! - ham dosya okuma yok (context yakiyor),
//! - kalici sembol grafigi / mimari ozet yok (kullaniciya asla grafik sunulmaz).
//!
//! Ucuncu yol: **retrieval, browsing degil**. Girdi dogal-dil bir sorudur;
//! cikti soruya cevap veren MINIMAL span'lardan olusan sirali bir kanit
//! paketidir. Her span'in yaninda tek satirlik "neden ilgili" gerekcesi ve
//! bir devam kursoru bulunur; hicbir zaman tum dosya donmez.
//!
//! `xai-codebase-graph` burada YALNIZCA dahili ve gecici bir indekstir:
//! varsayilan olarak diske onbellek yazmaz, disariya grafik olarak sizmaz,
//! sadece hangi span'larin okunacagini secmek icin kullanilir.
//!
//! Cikti `grok_build_hashline::AnchorScheme` ile uretilmis anchor tasir
//! (5.3 / 9.4). Boylece retrieval→edit zinciri kapanir: `hashline_edit`
//! donen anchor'i dogrudan tuketebilir. Dosya kaymissa [`LearnTool::refresh_anchor`]
//! `validate` + `find_shifted` ile anchor'i sinirli pencerede yeniden konumlar.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use xai_codebase_graph::{IndexManager, IndexManagerConfig, IndexManagerHandle, ScopeGraphIndex};
use xai_grok_tools::implementations::grok_build_hashline::anchor::split_lines;
use xai_grok_tools::implementations::grok_build_hashline::config::HashlineSchemeParams;
use xai_grok_tools::implementations::grok_build_hashline::scheme::{
    AnchorScheme, DEFAULT_SEARCH_RADIUS, ParsedAnchor, ShiftResult, ValidationResult,
};

use crate::error::ToolsError;

// ---------------------------------------------------------------------------
// Tavanlar — context yakmamak SART. Her sinir burada tek noktada tanimli.
// ---------------------------------------------------------------------------

/// Tek bir span'in donebilecegi EN FAZLA satir sayisi.
pub const MAX_SPAN_LINES: usize = 40;
/// Bir kanit paketindeki en fazla span sayisi.
pub const MAX_SPANS_PER_PACK: usize = 6;
/// Bir kanit paketinin toplam satir tavani (tum span'lar dahil).
pub const MAX_PACK_LINES: usize = 160;
/// Span basina yukari dogru toplanabilecek doc-yorum/oznitelik satiri sayisi.
const MAX_CONTEXT_ABOVE: usize = 4;
/// Okunacak dosya boyu tavani; bunun ustundeki dosyalar atlanir.
const MAX_FILE_BYTES: u64 = 1 << 20;
/// Indeksten taranacak en fazla sembol (fuzzy eslesme icin).
const MAX_SYMBOL_SCAN: usize = 4096;
/// Sayfalama icin tutulan en fazla aday.
const MAX_CANDIDATES: usize = 128;
/// Bir sembol icin en fazla kac kullanim yeri aday olur.
const MAX_REF_SITES: usize = 3;
/// Bu esikten daha cok referansi olan sembol "genel" sayilir; kullanim
/// yerleri kanit olarak tasinmaz (gurultu yapar).
const HOT_SYMBOL_REFS: usize = 200;
/// Sorudan cikarilan en fazla terim.
const MAX_TERMS: usize = 8;
/// Bir terimin anlamli sayilmasi icin en kisa uzunluk.
const MIN_TERM_LEN: usize = 3;
/// "Neden ilgili" satirinin karakter tavani (tek satir, kisa).
const WHY_MAX_CHARS: usize = 160;

// Puanlama — sirali kanit paketi deterministik olmali.
const SCORE_EXACT_DEF: u32 = 100;
const SCORE_FUZZY_DEF: u32 = 55;
const SCORE_REF_SITE: u32 = 30;
const SCORE_PATH_HIT: u32 = 25;
const SCORE_REF_BOOST_CAP: u32 = 20;

/// Soruda tasiyici olmayan kelimeler (TR + EN). Terim cikariminda elenir.
const STOPWORDS: &[&str] = &[
    "a", "an", "and", "are", "bir", "bu", "but", "by", "da", "de", "der", "did", "do", "does",
    "for", "from", "gibi", "hangi", "how", "icin", "in", "is", "it", "kim", "mi", "mu", "nasil",
    "ne", "neden", "nerde", "nerede", "nereden", "nereye", "niye", "not", "of", "on", "or", "the",
    "to", "ve", "veya", "was", "what", "when", "where", "which", "who", "why", "with", "ya",
    "yoksa",
];

// ---------------------------------------------------------------------------
// Cikti tipleri
// ---------------------------------------------------------------------------

/// 1-tabanli, iki ucu dahil satir araligi.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineRange {
    /// Ilk satir (1-tabanli, dahil).
    pub start: usize,
    /// Son satir (1-tabanli, dahil).
    pub end: usize,
}

impl LineRange {
    /// Aralikta kac satir var.
    pub fn len(&self) -> usize {
        self.end.saturating_sub(self.start) + 1
    }

    /// Aralik bos mu (uretimde asla olmamali, tamlik icin).
    pub fn is_empty(&self) -> bool {
        self.end < self.start
    }

    /// Iki aralik kesisiyor mu.
    fn overlaps(&self, other: &Self) -> bool {
        self.start <= other.end && other.start <= self.end
    }
}

/// Kanit paketinde derinlesme kursoru.
///
/// Kursor sadece AYNI soru icin gecerlidir: `question_key` sorunun parmak
/// izidir, farkli bir soruyla gelen kursor reddedilir.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cursor {
    /// Sorunun normalize edilmis parmak izi.
    pub question_key: String,
    /// Siradaki adayin sirasi (0-tabanli).
    pub offset: usize,
}

impl Cursor {
    /// Kursoru tek satirlik metne cevirir (`"<key>@<offset>"`).
    pub fn render(&self) -> String {
        format!("{}@{}", self.question_key, self.offset)
    }

    /// [`Cursor::render`] ciktisini geri cozer. Bozuksa `None`.
    pub fn parse(text: &str) -> Option<Self> {
        let (key, offset) = text.rsplit_once('@')?;
        if key.is_empty() {
            return None;
        }
        let offset: usize = offset.parse().ok()?;
        Some(Self {
            question_key: key.to_owned(),
            offset,
        })
    }
}

/// Soruya cevap veren tek bir minimal kanit parcasi.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceSpan {
    /// Depo kokune gore yol.
    pub path: String,
    /// Kirpilmis satir araligi (tavan: [`MAX_SPAN_LINES`]).
    pub line_range: LineRange,
    /// TEK satirlik gerekce: bu span soruyla neden ilgili.
    pub why_relevant: String,
    /// Span'in ilk satirinin hashline anchor'i (5.3) — `hashline_edit` tuketir.
    pub anchor: String,
    /// Bu span'in ardindan devam etmek icin kursor.
    pub cursor: Cursor,
}

/// Bir sorunun cevabi: sirali kanit paketi.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidencePack {
    /// Sirali span'lar (en ilgili once).
    pub spans: Vec<EvidenceSpan>,
    /// Daha derine inmek icin kursor; kanit bittiyse `None`.
    pub next_cursor: Option<Cursor>,
    /// Cevaplanan soru (aynen).
    pub question: String,
}

impl EvidencePack {
    /// Pakette hic kanit var mi.
    pub fn is_empty(&self) -> bool {
        self.spans.is_empty()
    }

    /// Paketin toplam satir maliyeti.
    pub fn total_lines(&self) -> usize {
        self.spans.iter().map(|s| s.line_range.len()).sum()
    }
}

/// Daha once verilmis bir anchor'in guncel dosyaya gore durumu.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AnchorStatus {
    /// Anchor hala gecerli; satir numarasi degismedi.
    Fresh {
        /// Gecerli 1-tabanli satir.
        line: usize,
    },
    /// Anchor sinirli pencerede kaymis halde bulundu.
    Shifted {
        /// Yeni 1-tabanli satir.
        line: usize,
    },
    /// Birden fazla aday satir dogruladi; karar verilemedi.
    Ambiguous {
        /// Aday 1-tabanli satirlar.
        candidates: Vec<usize>,
    },
    /// Satir duruyor ama icerigi degismis, pencerede de bulunamadi.
    Stale,
    /// Satir numarasi dosyanin disinda.
    OutOfRange,
}

// ---------------------------------------------------------------------------
// Tool
// ---------------------------------------------------------------------------

/// Kod ogrenme tool'u: soru → sirali kanit paketi.
///
/// Indeks TEMBEL kurulur; ilk [`LearnTool::ask`] cagrisinda arka planda
/// baslar. Handle `Arc` ile tutulur, `LearnTool` dusunce indeks kanali da
/// kapanir — bu yuzden ayrica `shutdown` cagrilmaz (ayni depo icin baska bir
/// tuketicinin handle'i paylasiliyor olabilir).
pub struct LearnTool {
    /// Depo koku (dunce ile normalize edilmis mutlak yol).
    root: PathBuf,
    /// Dahili, gecici kod indeksi.
    index: tokio::sync::OnceCell<Arc<IndexManagerHandle>>,
    /// Anchor semasi — `registry.rs` hashline demetiyle AYNI parametreler.
    scheme: Box<dyn AnchorScheme>,
    /// Indeksin diske onbellek yazip yazmayacagi (varsayilan: hayir).
    cache_on_disk: bool,
}

impl std::fmt::Debug for LearnTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LearnTool")
            .field("root", &self.root)
            .field("scheme", &self.scheme.name())
            .field("index_started", &self.index.initialized())
            .finish()
    }
}

impl LearnTool {
    /// Depo kokunden yeni bir tool kurar.
    ///
    /// Anchor semasi `HashlineSchemeParams` varsayilanidir; bu deger
    /// `registry.rs` icindeki hashline demetiyle birebir aynidir (chunk/3/8),
    /// yoksa uretilen anchor'lari `hashline_edit` tuketemezdi.
    pub fn new(root: impl AsRef<Path>) -> Result<Self, ToolsError> {
        Self::with_scheme_params(root, HashlineSchemeParams::default())
    }

    /// Anchor sema parametrelerini disaridan vererek kurar.
    pub fn with_scheme_params(
        root: impl AsRef<Path>,
        params: HashlineSchemeParams,
    ) -> Result<Self, ToolsError> {
        // I: Path::canonicalize degil, dunce::canonicalize.
        let root = dunce::canonicalize(root.as_ref()).map_err(|e| {
            ToolsError::InvalidConfig(format!(
                "kod ogrenme koku cozulemedi ({}): {e}",
                root.as_ref().display()
            ))
        })?;
        let scheme = params.build_scheme().map_err(ToolsError::InvalidConfig)?;
        Ok(Self {
            root,
            index: tokio::sync::OnceCell::new(),
            scheme,
            cache_on_disk: false,
        })
    }

    /// Indeksin diske onbellek yazmasina izin verir.
    ///
    /// Varsayilan KAPALIDIR: 9.2'ye gore indeks dahili ve gecicidir, kullanici
    /// bakimini ustlenecegi kalici bir yapiyla karsilasmamalidir.
    pub fn with_disk_cache(mut self, enabled: bool) -> Self {
        self.cache_on_disk = enabled;
        self
    }

    /// Depo koku.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Soru sorar, sirali kanit paketi doner.
    ///
    /// `cursor` verilirse ayni sorunun bir sonraki diliminden devam edilir.
    pub async fn ask(&self, q: &str, cursor: Option<Cursor>) -> Result<EvidencePack, ToolsError> {
        let terms = extract_terms(q);
        if terms.is_empty() {
            return Err(ToolsError::InvalidConfig(
                "soru arama terimi icermiyor; en az bir tanimlayici/kelime gerekli".to_owned(),
            ));
        }
        let question_key = question_key(&terms);

        let offset = match cursor {
            Some(c) if c.question_key != question_key => {
                return Err(ToolsError::InvalidConfig(
                    "kursor baska bir soruya ait; ayni soruyla devam edilmeli".to_owned(),
                ));
            }
            Some(c) => c.offset,
            None => 0,
        };

        let snapshot = self.snapshot().await?;
        let candidates = rank_candidates(snapshot.as_ref(), &terms);

        let mut spans: Vec<EvidenceSpan> = Vec::new();
        let mut used_lines = 0usize;
        let mut taken: HashMap<String, Vec<LineRange>> = HashMap::new();
        let mut files: HashMap<String, Option<Arc<FileView>>> = HashMap::new();
        let mut next_offset = offset.min(candidates.len());

        for (i, cand) in candidates.iter().enumerate().skip(offset) {
            if spans.len() >= MAX_SPANS_PER_PACK {
                break;
            }
            let Some(view) = self.file_view(&cand.path, &mut files).await else {
                next_offset = i + 1;
                continue;
            };
            let lines = split_lines(&view.content);
            let Some(range) = carve_span(&lines, cand.line) else {
                next_offset = i + 1;
                continue;
            };
            // Ayni dosyada kesisen span tekrar tekrar dondurulmez.
            if taken
                .get(&cand.path)
                .is_some_and(|rs| rs.iter().any(|r| r.overlaps(&range)))
            {
                next_offset = i + 1;
                continue;
            }
            // Paket satir tavani asilacaksa bu adayi TUKETMEDEN dur; bir
            // sonraki cagri ayni yerden devam eder.
            if used_lines + range.len() > MAX_PACK_LINES && !spans.is_empty() {
                next_offset = i;
                break;
            }
            let Some(anchor) = view.anchors.get(range.start.saturating_sub(1)).cloned() else {
                next_offset = i + 1;
                continue;
            };

            used_lines += range.len();
            taken.entry(cand.path.clone()).or_default().push(range);
            spans.push(EvidenceSpan {
                path: cand.path.clone(),
                line_range: range,
                why_relevant: cand.why(),
                anchor,
                cursor: Cursor {
                    question_key: question_key.clone(),
                    offset: i + 1,
                },
            });
            next_offset = i + 1;
        }

        let next_cursor = if next_offset < candidates.len() && !spans.is_empty() {
            Some(Cursor {
                question_key,
                offset: next_offset,
            })
        } else {
            None
        };

        Ok(EvidencePack {
            spans,
            next_cursor,
            question: q.to_owned(),
        })
    }

    /// Daha once verilmis bir anchor'i guncel dosyaya gore dogrular.
    ///
    /// Retrieval→edit zincirinin ikinci halkasi: dosya kanit paketinden sonra
    /// degistiyse anchor `find_shifted` ile SINIRLI pencerede yeniden
    /// konumlanir (drift-tolerant, 9.4).
    pub async fn refresh_anchor(
        &self,
        path: &str,
        anchor: &str,
    ) -> Result<AnchorStatus, ToolsError> {
        let parsed = ParsedAnchor::parse(anchor)
            .ok_or_else(|| ToolsError::InvalidConfig(format!("bozuk anchor: {anchor}")))?;
        let resolved = self.resolve_path(path).ok_or_else(|| {
            ToolsError::InvalidConfig(format!("yol depo kokunun disinda ya da yok: {path}"))
        })?;
        let content = tokio::fs::read_to_string(&resolved)
            .await
            .map_err(|e| ToolsError::FileSystem(e.into()))?;
        let lines = split_lines(&content);

        Ok(match self.scheme.validate(&parsed, &lines) {
            ValidationResult::Valid => AnchorStatus::Fresh { line: parsed.line },
            ValidationResult::Stale | ValidationResult::OutOfRange => {
                match self
                    .scheme
                    .find_shifted(&parsed, &lines, DEFAULT_SEARCH_RADIUS)
                {
                    ShiftResult::Found { new_line } => AnchorStatus::Shifted { line: new_line },
                    ShiftResult::Ambiguous { candidates } => AnchorStatus::Ambiguous { candidates },
                    ShiftResult::NotFound => {
                        if parsed.line > lines.len() {
                            AnchorStatus::OutOfRange
                        } else {
                            AnchorStatus::Stale
                        }
                    }
                }
            }
        })
    }

    /// Dahili indeks anlik goruntusu. Disariya ASLA sizmaz — yalnizca hangi
    /// span'larin okunacagini secmek icin kullanilir.
    async fn snapshot(&self) -> Result<Arc<ScopeGraphIndex>, ToolsError> {
        let handle = self
            .index
            .get_or_init(|| async {
                let mut config = IndexManagerConfig::new(self.root.clone());
                if !self.cache_on_disk {
                    config = config.without_cache_load().without_cache_save();
                }
                IndexManager::spawn(config)
            })
            .await;
        handle.get_snapshot_async().await.map_err(|_| {
            ToolsError::InvalidConfig("kod indeksi kapandi; kanit uretilemiyor".to_owned())
        })
    }

    /// Goreli yolu depo koku icinde cozer. Kok disina cikan yol reddedilir.
    fn resolve_path(&self, rel: &str) -> Option<PathBuf> {
        let joined = self.root.join(rel);
        let resolved = dunce::canonicalize(joined).ok()?;
        resolved.starts_with(&self.root).then_some(resolved)
    }

    /// Dosyayi (bir `ask` cagrisi boyunca) tek sefer okur ve anchor'larini uretir.
    async fn file_view(
        &self,
        rel: &str,
        cache: &mut HashMap<String, Option<Arc<FileView>>>,
    ) -> Option<Arc<FileView>> {
        if let Some(hit) = cache.get(rel) {
            return hit.clone();
        }
        let view = self.load_file(rel).await;
        cache.insert(rel.to_owned(), view.clone());
        view
    }

    async fn load_file(&self, rel: &str) -> Option<Arc<FileView>> {
        let resolved = self.resolve_path(rel)?;
        let meta = tokio::fs::metadata(&resolved).await.ok()?;
        if !meta.is_file() || meta.len() > MAX_FILE_BYTES {
            return None;
        }
        let content = tokio::fs::read_to_string(&resolved).await.ok()?;
        let anchors = {
            let lines = split_lines(&content);
            self.scheme
                .generate_anchors(&lines)
                .iter()
                .map(|a| a.render())
                .collect::<Vec<_>>()
        };
        Some(Arc::new(FileView { content, anchors }))
    }
}

/// Bir dosyanin `ask` suresince tutulan okunmus hali.
struct FileView {
    content: String,
    /// Satir basina hazir anchor metni (0-tabanli dizi, 1-tabanli satir).
    anchors: Vec<String>,
}

// ---------------------------------------------------------------------------
// Aday secimi ve siralama
// ---------------------------------------------------------------------------

/// Bir adayin soruya neden bagli oldugu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MatchKind {
    /// Sembol tanimi, terimle birebir eslesti.
    ExactDefinition,
    /// Sembol tanimi, terim sembol adinin icinde geciyor.
    FuzzyDefinition,
    /// Sembolun kullanildigi yer.
    ReferenceSite,
    /// Dosya yolu terimle eslesti.
    PathHit,
}

/// Siralamaya giren tek bir kanit adayi.
#[derive(Debug, Clone)]
struct Candidate {
    path: String,
    line: usize,
    symbol: String,
    term: String,
    kind: MatchKind,
    score: u32,
    refs: usize,
}

impl Candidate {
    /// TEK satirlik "neden ilgili" gerekcesi.
    fn why(&self) -> String {
        let text = match self.kind {
            MatchKind::ExactDefinition => format!(
                "'{}' terimi {} tanimiyla birebir eslesti ({} kullanim)",
                self.term, self.symbol, self.refs
            ),
            MatchKind::FuzzyDefinition => format!(
                "{} tanimi '{}' terimini iceriyor ({} kullanim)",
                self.symbol, self.term, self.refs
            ),
            MatchKind::ReferenceSite => format!(
                "{} burada kullaniliyor; '{}' teriminin izi",
                self.symbol, self.term
            ),
            MatchKind::PathHit => format!(
                "dosya yolu '{}' ile eslesti; modulun giris bolgesi",
                self.term
            ),
        };
        one_line(&text)
    }
}

/// Indeksten soruya gore adaylari toplar ve deterministik siralar.
fn rank_candidates(index: &ScopeGraphIndex, terms: &[String]) -> Vec<Candidate> {
    let mut best: HashMap<(String, usize), Candidate> = HashMap::new();

    let mut offer = |cand: Candidate| {
        let key = (cand.path.clone(), cand.line);
        match best.get(&key) {
            Some(existing) if existing.score >= cand.score => {}
            _ => {
                best.insert(key, cand);
            }
        }
    };

    // 1) Birebir sembol eslesmesi — terimin isim varyantlari uzerinden.
    for term in terms {
        for variant in name_variants(term) {
            for (path, line) in index.find_definitions(&variant) {
                offer(Candidate {
                    path: path.to_owned(),
                    line,
                    symbol: variant.clone(),
                    term: term.clone(),
                    kind: MatchKind::ExactDefinition,
                    score: SCORE_EXACT_DEF,
                    refs: 0,
                });
            }
        }
    }

    // 2) Fuzzy: terim sembol adinin icinde geciyorsa tanim + (sinirli) kullanim.
    for (name, refs) in index.top_referenced_symbols(MAX_SYMBOL_SCAN) {
        let lowered = name.to_lowercase();
        let Some(term) = terms
            .iter()
            .find(|t| t.len() >= MIN_TERM_LEN && lowered.contains(t.as_str()))
        else {
            continue;
        };
        let boost = u32::try_from(refs).unwrap_or(u32::MAX).min(SCORE_REF_BOOST_CAP);
        for (path, line) in index.find_definitions(&name) {
            offer(Candidate {
                path: path.to_owned(),
                line,
                symbol: name.clone(),
                term: term.clone(),
                kind: MatchKind::FuzzyDefinition,
                score: SCORE_FUZZY_DEF + boost,
                refs,
            });
        }
        // Cok yaygin semboller kanit degil gurultu uretir; kullanim yerleri atlanir.
        if refs > HOT_SYMBOL_REFS {
            continue;
        }
        for (path, line) in index.find_references(&name).into_iter().take(MAX_REF_SITES) {
            offer(Candidate {
                path: path.to_owned(),
                line,
                symbol: name.clone(),
                term: term.clone(),
                kind: MatchKind::ReferenceSite,
                score: SCORE_REF_SITE,
                refs,
            });
        }
    }

    // 3) Dosya yolu eslesmesi — "auth nerede" gibi modul sorulari icin.
    for path in index.indexed_files() {
        let lowered = path.to_lowercase();
        if let Some(term) = terms
            .iter()
            .find(|t| t.len() >= MIN_TERM_LEN && lowered.contains(t.as_str()))
        {
            offer(Candidate {
                path: path.to_owned(),
                line: 1,
                symbol: path.to_owned(),
                term: term.clone(),
                kind: MatchKind::PathHit,
                score: SCORE_PATH_HIT,
                refs: 0,
            });
        }
    }

    let mut out: Vec<Candidate> = best.into_values().collect();
    // Deterministik siralama: puan ↓, kullanim ↓, yol ↑, satir ↑.
    out.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| b.refs.cmp(&a.refs))
            .then_with(|| a.path.cmp(&b.path))
            .then_with(|| a.line.cmp(&b.line))
    });
    out.truncate(MAX_CANDIDATES);
    out
}

// ---------------------------------------------------------------------------
// Soru isleme
// ---------------------------------------------------------------------------

/// Dogal-dil sorudan arama terimlerini cikarir.
///
/// Tokenlar kucuk harfe indirilir; `snake_case` ve `camelCase` parcalanir,
/// hem butun token hem alt kelimeler terim olur.
fn extract_terms(question: &str) -> Vec<String> {
    let mut terms: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    let push = |candidate: String, terms: &mut Vec<String>, seen: &mut HashSet<String>| {
        if candidate.len() < MIN_TERM_LEN || STOPWORDS.contains(&candidate.as_str()) {
            return;
        }
        if seen.insert(candidate.clone()) {
            terms.push(candidate);
        }
    };

    for raw in question.split(|c: char| !(c.is_alphanumeric() || c == '_')) {
        if raw.is_empty() {
            continue;
        }
        push(raw.to_lowercase(), &mut terms, &mut seen);
        for part in split_identifier(raw) {
            push(part, &mut terms, &mut seen);
        }
        if terms.len() >= MAX_TERMS {
            break;
        }
    }
    terms.truncate(MAX_TERMS);
    terms
}

/// `snake_case` / `camelCase` / `PascalCase` tanimlayicisini alt kelimelere ayirir.
fn split_identifier(ident: &str) -> Vec<String> {
    let mut parts: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut prev_lower = false;

    for ch in ident.chars() {
        if ch == '_' || ch == '-' {
            if !current.is_empty() {
                parts.push(std::mem::take(&mut current));
            }
            prev_lower = false;
            continue;
        }
        if ch.is_uppercase() && prev_lower && !current.is_empty() {
            parts.push(std::mem::take(&mut current));
        }
        prev_lower = ch.is_lowercase() || ch.is_numeric();
        for lowered in ch.to_lowercase() {
            current.push(lowered);
        }
    }
    if !current.is_empty() {
        parts.push(current);
    }
    parts
}

/// Bir terimin indekste aranacak isim varyantlari: `term`, `snake_case`
/// (aynen), `PascalCase`, `camelCase`, `SCREAMING_CASE`.
fn name_variants(term: &str) -> Vec<String> {
    let mut out = vec![term.to_owned()];
    let words = split_identifier(term);
    if words.is_empty() {
        return out;
    }

    let pascal: String = words.iter().map(|w| capitalize(w)).collect();
    let camel = match words.split_first() {
        Some((head, tail)) => {
            let mut s = head.clone();
            for w in tail {
                s.push_str(&capitalize(w));
            }
            s
        }
        None => term.to_owned(),
    };
    let screaming = words
        .iter()
        .map(|w| w.to_uppercase())
        .collect::<Vec<_>>()
        .join("_");

    for variant in [pascal, camel, screaming, words.join("_")] {
        if !out.contains(&variant) {
            out.push(variant);
        }
    }
    out
}

fn capitalize(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// Sorunun parmak izi — kursorun ayni soruya ait oldugunu dogrular.
/// FNV-1a; kriptografik degil, yalnizca esitlik damgasi.
fn question_key(terms: &[String]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for term in terms {
        for byte in term.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash ^= 0xff;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// Gerekceyi tek satira indirger ve kirpar.
fn one_line(text: &str) -> String {
    let flat: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let trimmed = flat.trim();
    if trimmed.chars().count() <= WHY_MAX_CHARS {
        return trimmed.to_owned();
    }
    let mut out: String = trimmed.chars().take(WHY_MAX_CHARS.saturating_sub(1)).collect();
    out.push('…');
    out
}

// ---------------------------------------------------------------------------
// Span kirpma
// ---------------------------------------------------------------------------

/// `def_line` (1-tabanli) etrafinda MINIMAL span'i kirpar.
///
/// Yukari dogru yalnizca doc-yorum/oznitelik basligi toplanir; asagi dogru
/// kaseli blok dengesi izlenir. Her durumda [`MAX_SPAN_LINES`] tavandir.
fn carve_span(lines: &[&str], def_line: usize) -> Option<LineRange> {
    let total = lines.len();
    if def_line == 0 || def_line > total {
        return None;
    }
    let idx = def_line - 1;

    // Yukari: baslik satirlari (doc-yorum, oznitelik, blok yorum govdesi).
    let mut start = idx;
    let mut climbed = 0usize;
    while start > 0 && climbed < MAX_CONTEXT_ABOVE {
        let Some(prev) = lines.get(start - 1).map(|l| l.trim_start()) else {
            break;
        };
        if is_header_line(prev) {
            start -= 1;
            climbed += 1;
        } else {
            break;
        }
    }

    // Asagi: blok dengesi ya da tavan.
    let limit = total.min(idx + MAX_SPAN_LINES);
    let mut depth: i32 = 0;
    let mut opened = false;
    let mut end = idx;
    for i in idx..limit {
        let Some(line) = lines.get(i) else { break };
        depth += brace_delta(line);
        if depth > 0 {
            opened = true;
        }
        end = i;
        if opened && depth <= 0 {
            break;
        }
        if !opened && i > idx && line.trim().is_empty() {
            break;
        }
    }

    // Sondaki bos satirlari at.
    while end > start && lines.get(end).is_some_and(|l| l.trim().is_empty()) {
        end -= 1;
    }
    // Tavan her kosulda uygulanir.
    if end.saturating_sub(start) + 1 > MAX_SPAN_LINES {
        end = start + MAX_SPAN_LINES - 1;
    }
    if end >= total {
        end = total.saturating_sub(1);
    }
    if end < start {
        end = start;
    }

    Some(LineRange {
        start: start + 1,
        end: end + 1,
    })
}

/// Satir bir tanimin "baslik" satiri mi (doc-yorum / oznitelik / dekorator).
fn is_header_line(trimmed: &str) -> bool {
    trimmed.starts_with("///")
        || trimmed.starts_with("//!")
        || trimmed.starts_with("//")
        || trimmed.starts_with("#[")
        || trimmed.starts_with("#")
        || trimmed.starts_with("@")
        || trimmed.starts_with("*")
        || trimmed.starts_with("/*")
}

/// Satirdaki net kase dengesi; satir yorumlari ve string/char literalleri
/// kabaca atlanir (tam ayristirma degil, span kirpmaya yeter).
fn brace_delta(line: &str) -> i32 {
    let mut delta = 0i32;
    let mut chars = line.chars().peekable();
    let mut in_string = false;
    let mut in_char = false;
    let mut escaped = false;

    while let Some(ch) = chars.next() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' if in_string || in_char => escaped = true,
            '"' if !in_char => in_string = !in_string,
            '\'' if !in_string => in_char = !in_char,
            '/' if !in_string && !in_char && chars.peek() == Some(&'/') => break,
            '{' if !in_string && !in_char => delta += 1,
            '}' if !in_string && !in_char => delta -= 1,
            _ => {}
        }
    }
    delta
}

// ---------------------------------------------------------------------------
// Testler — burada unwrap/expect serbest (I6 uretim yolu icin gecerli).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn write_fixture(dir: &Path) {
        let src = dir.join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(
            src.join("auth.rs"),
            "//! Kimlik dogrulama modulu.\n\
             \n\
             /// Oturum durumu.\n\
             pub struct AuthSession {\n\
             \x20   pub token: String,\n\
             }\n\
             \n\
             /// Oturumu dogrular.\n\
             pub fn verify_auth(session: &AuthSession) -> bool {\n\
             \x20   !session.token.is_empty()\n\
             }\n",
        )
        .unwrap();
        std::fs::write(
            src.join("main.rs"),
            "mod auth;\n\
             \n\
             fn main() {\n\
             \x20   let session = auth::AuthSession { token: String::new() };\n\
             \x20   let ok = auth::verify_auth(&session);\n\
             \x20   println!(\"{ok}\");\n\
             }\n",
        )
        .unwrap();
    }

    #[test]
    fn terms_drop_stopwords_and_split_identifiers() {
        let terms = extract_terms("auth nerede zorlaniyor? verify_auth mi?");
        assert!(terms.contains(&"auth".to_string()));
        assert!(terms.contains(&"verify_auth".to_string()));
        assert!(terms.contains(&"verify".to_string()));
        assert!(!terms.contains(&"nerede".to_string()));
        assert!(terms.len() <= MAX_TERMS);
    }

    #[test]
    fn variants_cover_case_styles() {
        let variants = name_variants("auth_session");
        assert!(variants.contains(&"AuthSession".to_string()));
        assert!(variants.contains(&"authSession".to_string()));
        assert!(variants.contains(&"AUTH_SESSION".to_string()));
    }

    #[test]
    fn cursor_round_trips() {
        let cursor = Cursor {
            question_key: "deadbeef".to_owned(),
            offset: 7,
        };
        assert_eq!(Cursor::parse(&cursor.render()), Some(cursor));
        assert_eq!(Cursor::parse("bozuk"), None);
    }

    #[test]
    fn span_is_clipped_to_block_and_ceiling() {
        let text = "fn a() {\n    let x = 1;\n}\n\nfn b() {}\n";
        let lines = split_lines(text);
        let range = carve_span(&lines, 1).unwrap();
        assert_eq!(range.start, 1);
        assert_eq!(range.end, 3);

        let long: String = (0..500).map(|i| format!("    line {i};\n")).collect();
        let huge = format!("fn big() {{\n{long}}}\n");
        let lines = split_lines(&huge);
        let range = carve_span(&lines, 1).unwrap();
        assert!(range.len() <= MAX_SPAN_LINES, "span tavani asildi");
    }

    #[test]
    fn span_pulls_doc_header_above() {
        let text = "/// Belge.\n#[derive(Debug)]\nstruct S {\n    a: u8,\n}\n";
        let lines = split_lines(text);
        let range = carve_span(&lines, 3).unwrap();
        assert_eq!(range.start, 1, "doc + oznitelik basligi alinmali");
        assert_eq!(range.end, 5);
    }

    #[test]
    fn brace_delta_ignores_comments_and_strings() {
        assert_eq!(brace_delta("fn f() { // }"), 1);
        assert_eq!(brace_delta("let s = \"{{{\";"), 0);
        assert_eq!(brace_delta("}"), -1);
    }

    #[tokio::test]
    async fn empty_question_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let tool = LearnTool::new(dir.path()).unwrap();
        let err = tool.ask("?? !!", None).await;
        assert!(err.is_err());
    }

    #[tokio::test]
    async fn anchor_refresh_detects_fresh_then_stale_under_chunk_scheme() {
        let dir = tempfile::tempdir().unwrap();
        write_fixture(dir.path());
        let tool = LearnTool::new(dir.path()).unwrap();

        let content = std::fs::read_to_string(dir.path().join("src/auth.rs")).unwrap();
        let lines = split_lines(&content);
        let target = tool.scheme.generate_anchors(&lines)[3].render();

        let status = tool.refresh_anchor("src/auth.rs", &target).await.unwrap();
        assert_eq!(status, AnchorStatus::Fresh { line: 4 });

        // Basa iki satir ekle. Chunk semasinda parca parmak izi de degisir,
        // bu yuzden sonuc Stale'dir — anchor korumasi bilerek sikidir.
        std::fs::write(
            dir.path().join("src/auth.rs"),
            format!("// yeni\n// satir\n{content}"),
        )
        .unwrap();
        let status = tool.refresh_anchor("src/auth.rs", &target).await.unwrap();
        assert_eq!(status, AnchorStatus::Stale);
    }

    #[tokio::test]
    async fn anchor_refresh_recovers_shift_under_content_only_scheme() {
        let dir = tempfile::tempdir().unwrap();
        write_fixture(dir.path());
        let params = HashlineSchemeParams {
            scheme: "content_only".to_owned(),
            ..HashlineSchemeParams::default()
        };
        let tool = LearnTool::with_scheme_params(dir.path(), params).unwrap();

        let content = std::fs::read_to_string(dir.path().join("src/auth.rs")).unwrap();
        let lines = split_lines(&content);
        let target = tool.scheme.generate_anchors(&lines)[3].render();

        // Basa iki satir ekle: anchor sinirli pencerede kaymis olarak bulunur.
        std::fs::write(
            dir.path().join("src/auth.rs"),
            format!("// yeni\n// satir\n{content}"),
        )
        .unwrap();
        let status = tool.refresh_anchor("src/auth.rs", &target).await.unwrap();
        assert_eq!(status, AnchorStatus::Shifted { line: 6 });
    }

    #[tokio::test]
    async fn anchor_refresh_rejects_out_of_range_and_garbage() {
        let dir = tempfile::tempdir().unwrap();
        write_fixture(dir.path());
        let tool = LearnTool::new(dir.path()).unwrap();

        assert!(tool.refresh_anchor("src/auth.rs", "cop").await.is_err());
        assert!(tool.refresh_anchor("../disari.rs", "1:abc").await.is_err());
        let status = tool
            .refresh_anchor("src/auth.rs", "9000:abc:def")
            .await
            .unwrap();
        assert_eq!(status, AnchorStatus::OutOfRange);
    }

    #[tokio::test]
    async fn ask_returns_bounded_ranked_evidence() {
        let dir = tempfile::tempdir().unwrap();
        write_fixture(dir.path());
        let tool = LearnTool::new(dir.path()).unwrap();

        let pack = tool.ask("auth nerede dogrulaniyor?", None).await.unwrap();
        assert!(!pack.is_empty(), "kanit bulunmali");
        assert!(pack.spans.len() <= MAX_SPANS_PER_PACK);
        assert!(pack.total_lines() <= MAX_PACK_LINES);
        for span in &pack.spans {
            assert!(span.line_range.len() <= MAX_SPAN_LINES);
            assert!(!span.why_relevant.contains('\n'));
            assert!(span.why_relevant.chars().count() <= WHY_MAX_CHARS);
            assert!(ParsedAnchor::parse(&span.anchor).is_some(), "anchor bozuk");
        }

        // Kursor ayni soruya bagli; baska soruyla reddedilir.
        let cursor = pack.spans[0].cursor.clone();
        assert!(tool.ask("baska bir soru tamamen", Some(cursor)).await.is_err());
    }
}

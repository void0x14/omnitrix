//! Butce zarfi ve hiyerarsik muhasebe (AS4, MASTER-PLAN 7.4).
//!
//! Model:
//! - Ebeveyn, cocuga bir **butce zarfi** tahsis eder (`tasks.budget_allocated`).
//! - Cocugun harcamasi **ebeveynin kalanindan** duser (`tasks.budget_spent`
//!   agac boyunca yukari yayilir).
//! - Kok butce: tam-otonom modda sonsuz (K11); kullanici-odakli modda sonlu.
//! - "Is bitti" tespiti harcamayi durdurur — bitmis ise kaynak yakmak yasak (R6).
//!
//! I5: bu dosyada model adi/fiyati **yoktur**. Tum sayisal degerler cagrisi
//! yapan tarafca `BudgetConfig` icinde tasinir; degerler `omni-config`
//! `ConfigStore`'dan (env > config_kv > dosya, AS8) `KEY_*` sabitleriyle okunur.
//!
//! I6: uretim yolunda `unwrap`/`expect`/`panic!` yok; tum hatalar
//! `BudgetError` ile dondurulur.

use std::collections::HashMap;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Butce modu yapilandirma anahtari (AS8). Deger: `"autonomous"` | `"user_focused"`.
pub const KEY_BUDGET_MODE: &str = "scheduler.budget.mode";
/// Kok butce maliyet tavani anahtari (yalniz kullanici-odakli modda anlamli).
pub const KEY_BUDGET_MAX_COST: &str = "scheduler.budget.max_cost";
/// Kok butce token tavani anahtari.
pub const KEY_BUDGET_MAX_TOKENS: &str = "scheduler.budget.max_tokens";
/// Kok butce gecikme tavani anahtari (ms).
pub const KEY_BUDGET_MAX_LATENCY_MS: &str = "scheduler.budget.max_latency_ms";

/// Sonsuz maliyet zarfi sentineli.
///
/// `f64::INFINITY` **kullanilmaz**: `serde_json` sonsuzu seri hale getiremez ve
/// `Budget` akis/anlik-goruntu yolunda JSON'a yazilir. `f64::MAX` hem JSON
/// guvenli hem de pratikte erisilemez bir tavandir.
pub const UNLIMITED_COST: f64 = f64::MAX;

/// Persona semasindaki (11.1) `budget = { mode = ... }` degeri: tam-otonom.
pub const MODE_AUTONOMOUS: &str = "autonomous";
/// Persona semasindaki (11.1) `budget = { mode = ... }` degeri: kullanici-odakli.
pub const MODE_USER_FOCUSED: &str = "user_focused";

/// Butce modu (7.4). Kok zarfin sonlu mu sonsuz mu oldugunu belirler.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetMode {
    /// Tam-otonom: kok butce sonsuz (K11). Durdurucu, sonlanma oracle'idir (7.5).
    Autonomous,
    /// Kullanici-odakli: kok butce sonlu.
    UserFocused,
}

impl BudgetMode {
    /// Persona/config metnini moda cevirir. Bilinmeyen deger -> `None`.
    #[must_use]
    pub fn from_config_str(raw: &str) -> Option<Self> {
        match raw.trim() {
            MODE_AUTONOMOUS => Some(Self::Autonomous),
            MODE_USER_FOCUSED => Some(Self::UserFocused),
            _ => None,
        }
    }

    /// Yapilandirmaya yazilabilir metin karsiligi.
    #[must_use]
    pub fn as_config_str(self) -> &'static str {
        match self {
            Self::Autonomous => MODE_AUTONOMOUS,
            Self::UserFocused => MODE_USER_FOCUSED,
        }
    }

    /// Bu modda kok butce sonsuz mu? (K11)
    #[must_use]
    pub fn is_unbounded_root(self) -> bool {
        matches!(self, Self::Autonomous)
    }
}

/// Kok butcenin yapilandirmadan okunmus hali.
///
/// `omni-config` bagimliligi bu crate'te yok; cagiran taraf `ConfigStore::get`
/// ile `KEY_*` anahtarlarini okuyup bu yapiyi doldurur. Boylece hicbir
/// maliyet/limit sabiti koda gomulu olmaz (I5/AS8).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BudgetConfig {
    /// Butce modu.
    pub mode: BudgetMode,
    /// Token tavani; `None` -> mod belirler.
    #[serde(default)]
    pub max_tokens: Option<u64>,
    /// Maliyet tavani; `None` -> mod belirler.
    #[serde(default)]
    pub max_cost: Option<f64>,
    /// Gecikme tavani (ms); `None` -> mod belirler.
    #[serde(default)]
    pub max_latency_ms: Option<u64>,
}

impl BudgetConfig {
    /// Yalnizca mod bilinen, tavanlari moda birakan yapilandirma.
    #[must_use]
    pub fn from_mode(mode: BudgetMode) -> Self {
        Self { mode, max_tokens: None, max_cost: None, max_latency_ms: None }
    }
}

/// Butce zarfi tavanlari. Sonsuz zarf `u64::MAX` / [`UNLIMITED_COST`] ile temsil edilir.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Budget {
    /// Token tavani.
    pub max_tokens: u64,
    /// Maliyet tavani.
    pub max_cost: f64,
    /// Gecikme tavani (ms).
    pub max_latency_ms: u64,
}

impl Budget {
    /// Sinirsiz zarf (tam-otonom kok, K11).
    #[must_use]
    pub fn unlimited() -> Self {
        Self { max_tokens: u64::MAX, max_cost: UNLIMITED_COST, max_latency_ms: u64::MAX }
    }

    /// Yapilandirmadan kok zarf uretir (7.4).
    ///
    /// - `Autonomous`: acikca verilmemis her tavan **sonsuz**.
    /// - `UserFocused`: acikca verilmemis her tavan **sifir** — kullanici-odakli
    ///   modda "tanimsiz" bir tavan sessizce sonsuza donmez, spawn reddedilir.
    #[must_use]
    pub fn from_config(cfg: &BudgetConfig) -> Self {
        if cfg.mode.is_unbounded_root() {
            Self {
                max_tokens: cfg.max_tokens.unwrap_or(u64::MAX),
                max_cost: cfg.max_cost.unwrap_or(UNLIMITED_COST),
                max_latency_ms: cfg.max_latency_ms.unwrap_or(u64::MAX),
            }
        } else {
            Self {
                max_tokens: cfg.max_tokens.unwrap_or(0),
                max_cost: cfg.max_cost.unwrap_or(0.0),
                max_latency_ms: cfg.max_latency_ms.unwrap_or(0),
            }
        }
    }

    /// Zarfin token+maliyet bileseni (hiyerarsik muhasebe birimi).
    #[must_use]
    pub fn amount(&self) -> BudgetAmount {
        BudgetAmount { tokens: self.max_tokens, cost: self.max_cost }
    }
}

impl Default for Budget {
    fn default() -> Self {
        Self::unlimited()
    }
}

/// Tek bir ajanin zarf tuketimi (tur dongusu icinde okunur).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BudgetTracker {
    /// Bu ajana tahsis edilmis zarf.
    pub budget: Budget,
    /// Tuketilen token.
    pub tokens_used: u64,
    /// Tuketilen maliyet.
    pub cost_incurred: f64,
    /// Harcanan sure (ms).
    pub latency_ms: u64,
    /// "Is bitti" isareti; `true` iken harcama kaydi reddedilir (R6).
    #[serde(default)]
    finished: bool,
}

impl BudgetTracker {
    /// Verilen zarf uzerinde sifirdan sayac acar.
    #[must_use]
    pub fn new(budget: Budget) -> Self {
        Self { budget, tokens_used: 0, cost_incurred: 0.0, latency_ms: 0, finished: false }
    }

    /// Tuketimi kaydeder. Is bitmisse kaynak yakmak yasaktir (R6): cagri yok
    /// sayilir ve `false` doner.
    pub fn record_usage(&mut self, tokens: u64, cost: f64, latency_ms: u64) -> bool {
        if self.finished {
            return false;
        }
        self.tokens_used = self.tokens_used.saturating_add(tokens);
        self.cost_incurred += cost;
        self.latency_ms = self.latency_ms.saturating_add(latency_ms);
        true
    }

    /// "Is bitti" (7.5) — bundan sonra harcama kabul edilmez.
    pub fn mark_finished(&mut self) {
        self.finished = true;
    }

    /// Is bitmis mi?
    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.finished
    }

    /// Zarf tukendi mi? Bitmis is de "tukenmi" sayilir (harcama duracak).
    #[must_use]
    pub fn is_exhausted(&self) -> bool {
        self.finished
            || self.tokens_used >= self.budget.max_tokens
            || self.cost_incurred >= self.budget.max_cost
            || self.latency_ms >= self.budget.max_latency_ms
    }

    /// Kalan token.
    #[must_use]
    pub fn remaining_tokens(&self) -> u64 {
        self.budget.max_tokens.saturating_sub(self.tokens_used)
    }

    /// Kalan maliyet.
    #[must_use]
    pub fn remaining_cost(&self) -> f64 {
        (self.budget.max_cost - self.cost_incurred).max(0.0)
    }

    /// Tuketimin hiyerarsik deftere yazilacak hali.
    #[must_use]
    pub fn spent(&self) -> BudgetAmount {
        BudgetAmount { tokens: self.tokens_used, cost: self.cost_incurred }
    }
}

/// Token + maliyet ciftinden olusan muhasebe miktari (7.4).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BudgetAmount {
    /// Token bileseni.
    pub tokens: u64,
    /// Maliyet bileseni.
    pub cost: f64,
}

impl BudgetAmount {
    /// Sifir miktar.
    pub const ZERO: Self = Self { tokens: 0, cost: 0.0 };

    /// Sonsuz miktar (tam-otonom kok zarfi, K11).
    #[must_use]
    pub fn unlimited() -> Self {
        Self { tokens: u64::MAX, cost: UNLIMITED_COST }
    }

    /// Yeni miktar.
    #[must_use]
    pub fn new(tokens: u64, cost: f64) -> Self {
        Self { tokens, cost }
    }

    /// Tasma-guvenli toplama.
    #[must_use]
    pub fn saturating_add(self, other: Self) -> Self {
        Self { tokens: self.tokens.saturating_add(other.tokens), cost: self.cost + other.cost }
    }

    /// Tasma-guvenli cikarma (negatife dusmez).
    #[must_use]
    pub fn saturating_sub(self, other: Self) -> Self {
        Self {
            tokens: self.tokens.saturating_sub(other.tokens),
            cost: if self.cost >= UNLIMITED_COST { self.cost } else { (self.cost - other.cost).max(0.0) },
        }
    }

    /// Bilesen bazinda buyuk olani.
    #[must_use]
    pub fn max_of(self, other: Self) -> Self {
        Self { tokens: self.tokens.max(other.tokens), cost: self.cost.max(other.cost) }
    }

    /// Sonsuz mu? (her iki bilesen de tavanda)
    #[must_use]
    pub fn is_unlimited(self) -> bool {
        self.tokens == u64::MAX && self.cost >= UNLIMITED_COST
    }

    /// `self` miktari `cap` zarfina sigar mi?
    #[must_use]
    pub fn fits_within(self, cap: Self) -> bool {
        self.tokens <= cap.tokens && self.cost <= cap.cost
    }
}

/// Deftere yazilmis tek dugum (bir `tasks` satirina karsilik gelir).
#[derive(Debug, Clone)]
struct LedgerNode {
    parent: Option<Uuid>,
    children: Vec<Uuid>,
    /// `tasks.budget_allocated` — ebeveynin tahsis ettigi zarf.
    allocated: BudgetAmount,
    /// Bu dugumun **kendi** harcamasi.
    own_spent: BudgetAmount,
    /// Kendi + tum alt agac harcamasi; `tasks.budget_spent` bu degerdir.
    subtree_spent: BudgetAmount,
    /// "Is bitti" (7.5). Bitmisse harcama reddedilir ve rezerv serbest kalir (R6).
    finished: bool,
}

impl LedgerNode {
    fn new(parent: Option<Uuid>, allocated: BudgetAmount) -> Self {
        Self {
            parent,
            children: Vec::new(),
            allocated,
            own_spent: BudgetAmount::ZERO,
            subtree_spent: BudgetAmount::ZERO,
            finished: false,
        }
    }
}

/// Bir gorevin defterdeki anlik hali; `tasks` tablosuna yazilacak satir (14.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskBudgetRow {
    /// `tasks.id`.
    pub task_id: Uuid,
    /// `tasks.parent_id`.
    pub parent_id: Option<Uuid>,
    /// `tasks.budget_allocated` (REAL) — zarfin maliyet bileseni.
    /// Sonsuz zarf DB'ye `None` olarak yazilir (SQL `NULL` = sinirsiz).
    pub budget_allocated: Option<f64>,
    /// `tasks.budget_spent` (REAL) — alt agac dahil harcanan maliyet.
    pub budget_spent: f64,
    /// Zarfin token bileseni (DB'de kolon yok; raporlama icin).
    pub allocated_tokens: Option<u64>,
    /// Alt agac dahil harcanan token.
    pub spent_tokens: u64,
    /// Is bitti mi?
    pub finished: bool,
}

/// Hiyerarsik butce defteri (AS4, 7.4).
///
/// Es zamanli erisim `parking_lot::RwLock` ile korunur; `Arc` ile paylasilir.
#[derive(Debug, Default)]
pub struct BudgetLedger {
    nodes: RwLock<HashMap<Uuid, LedgerNode>>,
}

impl BudgetLedger {
    /// Bos defter.
    #[must_use]
    pub fn new() -> Self {
        Self { nodes: RwLock::new(HashMap::new()) }
    }

    /// Kok gorevi acar. Zarf, yapilandirmadan (`BudgetConfig`) uretilir:
    /// tam-otonom modda sonsuz (K11), kullanici-odakli modda sonlu.
    pub fn open_root(&self, task_id: Uuid, cfg: &BudgetConfig) -> Result<(), BudgetError> {
        let allocated = Budget::from_config(cfg).amount();
        let mut nodes = self.nodes.write();
        if nodes.contains_key(&task_id) {
            return Err(BudgetError::DuplicateTask(task_id));
        }
        nodes.insert(task_id, LedgerNode::new(None, allocated));
        Ok(())
    }

    /// Ebeveynden cocuga zarf tahsis eder (7.4).
    ///
    /// Reddedilir:
    /// - ebeveyn defterde yoksa (`ParentNotFound`),
    /// - ebeveyn bitmisse (`TaskFinished`) — bitmis ise kaynak yakmak yasak (R6),
    /// - istenen zarf ebeveynin **kalanina** sigmiyorsa (`InsufficientBudget`).
    pub fn allocate_child(
        &self,
        parent_id: Uuid,
        child_id: Uuid,
        request: BudgetAmount,
    ) -> Result<(), BudgetError> {
        let mut nodes = self.nodes.write();
        if nodes.contains_key(&child_id) {
            return Err(BudgetError::DuplicateTask(child_id));
        }
        let available = {
            let parent = nodes.get(&parent_id).ok_or(BudgetError::ParentNotFound(parent_id))?;
            if parent.finished {
                return Err(BudgetError::TaskFinished(parent_id));
            }
            Self::available_of(&nodes, parent_id).unwrap_or(BudgetAmount::ZERO)
        };
        if !request.fits_within(available) {
            return Err(BudgetError::InsufficientBudget {
                parent: parent_id,
                requested: request,
                available,
            });
        }
        nodes.insert(child_id, LedgerNode::new(Some(parent_id), request));
        if let Some(parent) = nodes.get_mut(&parent_id) {
            parent.children.push(child_id);
        }
        Ok(())
    }

    /// Ebeveynin kalanindan **bolunmus** cocuk zarfi uretir: kalanin `share`
    /// orani kadari. `share` 0.0..=1.0 disindaysa hata.
    pub fn allocate_child_share(
        &self,
        parent_id: Uuid,
        child_id: Uuid,
        share: f64,
    ) -> Result<BudgetAmount, BudgetError> {
        if !share.is_finite() || !(0.0..=1.0).contains(&share) {
            return Err(BudgetError::InvalidShare(share));
        }
        let available = self.available(parent_id).ok_or(BudgetError::ParentNotFound(parent_id))?;
        let request = if available.is_unlimited() {
            // Sonsuz ebeveyn: cocuk da sonsuz zarf alir (K11).
            BudgetAmount::unlimited()
        } else {
            let tokens = ((available.tokens as f64) * share) as u64;
            let cost =
                if available.cost >= UNLIMITED_COST { UNLIMITED_COST } else { available.cost * share };
            BudgetAmount::new(tokens, cost)
        };
        self.allocate_child(parent_id, child_id, request)?;
        Ok(request)
    }

    /// Harcamayi kaydeder ve **ata zincirine yayar** — cocugun harcamasi
    /// ebeveynin kalanindan duser (7.4).
    ///
    /// Is bitmisse harcama reddedilir (R6).
    pub fn record_spend(&self, task_id: Uuid, amount: BudgetAmount) -> Result<(), BudgetError> {
        let mut nodes = self.nodes.write();
        {
            let node = nodes.get(&task_id).ok_or(BudgetError::TaskNotFound(task_id))?;
            if node.finished {
                return Err(BudgetError::TaskFinished(task_id));
            }
        }
        if let Some(node) = nodes.get_mut(&task_id) {
            node.own_spent = node.own_spent.saturating_add(amount);
        }
        // Kendi dugumu dahil, koke kadar alt-agac toplamini guncelle.
        let mut cursor = Some(task_id);
        let mut guard = 0usize;
        let cap = nodes.len().saturating_add(1);
        while let Some(id) = cursor {
            guard += 1;
            if guard > cap {
                // Defterde donguye benzer bir bozulma var; sessiz sonsuz dongu yerine hata.
                return Err(BudgetError::CorruptChain(id));
            }
            let Some(node) = nodes.get_mut(&id) else { break };
            node.subtree_spent = node.subtree_spent.saturating_add(amount);
            cursor = node.parent;
        }
        Ok(())
    }

    /// "Is bitti" (7.5): gorevi ve tum alt agacini bitmis isaretler. Bundan
    /// sonra bu alt agacta harcama kabul edilmez ve tutulan rezerv serbest
    /// kalir — bitmis ise kaynak yakmak yasak (R6).
    ///
    /// Bitmis isaretlenen gorev kimliklerini doner.
    pub fn mark_finished(&self, task_id: Uuid) -> Vec<Uuid> {
        let mut nodes = self.nodes.write();
        let mut stack = vec![task_id];
        let mut finished = Vec::new();
        let cap = nodes.len();
        while let Some(id) = stack.pop() {
            if finished.len() > cap {
                break;
            }
            let Some(node) = nodes.get_mut(&id) else { continue };
            if node.finished {
                continue;
            }
            node.finished = true;
            stack.extend(node.children.iter().copied());
            finished.push(id);
        }
        finished
    }

    /// Gorev bitmis mi?
    #[must_use]
    pub fn is_finished(&self, task_id: Uuid) -> bool {
        self.nodes.read().get(&task_id).map(|n| n.finished).unwrap_or(false)
    }

    /// Zarf tukendi mi? Bitmis is de tukenmi sayilir (harcama durur).
    #[must_use]
    pub fn is_exhausted(&self, task_id: Uuid) -> bool {
        let nodes = self.nodes.read();
        match nodes.get(&task_id) {
            None => true,
            Some(node) if node.finished => true,
            Some(node) => {
                node.subtree_spent.tokens >= node.allocated.tokens
                    || node.subtree_spent.cost >= node.allocated.cost
            }
        }
    }

    /// Gorevin kalan (henuz ne harcanmis ne de cocuklara rezerve edilmis) zarfi.
    #[must_use]
    pub fn available(&self, task_id: Uuid) -> Option<BudgetAmount> {
        let nodes = self.nodes.read();
        Self::available_of(&nodes, task_id)
    }

    /// Gorevin alt agac dahil harcamasi (`tasks.budget_spent`).
    #[must_use]
    pub fn spent(&self, task_id: Uuid) -> Option<BudgetAmount> {
        self.nodes.read().get(&task_id).map(|n| n.subtree_spent)
    }

    /// Gorevin zarfi (`tasks.budget_allocated`).
    #[must_use]
    pub fn allocated(&self, task_id: Uuid) -> Option<BudgetAmount> {
        self.nodes.read().get(&task_id).map(|n| n.allocated)
    }

    /// Defterdeki gorev sayisi.
    #[must_use]
    pub fn len(&self) -> usize {
        self.nodes.read().len()
    }

    /// Defter bos mu?
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.read().is_empty()
    }

    /// Gorevi ve alt agacini defterden siler (gorev kapandiktan sonra).
    pub fn drop_subtree(&self, task_id: Uuid) -> Vec<Uuid> {
        let mut nodes = self.nodes.write();
        let mut stack = vec![task_id];
        let mut removed = Vec::new();
        let cap = nodes.len();
        while let Some(id) = stack.pop() {
            if removed.len() > cap {
                break;
            }
            let Some(node) = nodes.remove(&id) else { continue };
            if let Some(pid) = node.parent
                && let Some(parent) = nodes.get_mut(&pid)
            {
                parent.children.retain(|c| *c != id);
            }
            stack.extend(node.children.iter().copied());
            removed.push(id);
        }
        removed
    }

    /// `tasks` tablosuna yazilacak satiri uretir (14.2).
    #[must_use]
    pub fn db_row(&self, task_id: Uuid) -> Option<TaskBudgetRow> {
        let nodes = self.nodes.read();
        let node = nodes.get(&task_id)?;
        Some(TaskBudgetRow {
            task_id,
            parent_id: node.parent,
            budget_allocated: if node.allocated.cost < UNLIMITED_COST {
                Some(node.allocated.cost)
            } else {
                None
            },
            budget_spent: node.subtree_spent.cost,
            allocated_tokens: if node.allocated.tokens == u64::MAX {
                None
            } else {
                Some(node.allocated.tokens)
            },
            spent_tokens: node.subtree_spent.tokens,
            finished: node.finished,
        })
    }

    /// Crash-only yeniden kurulum (3.2/I7): DB'den okunan `tasks` satirini
    /// deftere geri koyar. `budget_allocated`/`allocated_tokens` `None` ise
    /// sonsuz zarf demektir.
    pub fn restore(&self, row: &TaskBudgetRow) -> Result<(), BudgetError> {
        let mut nodes = self.nodes.write();
        if nodes.contains_key(&row.task_id) {
            return Err(BudgetError::DuplicateTask(row.task_id));
        }
        let allocated = BudgetAmount::new(
            row.allocated_tokens.unwrap_or(u64::MAX),
            row.budget_allocated.unwrap_or(UNLIMITED_COST),
        );
        let mut node = LedgerNode::new(row.parent_id, allocated);
        node.subtree_spent = BudgetAmount::new(row.spent_tokens, row.budget_spent);
        node.own_spent = node.subtree_spent;
        node.finished = row.finished;
        nodes.insert(row.task_id, node);
        if let Some(pid) = row.parent_id
            && let Some(parent) = nodes.get_mut(&pid)
            && !parent.children.contains(&row.task_id)
        {
            parent.children.push(row.task_id);
        }
        Ok(())
    }

    /// Kalan zarf: `allocated - kendi harcamasi - canli cocuklarin tuttugu rezerv`.
    ///
    /// Canli cocuk zarfini tam tutar (harcasa da harcamasa da); **bitmis** cocuk
    /// yalnizca gercekten harcadigi kadarini tutar — rezerv serbest kalir (R6).
    fn available_of(nodes: &HashMap<Uuid, LedgerNode>, task_id: Uuid) -> Option<BudgetAmount> {
        let node = nodes.get(&task_id)?;
        if node.finished {
            return Some(BudgetAmount::ZERO);
        }
        let mut committed = node.own_spent;
        for child_id in &node.children {
            let Some(child) = nodes.get(child_id) else { continue };
            let held = if child.finished {
                child.subtree_spent
            } else {
                child.allocated.max_of(child.subtree_spent)
            };
            committed = committed.saturating_add(held);
        }
        Some(node.allocated.saturating_sub(committed))
    }
}

/// Butce muhasebesi hatalari.
#[derive(Debug, thiserror::Error)]
pub enum BudgetError {
    /// Gorev defterde zaten var.
    #[error("gorev {0} defterde zaten kayitli")]
    DuplicateTask(Uuid),

    /// Gorev defterde yok.
    #[error("gorev {0} defterde bulunamadi")]
    TaskNotFound(Uuid),

    /// Ebeveyn defterde yok.
    #[error("ebeveyn gorev {0} defterde bulunamadi")]
    ParentNotFound(Uuid),

    /// Is bitti; harcama/tahsis yasak (R6).
    #[error("gorev {0} bitmis: bitmis ise kaynak yakmak yasak")]
    TaskFinished(Uuid),

    /// Ebeveynin kalani istenen zarfi karsilamiyor.
    #[error(
        "gorev {parent} icin butce yetersiz: istenen {requested:?}, kalan {available:?}"
    )]
    InsufficientBudget {
        /// Ebeveyn gorev.
        parent: Uuid,
        /// Istenen zarf.
        requested: BudgetAmount,
        /// Ebeveynde kalan.
        available: BudgetAmount,
    },

    /// Pay orani 0.0..=1.0 disinda.
    #[error("gecersiz butce payi {0}: 0.0..=1.0 olmali")]
    InvalidShare(f64),

    /// Defterdeki ata zinciri bozuk (dongu benzeri).
    #[error("butce defterinde bozuk ata zinciri: {0}")]
    CorruptChain(Uuid),
}

impl BudgetError {
    /// `NoticeView.code` alanina yazilacak makine-eslenebilir kod (6.1).
    #[must_use]
    pub fn notice_code(&self) -> &'static str {
        match self {
            Self::DuplicateTask(_) => "budget_duplicate_task",
            Self::TaskNotFound(_) | Self::ParentNotFound(_) => "budget_task_not_found",
            Self::TaskFinished(_) => "budget_task_finished",
            Self::InsufficientBudget { .. } => "budget_exhausted",
            Self::InvalidShare(_) => "budget_invalid_share",
            Self::CorruptChain(_) => "budget_corrupt_chain",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user_focused(max_cost: f64, max_tokens: u64) -> BudgetConfig {
        BudgetConfig {
            mode: BudgetMode::UserFocused,
            max_tokens: Some(max_tokens),
            max_cost: Some(max_cost),
            max_latency_ms: None,
        }
    }

    #[test]
    fn autonomous_root_is_unbounded() {
        let cfg = BudgetConfig::from_mode(BudgetMode::Autonomous);
        let budget = Budget::from_config(&cfg);
        assert!(budget.amount().is_unlimited());
    }

    #[test]
    fn user_focused_root_is_finite() {
        let budget = Budget::from_config(&user_focused(5.0, 1000));
        assert!(!budget.amount().is_unlimited());
        assert!((budget.max_cost - 5.0).abs() < f64::EPSILON);
    }

    #[test]
    fn mode_roundtrips_through_config_str() {
        for mode in [BudgetMode::Autonomous, BudgetMode::UserFocused] {
            let s = mode.as_config_str();
            assert_eq!(BudgetMode::from_config_str(s), Some(mode));
        }
        assert_eq!(BudgetMode::from_config_str("bilinmeyen"), None);
    }

    #[test]
    fn child_spend_reduces_parent_remaining() {
        let ledger = BudgetLedger::new();
        let root = Uuid::new_v4();
        let child = Uuid::new_v4();
        assert!(ledger.open_root(root, &user_focused(10.0, 1000)).is_ok());
        assert!(ledger.allocate_child(root, child, BudgetAmount::new(400, 4.0)).is_ok());

        // Zarf tahsis edildi: kok kalani 10.0 - 4.0 = 6.0.
        let avail = ledger.available(root).unwrap_or(BudgetAmount::ZERO);
        assert!((avail.cost - 6.0).abs() < 1e-9);

        // Cocuk harcadi: harcama koke yayilir ama rezerv icinde kalir.
        assert!(ledger.record_spend(child, BudgetAmount::new(100, 1.0)).is_ok());
        let avail = ledger.available(root).unwrap_or(BudgetAmount::ZERO);
        assert!((avail.cost - 6.0).abs() < 1e-9);
        let spent = ledger.spent(root).unwrap_or(BudgetAmount::ZERO);
        assert!((spent.cost - 1.0).abs() < 1e-9);
    }

    #[test]
    fn child_overspend_eats_parent_remaining() {
        let ledger = BudgetLedger::new();
        let root = Uuid::new_v4();
        let child = Uuid::new_v4();
        assert!(ledger.open_root(root, &user_focused(10.0, 1000)).is_ok());
        assert!(ledger.allocate_child(root, child, BudgetAmount::new(100, 1.0)).is_ok());
        assert!(ledger.record_spend(child, BudgetAmount::new(300, 3.0)).is_ok());

        // Cocuk zarfini asti; kokun kalani gercek harcamaya gore duser.
        let avail = ledger.available(root).unwrap_or(BudgetAmount::ZERO);
        assert!((avail.cost - 7.0).abs() < 1e-9, "kalan {}", avail.cost);
    }

    #[test]
    fn allocation_beyond_parent_remaining_is_rejected() {
        let ledger = BudgetLedger::new();
        let root = Uuid::new_v4();
        assert!(ledger.open_root(root, &user_focused(2.0, 100)).is_ok());
        let err = ledger.allocate_child(root, Uuid::new_v4(), BudgetAmount::new(50, 3.0));
        assert!(matches!(err, Err(BudgetError::InsufficientBudget { .. })));
    }

    #[test]
    fn finished_task_stops_spending() {
        let ledger = BudgetLedger::new();
        let root = Uuid::new_v4();
        let child = Uuid::new_v4();
        assert!(ledger.open_root(root, &user_focused(10.0, 1000)).is_ok());
        assert!(ledger.allocate_child(root, child, BudgetAmount::new(400, 4.0)).is_ok());

        let finished = ledger.mark_finished(root);
        assert_eq!(finished.len(), 2, "alt agac da bitmis olmali");
        assert!(ledger.is_finished(child));

        let err = ledger.record_spend(child, BudgetAmount::new(1, 0.1));
        assert!(matches!(err, Err(BudgetError::TaskFinished(_))));
    }

    #[test]
    fn finished_child_releases_reservation() {
        let ledger = BudgetLedger::new();
        let root = Uuid::new_v4();
        let child = Uuid::new_v4();
        assert!(ledger.open_root(root, &user_focused(10.0, 1000)).is_ok());
        assert!(ledger.allocate_child(root, child, BudgetAmount::new(400, 4.0)).is_ok());
        assert!(ledger.record_spend(child, BudgetAmount::new(100, 1.0)).is_ok());
        ledger.mark_finished(child);

        // Rezerv serbest: kalan 10.0 - 1.0 (gercek harcama).
        let avail = ledger.available(root).unwrap_or(BudgetAmount::ZERO);
        assert!((avail.cost - 9.0).abs() < 1e-9, "kalan {}", avail.cost);
    }

    #[test]
    fn unlimited_root_never_exhausts() {
        let ledger = BudgetLedger::new();
        let root = Uuid::new_v4();
        assert!(ledger.open_root(root, &BudgetConfig::from_mode(BudgetMode::Autonomous)).is_ok());
        assert!(ledger.record_spend(root, BudgetAmount::new(u64::MAX / 2, 1e12)).is_ok());
        assert!(!ledger.is_exhausted(root));
    }

    #[test]
    fn share_allocation_splits_remaining() {
        let ledger = BudgetLedger::new();
        let root = Uuid::new_v4();
        assert!(ledger.open_root(root, &user_focused(10.0, 1000)).is_ok());
        let amount = ledger
            .allocate_child_share(root, Uuid::new_v4(), 0.5)
            .unwrap_or(BudgetAmount::ZERO);
        assert!((amount.cost - 5.0).abs() < 1e-9);
        assert_eq!(amount.tokens, 500);
        assert!(matches!(
            ledger.allocate_child_share(root, Uuid::new_v4(), 1.5),
            Err(BudgetError::InvalidShare(_))
        ));
    }

    #[test]
    fn db_row_roundtrips_through_restore() {
        let ledger = BudgetLedger::new();
        let root = Uuid::new_v4();
        let child = Uuid::new_v4();
        assert!(ledger.open_root(root, &user_focused(10.0, 1000)).is_ok());
        assert!(ledger.allocate_child(root, child, BudgetAmount::new(400, 4.0)).is_ok());
        assert!(ledger.record_spend(child, BudgetAmount::new(100, 1.0)).is_ok());

        let (Some(row_root), Some(row_child)) = (ledger.db_row(root), ledger.db_row(child))
        else {
            unreachable!("defterde iki gorev var")
        };
        assert_eq!(row_root.budget_allocated, Some(10.0));
        assert!((row_root.budget_spent - 1.0).abs() < 1e-9);
        assert_eq!(row_child.parent_id, Some(root));

        let restored = BudgetLedger::new();
        assert!(restored.restore(&row_root).is_ok());
        assert!(restored.restore(&row_child).is_ok());
        assert_eq!(restored.len(), 2);
        assert_eq!(restored.db_row(child).map(|r| r.parent_id), Some(Some(root)));
    }

    #[test]
    fn tracker_refuses_usage_after_finish() {
        let mut tracker = BudgetTracker::new(Budget::from_config(&user_focused(1.0, 100)));
        assert!(tracker.record_usage(10, 0.1, 5));
        tracker.mark_finished();
        assert!(!tracker.record_usage(10, 0.1, 5));
        assert_eq!(tracker.tokens_used, 10);
        assert!(tracker.is_exhausted());
    }

    #[test]
    fn drop_subtree_removes_children() {
        let ledger = BudgetLedger::new();
        let root = Uuid::new_v4();
        let child = Uuid::new_v4();
        assert!(ledger.open_root(root, &user_focused(10.0, 1000)).is_ok());
        assert!(ledger.allocate_child(root, child, BudgetAmount::new(1, 1.0)).is_ok());
        let removed = ledger.drop_subtree(root);
        assert_eq!(removed.len(), 2);
        assert!(ledger.is_empty());
    }
}

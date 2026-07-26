//! K3 yetki broker'i — tool cagrilarinin TEK zorlama noktasi (MASTER-PLAN 9.1).
//!
//! Karar burada verilir, baska hicbir yerde tekrarlanmaz. Her karar
//! `capability_audit` satirina yazilacak sekilde gerekcelendirilir.

use std::collections::BTreeSet;

/// Broker karari.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Cagri izin listesine uyuyor.
    Allow,
    /// Cagri reddedildi; metin denetim kaydina gider.
    Deny(String),
}

impl Decision {
    /// Kararin izin verip vermedigi.
    pub fn is_allowed(&self) -> bool {
        matches!(self, Decision::Allow)
    }
}

/// Persona basina kurulan izin/red listesi.
#[derive(Debug, Clone, Default)]
pub struct ToolBroker {
    allowed: BTreeSet<String>,
    denied: BTreeSet<String>,
}

impl ToolBroker {
    /// Bos broker: izin listesi bos oldugu surece yalnizca red listesi zorlanir.
    pub fn new() -> Self {
        Self::default()
    }

    /// Izin listesine tool ekler.
    pub fn allow(mut self, tool: impl Into<String>) -> Self {
        self.allowed.insert(tool.into());
        self
    }

    /// Red listesine tool ekler; red her zaman izne baskin gelir.
    pub fn deny(mut self, tool: impl Into<String>) -> Self {
        self.denied.insert(tool.into());
        self
    }

    /// Izin listesindeki tool adlari.
    pub fn allowed(&self) -> impl Iterator<Item = &str> {
        self.allowed.iter().map(String::as_str)
    }

    /// Red listesindeki tool adlari.
    pub fn denied(&self) -> impl Iterator<Item = &str> {
        self.denied.iter().map(String::as_str)
    }

    /// Tek karar noktasi. Bos izin listesi = broker yapilandirilmamis demektir.
    pub fn decide(&self, tool: &str) -> Decision {
        if self.denied.contains(tool) {
            return Decision::Deny(format!("'{tool}' red listesinde"));
        }
        if self.allowed.is_empty() || self.allowed.contains(tool) {
            Decision::Allow
        } else {
            Decision::Deny(format!("'{tool}' izin listesinde degil"))
        }
    }
}

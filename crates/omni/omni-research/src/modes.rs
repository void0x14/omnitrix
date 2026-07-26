//! Arastirma modlari (MASTER-PLAN 19.2): **yuzeysel / derin / okyanus**.
//!
//! Modun tek isi tarama butcesini somut sayilara baglamaktir. Cekirdek dongu
//! ([`crate::ResearchEngine`]) bu sayilari okur; saglayici degisse de dongu
//! ayni kalir. Sayilar burada tek noktada durur ki "derin mod ne kadar genis
//! tarar" sorusunun tek bir kanit adresi olsun.
//!
//! Butce boyutlari (hepsi mod arttikca monoton buyur):
//!
//! | Mod       | `max_sources` | `crawl_depth` | `refine_rounds` | `per_query_results` | `refine_fanout` | `max_queries` |
//! |-----------|---------------|---------------|-----------------|---------------------|-----------------|---------------|
//! | yuzeysel  | 8             | 1             | 1               | 8                   | 0               | 1             |
//! | derin     | 40            | 2             | 3               | 12                  | 3               | 12            |
//! | okyanus   | 160           | 4             | 6               | 20                  | 6               | 60            |

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Tarama genisligi modu. `research_findings.mode` sutununa
/// [`ResearchMode::as_db_str`] ile yazilir (migrations/0008).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResearchMode {
    /// Yuzeysel: tek tur, tek sorgu, sadece arama sonucu ozetleri.
    Surface,
    /// Derin: sorgu genisletmeli birkac tur, sinirli link takibi.
    Deep,
    /// Okyanus: genis butce, cok turlu genisletme, derin link takibi.
    Ocean,
}

impl ResearchMode {
    /// Kapinin talep ettigi uc mod; testler ve CLI bu diziyi dolasir.
    pub const ALL: [ResearchMode; 3] = [
        ResearchMode::Surface,
        ResearchMode::Deep,
        ResearchMode::Ocean,
    ];

    /// `research_findings.mode` icin kanonik degeri verir.
    #[must_use]
    pub fn as_db_str(self) -> &'static str {
        match self {
            ResearchMode::Surface => "surface",
            ResearchMode::Deep => "deep",
            ResearchMode::Ocean => "ocean",
        }
    }

    /// DB/konfig degerinden mod cozer. Turkce adlar da kabul edilir cunku
    /// MASTER-PLAN 19.2 modlari o adlarla anar; kanonik yazim yine ingilizcedir.
    #[must_use]
    pub fn from_db_str(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "surface" | "yuzeysel" | "shallow" => Some(ResearchMode::Surface),
            "deep" | "derin" => Some(ResearchMode::Deep),
            "ocean" | "okyanus" => Some(ResearchMode::Ocean),
            _ => None,
        }
    }

    /// Bu modun somut tarama butcesi.
    #[must_use]
    pub const fn params(self) -> ModeParams {
        match self {
            ResearchMode::Surface => ModeParams {
                max_sources: 8,
                crawl_depth: 1,
                refine_rounds: 1,
                per_query_results: 8,
                refine_fanout: 0,
                max_queries: 1,
                deadline: Duration::from_secs(30),
            },
            ResearchMode::Deep => ModeParams {
                max_sources: 40,
                crawl_depth: 2,
                refine_rounds: 3,
                per_query_results: 12,
                refine_fanout: 3,
                max_queries: 12,
                deadline: Duration::from_secs(300),
            },
            ResearchMode::Ocean => ModeParams {
                max_sources: 160,
                crawl_depth: 4,
                refine_rounds: 6,
                per_query_results: 20,
                refine_fanout: 6,
                max_queries: 60,
                deadline: Duration::from_secs(1800),
            },
        }
    }
}

/// Bir modun tarama butcesi. Alanlarin hepsi ust sinirdir; cekirdek dongu
/// bunlari asamaz, saglayici da bunlari gormeden calisamaz.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModeParams {
    /// Rapora girecek benzersiz kaynak (URL) ust siniri.
    pub max_sources: usize,
    /// Saglayiciya iletilen link takip derinligi; 1 = yalnizca arama sonucu.
    pub crawl_depth: u8,
    /// Tekrar turu sayisi. 1 = tek atis, genisletme yok.
    pub refine_rounds: u8,
    /// Tek sorgudan alinacak sonuc ust siniri.
    pub per_query_results: usize,
    /// Bir turun sonunda uretilecek yeni (genisletilmis) sorgu sayisi.
    pub refine_fanout: u8,
    /// Tum turlar boyunca calistirilabilecek toplam sorgu sayisi.
    pub max_queries: usize,
    /// Tum arastirmanin ust sure siniri (AS13 sure sistemi ile uyumlu).
    pub deadline: Duration,
}

impl ModeParams {
    /// Kaba bir ust sinir: en fazla kac saglayici cagrisi yapilabilir.
    #[must_use]
    pub const fn max_provider_calls(&self) -> usize {
        self.max_queries
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mod_db_gidis_donus() {
        for mode in ResearchMode::ALL {
            let raw = mode.as_db_str();
            assert_eq!(ResearchMode::from_db_str(raw), Some(mode));
        }
        assert_eq!(
            ResearchMode::from_db_str("Okyanus"),
            Some(ResearchMode::Ocean)
        );
        assert_eq!(ResearchMode::from_db_str("kayip"), None);
    }

    #[test]
    fn butce_modla_birlikte_buyur() {
        let s = ResearchMode::Surface.params();
        let d = ResearchMode::Deep.params();
        let o = ResearchMode::Ocean.params();

        assert!(s.max_sources < d.max_sources && d.max_sources < o.max_sources);
        assert!(s.crawl_depth < d.crawl_depth && d.crawl_depth < o.crawl_depth);
        assert!(s.refine_rounds < d.refine_rounds && d.refine_rounds < o.refine_rounds);
        assert!(s.max_queries < d.max_queries && d.max_queries < o.max_queries);
        assert!(s.deadline < d.deadline && d.deadline < o.deadline);
    }

    #[test]
    fn yuzeysel_tek_atistir() {
        let s = ResearchMode::Surface.params();
        assert_eq!(s.refine_rounds, 1);
        assert_eq!(s.refine_fanout, 0);
        assert_eq!(s.max_provider_calls(), 1);
    }
}

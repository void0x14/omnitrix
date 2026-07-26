//! Gurultu kontrolu: tekrar eden olayda susturma (MASTER-PLAN Bolum 13).
//!
//! Ayni imzali olay, pencere dolmadan tekrar gelirse **gonderilmez**; bastirilan
//! tekrar sayisi biriktirilir ve pencere dolunca ilk gecen mesajla birlikte
//! raporlanir ("... + 12 tekrar bastirildi"). Boylece dongu halinde hata ureten
//! bir ajan telefonu dakikada bir caldirmaz ama olayin surdugu de kaybolmaz.
//!
//! Depo sinirlidir: kapasite asilinca en eski goruleni atilir, boylece uzun
//! calisan surecte harita sinirsiz buyumez (K2 — kaynak valisi ruhu).

use std::collections::HashMap;

use chrono::Duration;
use omni_proto::Timestamp;
use parking_lot::Mutex;

/// Varsayilan susturma penceresi (saniye).
pub const DEFAULT_WINDOW_SECS: i64 = 300;

/// Varsayilan imza kapasitesi.
pub const DEFAULT_CAPACITY: usize = 512;

/// Tek imzanin durumu.
#[derive(Debug, Clone, Copy)]
struct Entry {
    /// En son *gonderilen* mesajin ani.
    last_sent: Timestamp,
    /// En son *goruldugu* an (temizleme icin).
    last_seen: Timestamp,
    /// Son gonderimden bu yana bastirilan tekrar sayisi.
    suppressed: u32,
}

/// Susturma karari.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DedupVerdict {
    /// Gonder. `suppressed_since_last`, son gonderimden bu yana bastirilan
    /// tekrar sayisidir (ilk gonderimde 0).
    Send {
        /// Bastirilmis tekrar sayisi.
        suppressed_since_last: u32,
    },
    /// Gonderme. Pencere hala acik.
    Suppress {
        /// Bu imzanin kacinci bastirilan tekrari oldugu (1'den baslar).
        repeat: u32,
        /// Pencerenin dolmasina kalan saniye.
        retry_after_secs: i64,
    },
}

impl DedupVerdict {
    /// Mesaj gonderilecek mi?
    #[must_use]
    pub fn should_send(&self) -> bool {
        matches!(self, Self::Send { .. })
    }

    /// Gonderilecekse bastirilmis tekrar sayisi.
    #[must_use]
    pub fn suppressed_count(&self) -> u32 {
        match self {
            Self::Send {
                suppressed_since_last,
            } => *suppressed_since_last,
            Self::Suppress { repeat, .. } => *repeat,
        }
    }
}

/// Imza + zaman penceresi tabanli susturucu. `Send + Sync`; paylasimli
/// kullanim icin `Arc` ile sarilir.
#[derive(Debug)]
pub struct DedupWindow {
    window: Duration,
    capacity: usize,
    entries: Mutex<HashMap<String, Entry>>,
}

impl Default for DedupWindow {
    fn default() -> Self {
        Self::new(Duration::seconds(DEFAULT_WINDOW_SECS))
    }
}

impl DedupWindow {
    /// Verilen pencere ile susturucu kurar. Negatif/sifir pencere susturmayi
    /// fiilen kapatir (her olay gecer).
    #[must_use]
    pub fn new(window: Duration) -> Self {
        Self::with_capacity(window, DEFAULT_CAPACITY)
    }

    /// Pencere + kapasite ile kurar. Kapasite 0 verilirse 1'e yuvarlanir.
    #[must_use]
    pub fn with_capacity(window: Duration, capacity: usize) -> Self {
        Self {
            window,
            capacity: capacity.max(1),
            entries: Mutex::new(HashMap::new()),
        }
    }

    /// Susturma penceresi.
    #[must_use]
    pub fn window(&self) -> Duration {
        self.window
    }

    /// Izlenen imza sayisi.
    #[must_use]
    pub fn tracked(&self) -> usize {
        self.entries.lock().len()
    }

    /// Tum durumu siler.
    pub fn reset(&self) {
        self.entries.lock().clear();
    }

    /// Imzayi degerlendirir ve karari dondurur. Karar **durum degistirir**:
    /// `Send` donen cagri gonderim anini isaretler.
    pub fn admit(&self, signature: &str, now: Timestamp) -> DedupVerdict {
        let mut entries = self.entries.lock();
        Self::prune(&mut entries, self.window, now, self.capacity);

        match entries.get_mut(signature) {
            None => {
                entries.insert(
                    signature.to_owned(),
                    Entry {
                        last_sent: now,
                        last_seen: now,
                        suppressed: 0,
                    },
                );
                DedupVerdict::Send {
                    suppressed_since_last: 0,
                }
            }
            Some(entry) => {
                entry.last_seen = now;
                let elapsed = now - entry.last_sent;
                if elapsed >= self.window {
                    let suppressed = entry.suppressed;
                    entry.suppressed = 0;
                    entry.last_sent = now;
                    DedupVerdict::Send {
                        suppressed_since_last: suppressed,
                    }
                } else {
                    entry.suppressed = entry.suppressed.saturating_add(1);
                    let kalan = (self.window - elapsed).num_seconds().max(0);
                    DedupVerdict::Suppress {
                        repeat: entry.suppressed,
                        retry_after_secs: kalan,
                    }
                }
            }
        }
    }

    /// Suresi gecmis girdileri atar; hala tasiyorsa en eski goruleni dusurur.
    fn prune(
        entries: &mut HashMap<String, Entry>,
        window: Duration,
        now: Timestamp,
        capacity: usize,
    ) {
        // Pencerenin iki kati kadar gorulmeyen imza artik "tekrar" sayilmaz.
        let bayatlama = window * 2;
        entries.retain(|_, entry| now - entry.last_seen < bayatlama);

        while entries.len() >= capacity {
            let en_eski = entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_seen)
                .map(|(key, _)| key.clone());
            match en_eski {
                Some(key) => {
                    entries.remove(&key);
                }
                None => break,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t0() -> Timestamp {
        omni_proto::now()
    }

    #[test]
    fn ilk_olay_gecer() {
        let dedup = DedupWindow::new(Duration::seconds(60));
        assert_eq!(
            dedup.admit("a", t0()),
            DedupVerdict::Send {
                suppressed_since_last: 0
            }
        );
    }

    #[test]
    fn pencere_icinde_tekrar_bastirilir() {
        let dedup = DedupWindow::new(Duration::seconds(60));
        let now = t0();
        assert!(dedup.admit("a", now).should_send());
        let ikinci = dedup.admit("a", now + Duration::seconds(1));
        assert!(!ikinci.should_send());
        assert_eq!(
            ikinci,
            DedupVerdict::Suppress {
                repeat: 1,
                retry_after_secs: 59
            }
        );
        let ucuncu = dedup.admit("a", now + Duration::seconds(2));
        assert_eq!(ucuncu.suppressed_count(), 2);
    }

    #[test]
    fn pencere_dolunca_bastirilan_sayisi_raporlanir() {
        let dedup = DedupWindow::new(Duration::seconds(60));
        let now = t0();
        assert!(dedup.admit("a", now).should_send());
        for i in 1..=5 {
            assert!(!dedup.admit("a", now + Duration::seconds(i)).should_send());
        }
        assert_eq!(
            dedup.admit("a", now + Duration::seconds(60)),
            DedupVerdict::Send {
                suppressed_since_last: 5
            }
        );
        // Sayac sifirlanir.
        assert_eq!(
            dedup.admit("a", now + Duration::seconds(180)),
            DedupVerdict::Send {
                suppressed_since_last: 0
            }
        );
    }

    #[test]
    fn farkli_imzalar_birbirini_susturmaz() {
        let dedup = DedupWindow::new(Duration::seconds(60));
        let now = t0();
        assert!(dedup.admit("a", now).should_send());
        assert!(dedup.admit("b", now).should_send());
    }

    #[test]
    fn sifir_pencere_susturmayi_kapatir() {
        let dedup = DedupWindow::new(Duration::zero());
        let now = t0();
        assert!(dedup.admit("a", now).should_send());
        assert!(dedup.admit("a", now).should_send());
    }

    #[test]
    fn kapasite_asilinca_en_eski_dusulur() {
        let dedup = DedupWindow::with_capacity(Duration::seconds(600), 2);
        let now = t0();
        assert!(dedup.admit("a", now).should_send());
        assert!(dedup.admit("b", now + Duration::seconds(1)).should_send());
        assert!(dedup.admit("c", now + Duration::seconds(2)).should_send());
        assert!(dedup.tracked() <= 2);
    }

    #[test]
    fn bayat_girdi_temizlenir() {
        let dedup = DedupWindow::new(Duration::seconds(60));
        let now = t0();
        assert!(dedup.admit("a", now).should_send());
        // 3 pencere sonra "a" bayat sayilir ve silinir; yeni imza gibi gecer.
        assert_eq!(
            dedup.admit("b", now + Duration::seconds(180)),
            DedupVerdict::Send {
                suppressed_since_last: 0
            }
        );
        assert_eq!(dedup.tracked(), 1);
    }
}

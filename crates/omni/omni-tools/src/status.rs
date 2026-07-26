//! Calisma agaci durumu icin is-parcacigi butcesi (MASTER-PLAN 5.2).
//!
//! `gix` taramasi kaynak acgozludur; butce vendored `xai-gix-status`'tan
//! cozulur. Sonuc asla 0 degildir — `gix` tarafinda `Some(0)` "sinirsiz"
//! anlamina gelir ve butceyi tamamen devre disi birakirdi.

/// Durum taramasi icin onerilen is-parcacigi sayisi (her zaman >= 1).
pub fn status_thread_limit() -> usize {
    xai_gix_status::compute_gix_status_thread_limit()
}

/// Cekirdek sayisi / yumusak nproc / kullanilan parcacik uclusunden butce.
pub fn status_thread_limit_from(
    cores: usize,
    soft_nproc: Option<usize>,
    threads_used: usize,
) -> usize {
    xai_gix_status::compute_gix_status_thread_limit_from(cores, soft_nproc, threads_used)
}

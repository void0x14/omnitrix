//! Deterministik akış boru hattı denetleyicisi (Flow Governor).
//!
//! Görev akışı ASLA AI insiyatifinde değildir: akış seçimi, aşama sırası,
//! kanıt doğrulaması, süre kararı, paralel sorgu, yargıç denetimi ve ihlal
//! düzeltmesi tamamen bu modüldeki saf durum makinelerine aittir. AI yalnızca
//! mevcut aşamanın araçlarıyla çalışır ve aşamayı `flow_checkpoint` ile kapatır.

pub mod classifier;
pub mod config;
pub mod definition;
pub mod duration;
pub mod events;
pub mod gate;
pub mod governor;
pub mod judge;
pub mod notify;
pub mod parallel;
pub mod state;
pub mod store;

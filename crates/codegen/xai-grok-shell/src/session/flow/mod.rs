//! Deterministik akış boru hattı denetleyicisi (Flow Governor).
//!
//! Görev akışı ASLA AI insiyatifinde değildir: akış seçimi, aşama sırası,
//! kanıt doğrulaması ve ihlal düzeltmesi tamamen bu modüldeki saf durum
//! makinelerine aittir. AI yalnızca mevcut aşamanın araçlarıyla çalışır ve
//! aşamayı `flow_checkpoint` ile kapatır.

pub mod classifier;
pub mod definition;
pub mod duration;
pub mod gate;
pub mod state;
// pub mod events;      // Task 3
// pub mod governor;    // Task 3
// pub mod store;       // Task 3

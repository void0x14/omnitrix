//! Deterministik akış boru hattı denetleyicisi (Flow Governor).
//!
//! Görev akışı ASLA AI insiyatifinde değildir: akış seçimi, aşama sırası,
//! kanıt doğrulaması ve ihlal düzeltmesi tamamen bu modüldeki saf durum
//! makinelerine aittir. AI yalnızca mevcut aşamanın araçlarıyla çalışır ve
//! aşamayı `flow_checkpoint` ile kapatır.

pub mod definition;
pub mod state;
// pub mod classifier;  // Task 2
// pub mod duration;    // Task 2
// pub mod events;      // Task 3
// pub mod gate;        // Task 2
// pub mod governor;    // Task 3
// pub mod store;       // Task 3

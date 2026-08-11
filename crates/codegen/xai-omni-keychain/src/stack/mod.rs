//! Harici AI coding stack ↔ Omnitrix keychain senkronu.
//!
//! Mimari (hardcoded tek-tool değil):
//! - [`catalog`] veri odaklı stack tanımları (path adayları + format ailesi)
//! - [`formats`] format motorları (aynı formatı paylaşan tüm araçlar)
//! - [`sync`] merge-only senkron (varsayılan: conflict skip, ezme yok)
//!
//! Yeni araç = katalog satırı (+ gerekirse yeni format motoru).

mod catalog;
mod formats;
mod sync;

pub use catalog::{
    all_stack_defs, default_or_existing_path, detect_stacks, find_stack_def, resolve_stack_path,
    StackCapability, StackDef, StackPresence,
};
pub use formats::{extract_api_keys, FormatKind, RawKey};
pub use sync::{
    export_to_stack, import_from_stack, preview_export, preview_import, MergePolicy, PreviewEntry,
    SyncDirection, SyncPreview, SyncSummary,
};

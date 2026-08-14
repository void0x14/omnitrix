//! Omnitrix şifreli API key deposu.
//!
//! Key'ler `~/.grok/keychain.omx` dosyasında AES-256-GCM ile şifrelenir.
//! Anahtar kullanıcının belirlediği master password'den Argon2id ile türetilir
//! ve asla diske yazılmaz. RAM'deki hassas değerler `zeroize` ile sıfırlanır.

pub mod crypto;
pub mod detect;
mod export;
pub mod keyring_store;
pub mod stack;
mod store;
mod ttl;

pub use detect::*;
pub use export::{export_keychain, import_keychain, ExportScope, ImportSummary};
pub use stack::{
    all_stack_defs, default_or_existing_path, detect_stacks, export_to_stack, find_stack_def,
    import_from_stack, preview_export, preview_import, resolve_stack_path, FormatKind, MergePolicy,
    PreviewEntry, StackCapability, StackDef, StackPresence, SyncDirection, SyncPreview,
    SyncSummary,
};
pub use store::{
    auto_category, detect_key_type, BorrowedKey, KeyEntry, KeyId, KeySource, KeyType, Keychain,
    KeychainError, KeychainOptions,
};
pub use ttl::{MasterKeyCache, MasterKeyTtl};

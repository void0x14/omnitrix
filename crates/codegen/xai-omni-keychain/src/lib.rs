//! Omnitrix şifreli API key deposu.
//!
//! Key'ler `~/.grok/keychain.omx` dosyasında AES-256-GCM ile şifrelenir.
//! Anahtar kullanıcının belirlediği master password'den Argon2id ile türetilir
//! ve asla diske yazılmaz. RAM'deki hassas değerler `zeroize` ile sıfırlanır.

pub mod crypto;

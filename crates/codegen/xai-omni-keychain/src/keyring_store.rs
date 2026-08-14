//! Master password'ün OS keyring'inde saklanması (Secret Service / libsecret).
//!
//! Kullanıcı master password'ü bir kez girer; bu modül onu sistem
//! anahtarlığına yazar (`omnitrix-keychain` servisi, `master` kullanıcısı).
//! Sonraki CLI/TUI açılışlarında şifre anahtarlıktan sessizce okunur —
//! yeniden prompt gösterilmez. Linux'ta sync-secret-service (dbus üzerinden
//! Secret Service), macOS'ta apple-native, Windows'ta windows-native backend
//! kullanılır.

use std::io;

/// Anahtarlık servis adı (`keyring::Entry::new`'in ilk parametresi).
const KEYRING_SERVICE: &str = "omnitrix-keychain";
/// Anahtarlık kullanıcı adı (`keyring::Entry::new`'in ikinci parametresi).
const KEYRING_USER: &str = "master";

/// Anahtarlıkta saklı master password'ü döner.
///
/// - `Ok(password)`: anahtarlıkta kayıt var.
/// - `Err(NotFound)`: anahtarlıkta kayıt yok (`NoEntry`).
/// - `Err(Other)`: anahtarlığa erişilemedi (Secret Service daemon'u yok,
///   dbus kapalı, …).
pub fn master_password_get() -> io::Result<String> {
    entry()?
        .get_password()
        .map_err(|e| match e {
            keyring::Error::NoEntry => {
                io::Error::new(io::ErrorKind::NotFound, "anahtarlıkta kayıt yok")
            }
            other => io::Error::new(
                io::ErrorKind::Other,
                format!("sistem anahtarlığına erişilemedi: {other}"),
            ),
        })
}

/// Master password'ü anahtarlığa yazar (mevcut kaydın üzerine yazar).
pub fn master_password_set(password: &str) -> io::Result<()> {
    entry()?.set_password(password).map_err(|e| {
        io::Error::new(
            io::ErrorKind::Other,
            format!("sistem anahtarlığına yazılamadı: {e}"),
        )
    })
}

/// Master password kaydını anahtarlıktan siler. Kayıt yoksa sessizce başarılı
/// döner (`NoEntry` → `Ok`).
pub fn master_password_delete() -> io::Result<()> {
    match entry()?.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(io::Error::new(
            io::ErrorKind::Other,
            format!("anahtarlıktan silinemedi: {e}"),
        )),
    }
}

/// Anahtarlığın erişilebilir olup olmadığını söyler.
///
/// Okunan kayıt boş bile olsa (anahtarlık çalışıyor, kayıt yok) true döner;
/// yalnızca sert hatalarda (Secret Service daemon'una bağlanılamadı, dbus
/// kapalı, platform hatası) false döner.
pub fn has_keyring() -> bool {
    match master_password_get() {
        Ok(_) => true,
        Err(e) => e.kind() == io::ErrorKind::NotFound,
    }
}

/// `omnitrix-keychain` / `master` kaydına erişim; giriş başarısızlığını
/// Türkçe `io::Error`'a çevirir.
fn entry() -> io::Result<keyring::Entry> {
    keyring::Entry::new(KEYRING_SERVICE, KEYRING_USER).map_err(|e| {
        io::Error::new(
            io::ErrorKind::Other,
            format!("sistem anahtarlığına erişilemedi: {e}"),
        )
    })
}

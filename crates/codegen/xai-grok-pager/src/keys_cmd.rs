//! `grok keys` — şifreli API key keychain yönetimi.
//!
//! Her alt komut keychain'i [`xai_omni_keychain::Keychain::open`] ile açar;
//! master password gizli okunur (terminal echo kapatılarak). İlk açılışta
//! dosya olmadığından yeni keychain yaratılır — Task 5 kapsamında master
//! password yalnızca bir kez sorulur, onay tekrarı yoktur (brief'ten sapma,
//! rapora işlendi).
//!
//! Çıktı dili Türkçe; çıktı formatları brief'teki gibidir.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use xai_omni_keychain::{
    ExportScope, ImportSummary, Keychain, KeychainError, KeychainOptions, MasterKeyTtl,
    MergePolicy, all_stack_defs, detect_stacks, export_to_stack, find_stack_def, import_from_stack,
    preview_export, preview_import,
};

use crate::app::cli::{KeysArgs, KeysCommand};

/// `~/.grok` altındaki keychain dosyası adı (path'i çağıran verir).
pub const KEYCHAIN_FILE: &str = "keychain.omx";

/// `grok keys` giriş noktası. `grok_home` `~/.grok` (GROK_HOME'a duyarlı).
pub async fn run(keys_args: KeysArgs, grok_home: PathBuf) -> anyhow::Result<()> {
    match keys_args.command {
        KeysCommand::List => cmd_list(&grok_home),
        KeysCommand::Show { id } => cmd_show(&grok_home, &id),
        KeysCommand::Add {
            provider,
            api_key,
            model,
            base_url,
        } => cmd_add(
            &grok_home,
            &provider,
            api_key.as_deref(),
            model.as_deref(),
            base_url.as_deref(),
        ),
        KeysCommand::Edit {
            id,
            model,
            base_url,
            api_key,
        } => cmd_edit(
            &grok_home,
            &id,
            model.as_deref(),
            base_url.as_deref(),
            api_key.as_deref(),
        ),
        KeysCommand::Remove { id } => cmd_remove(&grok_home, &id),
        KeysCommand::Export { path, category } => cmd_export(&grok_home, path, &category),
        KeysCommand::Import { path, overwrite } => cmd_import(&grok_home, &path, overwrite),
        KeysCommand::Categories => cmd_categories(&grok_home),
        KeysCommand::Stacks { detected } => cmd_stacks(detected),
        KeysCommand::SyncFrom {
            stack,
            path,
            dry_run,
            overwrite,
        } => cmd_sync_from(&grok_home, &stack, path.as_deref(), dry_run, overwrite),
        KeysCommand::SyncTo {
            stack,
            path,
            dry_run,
            overwrite,
        } => cmd_sync_to(&grok_home, &stack, path.as_deref(), dry_run, overwrite),
    }
}

// ---------------------------------------------------------------------------
// Alt komutlar
// ---------------------------------------------------------------------------

fn cmd_list(grok_home: &Path) -> anyhow::Result<()> {
    let mut kc = prompt_and_open_keychain(grok_home)?;
    let entries = kc.list_keys()?;
    println!("{}", format_list_header());
    for entry in &entries {
        println!("{}", format_list_row(entry));
    }
    println!();
    println!("{} kayıt (grok keys show <id> ile tam key)", entries.len());
    Ok(())
}

fn cmd_show(grok_home: &Path, id: &str) -> anyhow::Result<()> {
    let mut kc = prompt_and_open_keychain(grok_home)?;
    let entry = kc
        .list_keys()?
        .into_iter()
        .find(|e| e.id == id)
        .ok_or_else(|| anyhow::anyhow!("key bulunamadı: {id} (grok keys list)"))?;
    let secret = kc.reveal(id.to_string())?;
    println!("Provider: {}", entry.provider_id);
    println!("Category: {}", entry.category);
    println!("Tip: {}", entry.key_type.label());
    if let Some(b) = entry.balance {
        println!("Bakiye: ${b:.2}");
    }
    println!("API Key: {}", secret.as_str());
    println!("Model: {}", entry.model_id.as_deref().unwrap_or("-"));
    println!("ID: {}", entry.id);
    Ok(())
}

fn cmd_add(
    grok_home: &Path,
    provider: &str,
    api_key: Option<&str>,
    model: Option<&str>,
    base_url: Option<&str>,
) -> anyhow::Result<()> {
    let mut kc = prompt_and_open_keychain(grok_home)?;
    let api_key = match api_key {
        Some(k) => k.to_string(),
        None => read_secret("API key: ").context("API key okunamadı")?,
    };
    if api_key.trim().is_empty() {
        anyhow::bail!("API key boş olamaz");
    }
    // Kategori otomatik: sağlayıcıya göre sistem belirler (kullanıcı seçmez).
    let category = xai_omni_keychain::auto_category(provider);
    let id = kc.add_key_auto(
        provider,
        &api_key,
        model.map(str::to_string),
        base_url.map(str::to_string),
    )?;
    kc.save()?;
    println!("keychain'e eklendi: {provider} ({category}) [{id}]");
    Ok(())
}

fn cmd_edit(
    grok_home: &Path,
    id: &str,
    model: Option<&str>,
    base_url: Option<&str>,
    api_key: Option<&str>,
) -> anyhow::Result<()> {
    let mut kc = prompt_and_open_keychain(grok_home)?;
    kc.update_key(
        id.to_string(),
        model.map(str::to_string),
        base_url.map(str::to_string),
        api_key.map(str::to_string),
    )?;
    kc.save()?;
    println!("güncellendi: {id}");
    Ok(())
}

fn cmd_remove(grok_home: &Path, id: &str) -> anyhow::Result<()> {
    let mut kc = prompt_and_open_keychain(grok_home)?;
    kc.remove_key(id.to_string())
        .map_err(|e| anyhow::anyhow!("silme başarısız: {e}"))?;
    kc.save()?;
    println!("silindi: {id}");
    Ok(())
}

fn cmd_export(
    grok_home: &Path,
    path: Option<PathBuf>,
    categories: &[String],
) -> anyhow::Result<()> {
    let mut kc = prompt_and_open_keychain(grok_home)?;
    let scope = if categories.is_empty() {
        ExportScope::All
    } else {
        ExportScope::Categories(categories.to_vec())
    };
    let entries = kc.list_keys()?;
    let exported_count = entries
        .iter()
        .filter(|e| categories.is_empty() || categories.iter().any(|c| c == &e.category))
        .count();
    let path = match path {
        Some(p) => p,
        None => default_export_path(grok_home, unix_ts()),
    };
    let password = read_secret("export şifresi: ").context("export şifresi okunamadı")?;
    let confirmation =
        read_secret("export şifresi (tekrar): ").context("export şifresi okunamadı")?;
    if password.is_empty() || password != confirmation {
        anyhow::bail!("export şifreleri boş veya eşleşmiyor");
    }
    let bytes = xai_omni_keychain::export_keychain(&kc, scope, &password)?;
    std::fs::write(&path, &bytes)
        .with_context(|| format!("export dosyası yazılamadı: {}", path.display()))?;
    println!("export edildi: {} ({} key)", path.display(), exported_count);
    Ok(())
}

fn cmd_import(grok_home: &Path, path: &Path, overwrite: bool) -> anyhow::Result<()> {
    let mut kc = prompt_and_open_keychain(grok_home)?;
    let bytes = std::fs::read(path)
        .with_context(|| format!("export dosyası okunamadı: {}", path.display()))?;
    let password = read_secret("export şifresi: ").context("export şifresi okunamadı")?;
    let summary = xai_omni_keychain::import_keychain(&mut kc, &bytes, &password, overwrite)
        .map_err(|e| anyhow::anyhow!("import başarısız: {e}"))?;
    kc.save()?;
    println!("{}", format_import_summary(&summary));
    Ok(())
}

fn cmd_categories(grok_home: &Path) -> anyhow::Result<()> {
    let kc = prompt_and_open_keychain(grok_home)?;
    let default = kc.default_category();
    let mut categories = kc.categories();
    categories.sort();
    for category in categories {
        if category == default {
            println!("{category} (varsayilan)");
        } else {
            println!("{category}");
        }
    }
    Ok(())
}

fn cmd_stacks(detected_only: bool) -> anyhow::Result<()> {
    if detected_only {
        let found = detect_stacks();
        if found.is_empty() {
            println!("tespit edilen stack yok");
            return Ok(());
        }
        println!("{:<16}  {:<22}  {}", "ID", "LABEL", "PATH");
        for p in found {
            let path = p
                .path
                .as_ref()
                .map(|x| x.display().to_string())
                .unwrap_or_else(|| "-".into());
            println!("{:<16}  {:<22}  {}", p.def.id, p.def.label, path);
        }
        return Ok(());
    }
    println!(
        "{:<16}  {:<6}  {:<6}  {:<22}  {}",
        "ID", "IMP", "EXP", "LABEL", "AÇIKLAMA"
    );
    for def in all_stack_defs() {
        let imp = if def.capability.import {
            "evet"
        } else {
            "hayır"
        };
        let exp = if def.capability.export {
            "evet"
        } else {
            "hayır"
        };
        println!(
            "{:<16}  {:<6}  {:<6}  {:<22}  {}",
            def.id, imp, exp, def.label, def.description
        );
    }
    println!();
    println!("kullanım:");
    println!("  grok keys sync-from <stack> [--path PATH] [--dry-run] [--overwrite]");
    println!("  grok keys sync-to   <stack> [--path PATH] [--dry-run] [--overwrite]");
    println!("  grok keys stacks --detected");
    Ok(())
}

fn cmd_sync_from(
    grok_home: &Path,
    stack: &str,
    path: Option<&Path>,
    dry_run: bool,
    overwrite: bool,
) -> anyhow::Result<()> {
    let def = find_stack_def(stack)
        .ok_or_else(|| anyhow::anyhow!("bilinmeyen stack: {stack} (grok keys stacks)"))?;
    if !def.capability.import {
        anyhow::bail!("{} import desteklemiyor", def.id);
    }
    let policy = if overwrite {
        MergePolicy::Overwrite
    } else {
        MergePolicy::SkipConflicts
    };
    let mut kc = prompt_and_open_keychain(grok_home)?;
    if dry_run {
        let preview = preview_import(def, path, &kc)
            .map_err(|e| anyhow::anyhow!("önizleme başarısız: {e}"))?;
        println!(
            "önizleme {} → omnitrix  ({})",
            preview.stack_label,
            preview.path.display()
        );
        println!(
            "aday: {}  conflict: {}",
            preview.candidates.len(),
            preview.conflict_count
        );
        for c in &preview.candidates {
            let mark = if c.conflict { "CONFLICT" } else { "yeni" };
            println!(
                "  [{mark}] {}  {}  ({})",
                c.provider_id, c.masked, c.source_field
            );
        }
        if !overwrite && preview.conflict_count > 0 {
            println!(
                "not: conflict'ler atlanacak (varsayılan merge); --overwrite ile üzerine yazılır"
            );
        }
        return Ok(());
    }
    let summary = import_from_stack(def, path, &mut kc, policy)
        .map_err(|e| anyhow::anyhow!("sync-from başarısız: {e}"))?;
    kc.save()?;
    println!("{}", summary.format_tr());
    if !summary.skipped_conflicts.is_empty() {
        println!("atlanan conflict: {}", summary.skipped_conflicts.join(", "));
    }
    Ok(())
}

fn cmd_sync_to(
    grok_home: &Path,
    stack: &str,
    path: Option<&Path>,
    dry_run: bool,
    overwrite: bool,
) -> anyhow::Result<()> {
    let def = find_stack_def(stack)
        .ok_or_else(|| anyhow::anyhow!("bilinmeyen stack: {stack} (grok keys stacks)"))?;
    if !def.capability.export {
        anyhow::bail!("{} export desteklemiyor", def.id);
    }
    let policy = if overwrite {
        MergePolicy::Overwrite
    } else {
        MergePolicy::SkipConflicts
    };
    let kc = prompt_and_open_keychain(grok_home)?;
    if dry_run {
        let preview = preview_export(def, path, &kc)
            .map_err(|e| anyhow::anyhow!("önizleme başarısız: {e}"))?;
        println!(
            "önizleme omnitrix → {}  ({})",
            preview.stack_label,
            preview.path.display()
        );
        println!(
            "aday: {}  conflict: {}",
            preview.candidates.len(),
            preview.conflict_count
        );
        for c in &preview.candidates {
            let mark = if c.conflict { "CONFLICT" } else { "yeni" };
            println!("  [{mark}] {}  {}", c.provider_id, c.masked);
        }
        if !overwrite && preview.conflict_count > 0 {
            println!("not: conflict'ler atlanacak; hedef kayıtlar ezilmez");
        }
        return Ok(());
    }
    let summary = export_to_stack(def, path, &kc, policy)
        .map_err(|e| anyhow::anyhow!("sync-to başarısız: {e}"))?;
    println!("{}", summary.format_tr());
    if !summary.skipped_conflicts.is_empty() {
        println!("atlanan conflict: {}", summary.skipped_conflicts.join(", "));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Keychain açma + gizli giriş
// ---------------------------------------------------------------------------

/// Master password'ü gizli okur ve keychain'i açar. Dosya yoksa yeni keychain
/// yaratılır (master password tek sefer sorulur, onay yok — Task 5 sapması).
/// `connect_cmd` de aynı açılışı kullanır (pub(crate)).
pub(crate) fn prompt_and_open_keychain(grok_home: &Path) -> anyhow::Result<Keychain> {
    let path = grok_home.join(KEYCHAIN_FILE);
    let first_open = !path.exists();
    let prompt = if first_open {
        "yeni keychain: master password belirle: "
    } else {
        "keychain master password: "
    };
    let password = read_secret(prompt).context("master password okunamadı")?;
    if password.is_empty() {
        anyhow::bail!("master password boş olamaz");
    }
    Keychain::open(
        KeychainOptions {
            path: Some(path),
            ttl: MasterKeyTtl::default(),
        },
        || password,
    )
    .map_err(keychain_error)
}

/// Keychain hatalarını Türkçe/Türkçe-karışık anlaşılır mesajlara çevirir.
fn keychain_error(e: KeychainError) -> anyhow::Error {
    match e {
        KeychainError::WrongPassword => {
            anyhow::anyhow!("yanlış master password; keychain açılamadı")
        }
        KeychainError::Locked => {
            anyhow::anyhow!("keychain kilitli; master password yeniden girilmeli")
        }
        other => anyhow::anyhow!("keychain açılamadı: {other}"),
    }
}

/// Gizli girdi okuma: prompt stderr'e basılır (stdout çıktıları temiz kalır),
/// Unix'te terminal echo kapatılır, okuma sonrası geri açılır. TTY değilse
/// düz stdin okur (pipe'tan master password verilebilir).
pub fn read_secret(prompt: &str) -> std::io::Result<String> {
    eprint!("{prompt}");
    std::io::stderr().flush()?;
    let mut line = String::new();
    #[cfg(unix)]
    let echo = disable_terminal_echo();
    let read = std::io::stdin().read_line(&mut line);
    #[cfg(unix)]
    restore_terminal_echo(echo);
    read?;
    eprintln!();
    Ok(line.trim_end_matches(['\n', '\r']).to_string())
}

/// Echo'yu kapatır; önceki durumu döner (restore için).
#[cfg(unix)]
fn disable_terminal_echo() -> Option<std::os::fd::RawFd> {
    use std::os::fd::RawFd;
    let fd: RawFd = libc::STDIN_FILENO;
    // termios yoksa (TTY değil) dokunmadan None.
    let mut termios: libc::termios = unsafe { std::mem::zeroed() };
    if unsafe { libc::tcgetattr(fd, &mut termios) } != 0 {
        return None;
    }
    termios.c_lflag &= !(libc::ECHO | libc::ECHONL);
    if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &termios) } != 0 {
        return None;
    }
    Some(fd)
}

/// Echo'yu tekrar açar (yalnızca biz kapattıysak).
#[cfg(unix)]
fn restore_terminal_echo(fd: Option<std::os::fd::RawFd>) {
    let Some(fd) = fd else { return };
    let mut termios: libc::termios = unsafe { std::mem::zeroed() };
    if unsafe { libc::tcgetattr(fd, &mut termios) } != 0 {
        return;
    }
    termios.c_lflag |= libc::ECHO | libc::ECHONL;
    let _ = unsafe { libc::tcsetattr(fd, libc::TCSANOW, &termios) };
}

// ---------------------------------------------------------------------------
// Çıktı formatları (saf fonksiyonlar — test edilebilir)
// ---------------------------------------------------------------------------

/// `KATEGORI  PROVIDER  MASKELI  MODEL  SON_KULLANIM  ID` başlığı.
pub fn format_list_header() -> String {
    let mut out = String::new();
    out.push_str(&pad("KATEGORI", 16));
    out.push_str("  ");
    out.push_str(&pad("PROVIDER", 16));
    out.push_str("  ");
    out.push_str(&pad("MASKELI", 24));
    out.push_str("  ");
    out.push_str(&pad("MODEL", 20));
    out.push_str("  ");
    out.push_str(&pad("SON_KULLANIM", 12));
    out.push_str("  ");
    out.push_str("ID");
    out
}

/// Tek kayıt satırı (maskeli görünüm; ham key buraya asla girmez).
pub fn format_list_row(entry: &xai_omni_keychain::KeyEntry) -> String {
    let mut out = String::new();
    out.push_str(&pad(&entry.category, 16));
    out.push_str("  ");
    out.push_str(&pad(&entry.provider_id, 16));
    out.push_str("  ");
    out.push_str(&pad(&entry.masked, 24));
    out.push_str("  ");
    out.push_str(&pad(entry.model_id.as_deref().unwrap_or("-"), 20));
    out.push_str("  ");
    out.push_str(&pad(
        entry.last_used.as_deref().map(short_date).unwrap_or("-"),
        12,
    ));
    out.push_str("  ");
    out.push_str(&entry.id);
    out
}

/// RFC3339 zaman damgasının tarih kısmı (`2026-08-09T12:34:56Z` → `2026-08-09`);
/// tarih biçiminde değilse olduğu gibi döner.
pub fn short_date(rfc3339: &str) -> &str {
    if rfc3339.len() >= 10
        && rfc3339.as_bytes()[..10]
            .iter()
            .all(|b| b.is_ascii_digit() || *b == b'-')
    {
        &rfc3339[..10]
    } else {
        rfc3339
    }
}

/// Alanı genişliğe sola yaslar (uzun değerler kesilir).
pub fn pad(s: &str, width: usize) -> String {
    let mut out: String = s.chars().take(width).collect();
    while out.chars().count() < width {
        out.push(' ');
    }
    out
}

/// Varsayılan export yolu: `~/.grok/keychain-export-<unix_ts>.omx`.
pub fn default_export_path(grok_home: &Path, ts: u64) -> PathBuf {
    grok_home.join(format!("keychain-export-{ts}.omx"))
}

fn unix_ts() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Import özetini brief formatında yazar: `import edildi: N key (üzerine
/// yazılan: A, atlanan: B)`.
pub fn format_import_summary(summary: &ImportSummary) -> String {
    format!(
        "import edildi: {} key (üzerine yazılan: {}, atlanan: {})",
        summary.imported_keys,
        summary.overwritten.len(),
        summary.skipped.len(),
    )
}

#[cfg(test)]
#[path = "keys_cmd_tests.rs"]
mod tests; // ayrı test dosyası (keys_cmd_tests.rs)

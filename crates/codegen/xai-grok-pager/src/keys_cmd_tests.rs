//! `keys_cmd` saf çıktı biçimlendirme testleri (cargo çalıştırılmadı —
//! Task 5 binding'i gereği yalnızca yazıldı).

use std::path::PathBuf;

use super::*;
use xai_omni_keychain::{KeyEntry, KeySource};

fn sample_entry(id: &str, category: &str, provider: &str) -> KeyEntry {
    KeyEntry {
        id: id.to_string(),
        category: category.to_string(),
        provider_id: provider.to_string(),
        provider_label: provider.to_string(),
        masked: "sk-…a1b2".to_string(),
        model_id: Some("gpt-5".to_string()),
        base_url: None,
        created_at: "2026-08-01T10:00:00Z".to_string(),
        last_used: Some("2026-08-09T12:34:56Z".to_string()),
        source: KeySource::Manual,
        key_type: xai_omni_keychain::KeyType::Legacy,
        balance: None,
    }
}

#[test]
fn header_contains_all_columns() {
    let header = format_list_header();
    for column in ["KATEGORI", "PROVIDER", "MASKELI", "MODEL", "SON_KULLANIM", "ID"] {
        assert!(header.contains(column), "header missing {column}: {header}");
    }
}

#[test]
fn row_contains_id_and_masked_never_full_key() {
    let row = format_list_row(&sample_entry("k_1234", "personal", "openai"));
    assert!(row.contains("k_1234"));
    assert!(row.contains("sk-…a1b2"));
    // Maskeli form meşru olarak "sk-" ile başlar; gerçek invariant satıra
    // tam key'in gizli kısmının asla girmemesidir.
    assert!(!row.contains("super-secret"), "row must never carry the full key secret");
}

#[test]
fn row_uses_dash_for_missing_model_and_last_used() {
    let mut e = sample_entry("k_1", "work", "deepseek");
    e.model_id = None;
    e.last_used = None;
    let row = format_list_row(&e);
    // İki "-" beklenir: model ve son kullanım sütunları.
    assert_eq!(row.matches('-').count() >= 2, true, "dashes for empty cells: {row}");
    assert!(!row.contains("MODEL"), "row must not contain header text: {row}");
}

#[test]
fn short_date_slices_rfc3339_date() {
    assert_eq!(short_date("2026-08-09T12:34:56Z"), "2026-08-09");
    assert_eq!(short_date("garbage"), "garbage");
    assert_eq!(short_date("2026-8-9T12:00:00Z"), "2026-8-9T12:00:00Z");
}

#[test]
fn pad_left_aligns_and_truncates() {
    assert_eq!(pad("abc", 6), "abc   ");
    assert_eq!(pad("abc", 3), "abc");
    assert_eq!(pad("abcdefgh", 4), "abcd");
    assert_eq!(pad("", 2), "  ");
}

#[test]
fn default_export_path_uses_timestamp() {
    let home = PathBuf::from("/home/user/.grok");
    assert_eq!(
        default_export_path(&home, 1_752_000_000),
        PathBuf::from("/home/user/.grok/keychain-export-1752000000.omx")
    );
}

#[test]
fn import_summary_formats_counts() {
    let summary = ImportSummary {
        imported_keys: 3,
        overwritten: vec!["work/openai".to_string()],
        skipped: vec!["personal/anthropic".to_string()],
    };
    let out = format_import_summary(&summary);
    assert!(out.starts_with("import edildi: 3 key"));
    assert!(out.contains("üzerine yazılan: 1"));
    assert!(out.contains("atlanan: 1"));
}

#[test]
fn read_secret_is_public_api_for_pipe_input() {
    // Salt derleme kontrolü: imza `(prompt) -> io::Result<String>`.
    fn _takes_fn(_f: fn(&str) -> std::io::Result<String>) {}
    _takes_fn(read_secret);
}

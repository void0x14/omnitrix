//! `/keys` keychain manager modal.
//!
//! Tablo görünümü: `Kategori | Provider | Maskeli | Model | Son Kullanım`.
//! Alt action çubuğu: `r` reveal, `a` add, `e` edit, `x` remove, `X` remove
//! category, `c` categories, `E` export, `I` import, `S` stack sync, `Esc` kapat.
//!
//! Güvenlik kuralı: satırlar her zaman `KeyEntry.masked` gösterir; ham key
//! yalnızca `Reveal` modunda RAM'e çözülür (`Zeroizing`) ve `Esc` ile
//! gizlenir. Master/export şifreleri maskeli editörle girilir.
//!
//! **Mimari:** bu modül pure-state'dir — keychain'e asla doğrudan dokunmaz.
//! Keychain işlemleri [`KeysManagerOutcome::Action`] olarak dispatch
//! katmanına gider (`app/dispatch/connect.rs`; invariant: dispatch
//! RAM mutasyonları + küçük atomik `save()` yazmaları yapar — transcript
//! export dispatch'iyle aynı model). Dispatch sonucu state'e geri yazılır
//! (provider wizard'ın `ConnectStep` deseni).

use std::path::PathBuf;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEventKind};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
use zeroize::Zeroizing;

use xai_omni_keychain::{ExportScope, ImportSummary, KeyEntry};

use crate::app::actions::Action;
use crate::input::line_editor::LineEditor;
use crate::keys_cmd::{format_import_summary, short_date};
use crate::theme::Theme;
use crate::views::modal_window::{ModalSizing, ModalWindowConfig, ModalWindowState, Shortcut};

// ---------------------------------------------------------------------------
// Modlar
// ---------------------------------------------------------------------------

/// Türetilmiş kategori filtresi — kullanıcı kategori "ayarı" yapmaz; sistem
/// key'leri sağlayıcıya göre otomatik kategoriler, bakiye/key tipi verileriyle
/// türetilmiş gruplar halinde sunar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CategoryFilter {
    /// Tümü (filtre yok).
    All,
    /// Bakiye sorgusu ≥ $10 ("yüksek bakiyeli").
    HighBalance,
    /// Aynı sağlayıcıdan ≥ 3 key ("çoklu anahtar").
    MultiKey,
    /// Key tipi grubu (proje / servis hesabı / legacy…).
    KeyType(xai_omni_keychain::KeyType),
    /// Sağlayıcı kategorisi (otomatik kategori — keychain'de yazılıdır).
    Provider(String),
}

impl CategoryFilter {
    /// Kısa liste etiketi.
    pub fn label(&self) -> String {
        match self {
            Self::All => "tümü".to_string(),
            Self::HighBalance => "yüksek bakiyeli ($10+)".to_string(),
            Self::MultiKey => "çoklu anahtar (3+)".to_string(),
            Self::KeyType(t) => format!("{} anahtarları", t.label()),
            Self::Provider(p) => p.clone(),
        }
    }

    /// Bu kayıt filtreye uyuyor mu?
    pub fn matches(&self, entry: &KeyEntry) -> bool {
        match self {
            Self::All => true,
            Self::HighBalance => entry.balance.is_some_and(|b| b >= 10.0),
            Self::MultiKey => false, // giriş sayısına bağlı; aşağıda hesaplanır
            Self::KeyType(t) => entry.key_type == *t,
            Self::Provider(p) => entry.provider_id == *p,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeysManagerMode {
    /// Keychain yok/kilitli: master password mini-input (maskeli).
    Unlock { error: Option<String> },
    /// Tablo + action çubuğu.
    Browse,
    /// Tam key görünümü. `full_key` yalnızca bu modda RAM'de yaşar; Esc ile
    /// mod değişir ve `Zeroizing` drop'ta sıfırlanır.
    Reveal {
        id: String,
        full_key: Zeroizing<String>,
    },
    /// `emin misin? y/n`
    ConfirmRemove { id: String },
    /// `X` — kategori silme onayı.
    ConfirmRemoveCategory { name: String },
    /// Yeni key formu (provider / key / kategori / model / base_url).
    Add,
    /// Kayıt düzenleme (model / base_url / opsiyonel key).
    Edit { id: String },
    /// Kategori listesi (varsayılan "(varsayilan)" işaretli; Enter set default).
    Categories,
    /// Export kapsamı: 0 = All, 1.. = kategoriler.
    ExportScope,
    /// Export şifresi (maskeli) — Enter aksiyon üretir.
    ExportPassword { scope: ExportScope },
    /// Export tamamlandı: yol + sayı.
    ExportDone { path: String, count: usize },
    /// Import dosya yolu girişi.
    ImportPath,
    /// Import export şifresi (maskeli).
    ImportPassword { path: String },
    /// Import özeti (toast benzeri ekran).
    ImportDone { summary: ImportSummary },
    /// Harici stack seçimi (tespit edilenler).
    StackPick,
    /// Seçilen stack yönü: scope_cursor 0=from stack, 1=to stack.
    StackDirection {
        stack_id: String,
        stack_label: String,
    },
    /// Önizleme + Enter onay.
    StackConfirm {
        stack_id: String,
        stack_label: String,
        into_omnitrix: bool,
        lines: Vec<String>,
        conflict_count: usize,
    },
    /// Stack sync sonucu.
    StackDone { message: String },
}

// ---------------------------------------------------------------------------
// Durum
// ---------------------------------------------------------------------------

pub struct KeysManagerState {
    pub window: ModalWindowState,
    pub entries: Vec<KeyEntry>,
    /// Export kapsamı için kategori adları (otomatik kategoriler).
    pub categories: Vec<String>,
    pub default_category: String,
    /// Aktif türetilmiş kategori filtresi (`None` = tümü).
    pub category_filter: Option<CategoryFilter>,
    /// Kategoriler ekranı satırları (türetilmiş; `c` ile girilir).
    pub category_rows: Vec<CategoryFilter>,
    pub selected: usize,
    pub scroll_offset: usize,
    pub mode: KeysManagerMode,
    /// Unlock için keychain dosya yolu. `None` → `$GROK_HOME/keychain.omx`
    /// (testler buraya tempdir koyar).
    pub keychain_path: Option<PathBuf>,
    /// Export varsayılan dizini. `None` → `grok_home()`.
    pub export_dir: Option<PathBuf>,
    /// OpenCode auth.json override. `None` olduğunda kurulu OpenCode yolu
    /// katalogdan çözülür. Testler ve taşınabilir kurulumlar kullanır.
    pub opencode_auth_path: Option<PathBuf>,
    /// OpenCode key'leri içe alındıktan sonra models.dev sonucu geldiğinde
    /// kullanılabilir provider/model'i bağlama isteği.
    pub auto_activate_opencode: bool,
    /// OpenCode config'indeki tercih sırası (`provider/model`).
    pub opencode_preferred_models: Vec<String>,
    /// OpenCode config'indeki özel/local provider model kayıtları.
    pub opencode_provider_models: Vec<crate::app::actions::OpenCodeProviderModel>,
    /// Bu taramada OpenCode auth.json içinden gelen provider kimlikleri.
    /// Otomatik etkinleştirme keychain'deki ilgisiz kayıtları kullanmaz.
    pub opencode_provider_ids: Vec<String>,
    /// Başarılı otomatik işlem bilgisi; hata değildir ve tabloyu kapatmaz.
    pub notice: Option<String>,
    // ── giriş editörleri (formlar / şifreler) ──
    pub master_editor: LineEditor,
    pub show_master: bool,
    pub path_editor: LineEditor,
    pub provider_editor: LineEditor,
    pub key_editor: LineEditor,
    pub model_editor: LineEditor,
    pub base_url_editor: LineEditor,
    /// Odaklanan form alanı indeksi (Add: 4 alan, Edit: 3 alan).
    pub form_field: usize,
    pub category_cursor: usize,
    pub scope_cursor: usize,
    /// Stack pick satırları: (id, label, path_display).
    pub stack_rows: Vec<(String, String, String)>,
    /// `/import <stack>` / `/export <stack>` slash komutu kilitli keychain
    /// yüzünden beklerken kuyruğa alınan iş: `(stack_id, into_omnitrix)`.
    /// Unlock başarılı olunca `dispatch_keychain_unlock` bu işi alır,
    /// senkronu çalıştırır ve sonucu scrollback'e yazar.
    pub pending_stack_sync: Option<(String, bool)>,
    /// Modal başlık override'ı (`/import <stack>` / `/export <stack>`
    /// akışında varsayılan "API Keys (Keychain)" yerine işlemi gösterir).
    pub title_override: Option<String>,
    pub error: Option<String>,
    // ── mouse hit rects (render'da doldurulur) ──
    pub list_rect: Rect,
    pub row_rects: Vec<Rect>,
}

impl KeysManagerState {
    /// Yeni manager. `locked` (keychain yok/kilitli) ise `Unlock` modunda açılır.
    pub fn new(entries: Vec<KeyEntry>, locked: bool) -> Self {
        let mut state = Self {
            window: ModalWindowState::new(),
            entries,
            categories: Vec::new(),
            default_category: String::new(),
            category_filter: None,
            category_rows: Vec::new(),
            selected: 0,
            scroll_offset: 0,
            mode: if locked {
                KeysManagerMode::Unlock { error: None }
            } else {
                KeysManagerMode::Browse
            },
            keychain_path: None,
            export_dir: None,
            opencode_auth_path: None,
            auto_activate_opencode: false,
            opencode_preferred_models: Vec::new(),
            opencode_provider_models: Vec::new(),
            opencode_provider_ids: Vec::new(),
            notice: None,
            master_editor: LineEditor::default(),
            show_master: false,
            path_editor: LineEditor::default(),
            provider_editor: LineEditor::default(),
            key_editor: LineEditor::default(),
            model_editor: LineEditor::default(),
            base_url_editor: LineEditor::default(),
            form_field: 0,
            category_cursor: 0,
            scope_cursor: 0,
            stack_rows: Vec::new(),
            pending_stack_sync: None,
            title_override: None,
            error: None,
            list_rect: Rect::default(),
            row_rects: Vec::new(),
        };
        state.refresh_meta();
        state
    }

    /// Kategori + default meta bilgisini RAM'den tazeler.
    fn refresh_meta(&mut self) {
        let mut cats = self
            .entries
            .iter()
            .map(|e| e.category.clone())
            .collect::<Vec<_>>();
        cats.sort();
        cats.dedup();
        self.categories = cats;
    }

    pub fn selected_entry(&self) -> Option<KeyEntry> {
        self.visible_entries().get(self.selected).cloned()
    }

    /// Belirli bir filtreye uyan kayıtlar (kopya; render/gezinme ortak).
    pub fn visible_entries_for(&self, filter: &CategoryFilter) -> Vec<KeyEntry> {
        if *filter == CategoryFilter::All {
            return self.entries.clone();
        }
        if *filter == CategoryFilter::MultiKey {
            // ≥ 3 keyi olan sağlayıcıların tüm kayıtları.
            let mut counts: std::collections::HashMap<&str, usize> =
                std::collections::HashMap::new();
            for e in &self.entries {
                *counts.entry(e.provider_id.as_str()).or_insert(0) += 1;
            }
            return self
                .entries
                .iter()
                .filter(|e| counts.get(e.provider_id.as_str()).is_some_and(|c| *c >= 3))
                .cloned()
                .collect();
        }
        self.entries
            .iter()
            .filter(|e| filter.matches(e))
            .cloned()
            .collect()
    }

    /// Aktif kategori filtresine uyan kayıtlar (kopya; render/gezinme ortak).
    pub fn visible_entries(&self) -> Vec<KeyEntry> {
        let Some(filter) = self.category_filter.as_ref() else {
            return self.entries.clone();
        };
        self.visible_entries_for(filter)
    }

    /// Dispatch: keychain'den çekilen güncel kayıtları uygular.
    pub fn apply_entries(&mut self, entries: Vec<KeyEntry>, default_category: String) {
        self.entries = entries;
        self.default_category = default_category;
        self.refresh_meta();
        if self.entries.is_empty() {
            self.selected = 0;
            self.scroll_offset = 0;
        } else if self.selected >= self.entries.len() {
            self.selected = self.entries.len() - 1;
        }
    }

    /// Dispatch: başarılı unlock → browse + satırlar taze.
    pub fn apply_unlocked(&mut self) {
        self.mode = KeysManagerMode::Browse;
        self.master_editor.reset();
        self.show_master = false;
        self.error = None;
    }

    /// Dispatch: başarısız unlock → hata ile kilit ekranına dön.
    pub fn apply_unlock_failed(&mut self, msg: String) {
        self.master_editor.reset();
        self.show_master = false;
        self.mode = KeysManagerMode::Unlock { error: Some(msg) };
    }

    /// Dispatch: reveal sonucu.
    pub fn apply_reveal(&mut self, id: String, full_key: Zeroizing<String>) {
        self.mode = KeysManagerMode::Reveal { id, full_key };
        self.error = None;
    }

    /// Dispatch: hata mesajı (browse'a dönüşle birlikte).
    pub fn apply_error_and_browse(&mut self, msg: String) {
        self.error = Some(msg);
        self.mode = KeysManagerMode::Browse;
        self.master_editor.reset();
        self.show_master = false;
    }

    fn move_selection(&mut self, delta: isize) {
        if self.visible_entries().is_empty() {
            return;
        }
        let len = self.visible_entries().len() as isize;
        let next = (self.selected as isize + delta).rem_euclid(len) as usize;
        self.selected = next;
        self.error = None;
    }
}

// ---------------------------------------------------------------------------
// Çıktı
// ---------------------------------------------------------------------------

/// Modal girdisi çıktısı: modal katmanı (`app/modals.rs`) bunları
/// `InputOutcome`'a çevirir; `Action` varyantları AppView dispatch'ine gider
/// (keychain yalnızca dispatch katmanında dokunulur — invariant).
#[derive(Debug)]
pub enum KeysManagerOutcome {
    /// Modal kapanmalı (browse'ta Esc / close butonu).
    Close,
    /// Durum değişti (yeniden çizim gerekli).
    Changed,
    /// Tuş/olay tüketilmedi.
    Unchanged,
    /// Keychain işlemi: dispatch'e giden aksiyon (sonuç dispatch tarafından
    /// modal state'ine geri yazılır).
    Action(Action),
}

// ---------------------------------------------------------------------------
// Girdi (pure-state)
// ---------------------------------------------------------------------------

/// Modal girdisi (pure-state): tuş + paste. Keychain işlemleri
/// [`KeysManagerOutcome::Action`] olarak dispatch'e gider; dispatch sonucu
/// state'e geri yazar.
pub fn handle_keys_manager_event(state: &mut KeysManagerState, ev: &Event) -> KeysManagerOutcome {
    match ev {
        Event::Key(key) if key.kind == KeyEventKind::Press => handle_keys_manager_key(state, key),
        Event::Paste(text) => {
            if matches!(&state.mode, KeysManagerMode::Unlock { .. })
                || matches!(
                    &state.mode,
                    KeysManagerMode::ExportPassword { .. } | KeysManagerMode::ImportPassword { .. }
                )
            {
                let _ = state.master_editor.insert_paste(text);
            } else if matches!(&state.mode, KeysManagerMode::ImportPath) {
                let _ = state.path_editor.insert_paste(text);
            } else if matches!(&state.mode, KeysManagerMode::Add) {
                match state.form_field {
                    0 => {
                        let _ = state.provider_editor.insert_paste(text);
                    }
                    1 => {
                        let _ = state.key_editor.insert_paste(text);
                    }
                    2 => {
                        let _ = state.model_editor.insert_paste(text);
                    }
                    _ => {
                        let _ = state.base_url_editor.insert_paste(text);
                    }
                }
            } else if matches!(&state.mode, KeysManagerMode::Edit { .. }) {
                match state.form_field {
                    0 => {
                        let _ = state.model_editor.insert_paste(text);
                    }
                    1 => {
                        let _ = state.base_url_editor.insert_paste(text);
                    }
                    _ => {
                        let _ = state.key_editor.insert_paste(text);
                    }
                }
            } else {
                return KeysManagerOutcome::Unchanged;
            }
            KeysManagerOutcome::Changed
        }
        _ => KeysManagerOutcome::Unchanged,
    }
}

fn handle_keys_manager_key(state: &mut KeysManagerState, key: &KeyEvent) -> KeysManagerOutcome {
    // Reveal: yalnızca Esc — `full_key` asla klonlanmaz (borrow kısa tutulur).
    if matches!(&state.mode, KeysManagerMode::Reveal { .. }) {
        return match key.code {
            KeyCode::Esc => {
                state.mode = KeysManagerMode::Browse;
                state.error = None;
                KeysManagerOutcome::Changed
            }
            _ => KeysManagerOutcome::Unchanged,
        };
    }
    // Arm'lar `state`'i mutate ederken scrutinee borrow'u canlı kalmasın diye
    // modu klonlarız (yalnızca küçük değerler; Reveal yukarıda ayrık).
    let mode = state.mode.clone();
    match mode {
        KeysManagerMode::Unlock { .. } => handle_unlock(state, key),
        // Browse'ta Esc doğrudan modalı kapatır (chrome'a düşmez — bu
        // katman tüm tuşları sahiplenir).
        KeysManagerMode::Browse => match key.code {
            KeyCode::Esc => KeysManagerOutcome::Close,
            _ => handle_browse(state, key),
        },
        KeysManagerMode::Reveal { .. } => unreachable!("handled above"),
        KeysManagerMode::ConfirmRemove { id } => match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                KeysManagerOutcome::Action(Action::KeychainRemove { id })
            }
            _ => {
                state.mode = KeysManagerMode::Browse;
                KeysManagerOutcome::Changed
            }
        },
        KeysManagerMode::ConfirmRemoveCategory { name } => match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                KeysManagerOutcome::Action(Action::KeychainRemoveCategory { name })
            }
            _ => {
                state.mode = KeysManagerMode::Browse;
                KeysManagerOutcome::Changed
            }
        },
        KeysManagerMode::Add => handle_add_form(state, key),
        KeysManagerMode::Edit { id } => handle_edit_form(state, key, id),
        KeysManagerMode::Categories => handle_categories(state, key),
        KeysManagerMode::ExportScope => handle_export_scope(state, key),
        KeysManagerMode::ExportPassword { scope } => handle_export_password(state, key, scope),
        KeysManagerMode::ExportDone { .. } => match key.code {
            KeyCode::Esc | KeyCode::Enter => {
                state.mode = KeysManagerMode::Browse;
                KeysManagerOutcome::Changed
            }
            _ => KeysManagerOutcome::Unchanged,
        },
        KeysManagerMode::ImportPath => match key.code {
            KeyCode::Esc => {
                state.mode = KeysManagerMode::Browse;
                state.error = None;
                KeysManagerOutcome::Changed
            }
            KeyCode::Enter => {
                let path = state.path_editor.text().trim().to_string();
                if path.is_empty() {
                    state.error = Some("dosya yolu boş olamaz".to_string());
                    return KeysManagerOutcome::Changed;
                }
                state.error = None;
                state.mode = KeysManagerMode::ImportPassword { path };
                state.master_editor.reset();
                state.show_master = false;
                KeysManagerOutcome::Changed
            }
            _ => {
                state.path_editor.handle_key(key);
                KeysManagerOutcome::Changed
            }
        },
        KeysManagerMode::ImportPassword { path } => handle_import_password(state, key, &path),
        KeysManagerMode::ImportDone { .. } => match key.code {
            KeyCode::Esc | KeyCode::Enter => {
                state.mode = KeysManagerMode::Browse;
                KeysManagerOutcome::Changed
            }
            _ => KeysManagerOutcome::Unchanged,
        },
        KeysManagerMode::StackPick => handle_stack_pick(state, key),
        KeysManagerMode::StackDirection {
            stack_id,
            stack_label,
        } => handle_stack_direction(state, key, stack_id, stack_label),
        KeysManagerMode::StackConfirm {
            stack_id,
            into_omnitrix,
            ..
        } => match key.code {
            KeyCode::Esc => {
                state.mode = KeysManagerMode::StackPick;
                KeysManagerOutcome::Changed
            }
            KeyCode::Enter => KeysManagerOutcome::Action(Action::KeychainStackSync {
                stack_id,
                into_omnitrix,
            }),
            _ => KeysManagerOutcome::Unchanged,
        },
        KeysManagerMode::StackDone { .. } => match key.code {
            KeyCode::Esc | KeyCode::Enter => {
                state.mode = KeysManagerMode::Browse;
                KeysManagerOutcome::Changed
            }
            _ => KeysManagerOutcome::Unchanged,
        },
    }
}

/// Kilitli açılış: master password (maskeli) → `KeychainUnlock` aksiyonu.
fn handle_unlock(state: &mut KeysManagerState, key: &KeyEvent) -> KeysManagerOutcome {
    match key.code {
        KeyCode::Esc => KeysManagerOutcome::Close,
        KeyCode::Enter => {
            let password = state.master_editor.text().to_string();
            if password.is_empty() {
                state.mode = KeysManagerMode::Unlock {
                    error: Some("master password boş olamaz".to_string()),
                };
                return KeysManagerOutcome::Changed;
            }
            KeysManagerOutcome::Action(Action::KeychainUnlock {
                password: Zeroizing::new(password),
            })
        }
        KeyCode::Tab => {
            state.show_master = !state.show_master;
            KeysManagerOutcome::Changed
        }
        KeyCode::Char('t') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.show_master = !state.show_master;
            KeysManagerOutcome::Changed
        }
        _ => {
            state.master_editor.handle_key(key);
            KeysManagerOutcome::Changed
        }
    }
}

/// Browse: seçim + action tuşları (keychain işlemleri aksiyon olarak çıkar).
fn handle_browse(state: &mut KeysManagerState, key: &KeyEvent) -> KeysManagerOutcome {
    match key.code {
        KeyCode::Up | KeyCode::Char('k') => {
            state.move_selection(-1);
            KeysManagerOutcome::Changed
        }
        KeyCode::Down | KeyCode::Char('j') => {
            state.move_selection(1);
            KeysManagerOutcome::Changed
        }
        KeyCode::Char('r') => {
            let Some(entry) = state.selected_entry() else {
                return KeysManagerOutcome::Unchanged;
            };
            KeysManagerOutcome::Action(Action::KeychainReveal {
                id: entry.id.clone(),
            })
        }
        KeyCode::Char('a') => {
            state.mode = KeysManagerMode::Add;
            state.form_field = 0;
            state.error = None;
            for editor in [
                &mut state.provider_editor,
                &mut state.key_editor,
                &mut state.model_editor,
                &mut state.base_url_editor,
            ] {
                editor.reset();
            }
            KeysManagerOutcome::Changed
        }
        KeyCode::Char('e') => {
            let Some(entry) = state.selected_entry() else {
                return KeysManagerOutcome::Unchanged;
            };
            state.mode = KeysManagerMode::Edit {
                id: entry.id.clone(),
            };
            state.form_field = 0;
            state.error = None;
            state
                .model_editor
                .set_text(entry.model_id.clone().unwrap_or_default());
            state
                .base_url_editor
                .set_text(entry.base_url.clone().unwrap_or_default());
            state.key_editor.reset();
            KeysManagerOutcome::Changed
        }
        KeyCode::Char('x') => {
            let Some(entry) = state.selected_entry() else {
                return KeysManagerOutcome::Unchanged;
            };
            state.mode = KeysManagerMode::ConfirmRemove {
                id: entry.id.clone(),
            };
            state.error = None;
            KeysManagerOutcome::Changed
        }
        KeyCode::Char('X') => {
            let Some(entry) = state.selected_entry() else {
                return KeysManagerOutcome::Unchanged;
            };
            state.mode = KeysManagerMode::ConfirmRemoveCategory {
                name: entry.category.clone(),
            };
            state.error = None;
            KeysManagerOutcome::Changed
        }
        KeyCode::Char('c') => {
            enter_categories(state);
            state.mode = KeysManagerMode::Categories;
            state.error = None;
            KeysManagerOutcome::Changed
        }
        KeyCode::Char('E') => {
            state.mode = KeysManagerMode::ExportScope;
            state.scope_cursor = 0;
            state.error = None;
            KeysManagerOutcome::Changed
        }
        KeyCode::Char('I') => {
            state.mode = KeysManagerMode::ImportPath;
            state.path_editor.reset();
            state.error = None;
            KeysManagerOutcome::Changed
        }
        KeyCode::Char('S') => {
            enter_stack_pick(state);
            state.mode = KeysManagerMode::StackPick;
            state.scope_cursor = 0;
            state.error = None;
            KeysManagerOutcome::Changed
        }
        _ => KeysManagerOutcome::Unchanged,
    }
}

/// Add formu: 4 alan (provider, key, model, base_url). Kategori yok —
/// sistem sağlayıcıya göre otomatik belirler.
/// Tab/Up/Down alan değiştirir; Enter `KeychainAdd` üretir; ctrl+t key'i
/// gösterir.
fn handle_add_form(state: &mut KeysManagerState, key: &KeyEvent) -> KeysManagerOutcome {
    match key.code {
        KeyCode::Esc => {
            state.mode = KeysManagerMode::Browse;
            state.error = None;
            KeysManagerOutcome::Changed
        }
        KeyCode::Tab | KeyCode::Up | KeyCode::Down => {
            state.form_field = (state.form_field + 1) % 4;
            KeysManagerOutcome::Changed
        }
        KeyCode::Char('t') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.show_master = !state.show_master;
            KeysManagerOutcome::Changed
        }
        KeyCode::Enter => {
            let provider = state.provider_editor.text().trim().to_string();
            let api_key = state.key_editor.text().to_string();
            if provider.is_empty() {
                state.error = Some("provider adı boş olamaz".to_string());
                return KeysManagerOutcome::Changed;
            }
            if api_key.is_empty() {
                state.error = Some("api key boş olamaz".to_string());
                return KeysManagerOutcome::Changed;
            }
            let model_id = {
                let m = state.model_editor.text().trim().to_string();
                if m.is_empty() { None } else { Some(m) }
            };
            let base_url = {
                let b = state.base_url_editor.text().trim().to_string();
                if b.is_empty() { None } else { Some(b) }
            };
            KeysManagerOutcome::Action(Action::KeychainAdd {
                provider_id: provider,
                api_key: Zeroizing::new(api_key),
                model_id,
                base_url,
            })
        }
        _ => {
            let editor = match state.form_field {
                0 => &mut state.provider_editor,
                1 => &mut state.key_editor,
                2 => &mut state.model_editor,
                _ => &mut state.base_url_editor,
            };
            editor.handle_key(key);
            state.error = None;
            KeysManagerOutcome::Changed
        }
    }
}

/// Edit formu: 3 alan (model, base_url, key opsiyonel) → `KeychainUpdate`.
fn handle_edit_form(
    state: &mut KeysManagerState,
    key: &KeyEvent,
    id: String,
) -> KeysManagerOutcome {
    match key.code {
        KeyCode::Esc => {
            state.mode = KeysManagerMode::Browse;
            state.error = None;
            KeysManagerOutcome::Changed
        }
        KeyCode::Tab | KeyCode::Up | KeyCode::Down => {
            state.form_field = (state.form_field + 1) % 3;
            KeysManagerOutcome::Changed
        }
        KeyCode::Char('t') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.show_master = !state.show_master;
            KeysManagerOutcome::Changed
        }
        KeyCode::Enter => {
            let model_id = {
                let m = state.model_editor.text().trim().to_string();
                if m.is_empty() { None } else { Some(m) }
            };
            let base_url = {
                let b = state.base_url_editor.text().trim().to_string();
                if b.is_empty() { None } else { Some(b) }
            };
            let api_key = {
                let k = state.key_editor.text().to_string();
                if k.is_empty() {
                    None
                } else {
                    Some(Zeroizing::new(k))
                }
            };
            KeysManagerOutcome::Action(Action::KeychainUpdate {
                id,
                model_id,
                base_url,
                api_key,
            })
        }
        _ => {
            let editor = match state.form_field {
                0 => &mut state.model_editor,
                1 => &mut state.base_url_editor,
                _ => &mut state.key_editor,
            };
            editor.handle_key(key);
            state.error = None;
            KeysManagerOutcome::Changed
        }
    }
}

fn enter_stack_pick(state: &mut KeysManagerState) {
    let mut rows = Vec::new();
    for p in xai_omni_keychain::detect_stacks() {
        let path = p
            .path
            .as_ref()
            .map(|x| x.display().to_string())
            .unwrap_or_else(|| "-".into());
        rows.push((p.def.id.to_string(), p.def.label.to_string(), path));
    }
    if rows.is_empty() {
        for def in xai_omni_keychain::all_stack_defs() {
            if def.id == "json-scan" || def.id == "env" {
                continue;
            }
            rows.push((
                def.id.to_string(),
                def.label.to_string(),
                "(path yok)".into(),
            ));
        }
    }
    state.stack_rows = rows;
    state.scope_cursor = 0;
}

fn handle_stack_pick(state: &mut KeysManagerState, key: &KeyEvent) -> KeysManagerOutcome {
    match key.code {
        KeyCode::Esc => {
            state.mode = KeysManagerMode::Browse;
            KeysManagerOutcome::Changed
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if state.stack_rows.is_empty() {
                return KeysManagerOutcome::Unchanged;
            }
            let len = state.stack_rows.len();
            state.scope_cursor = (state.scope_cursor + len - 1) % len;
            KeysManagerOutcome::Changed
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if state.stack_rows.is_empty() {
                return KeysManagerOutcome::Unchanged;
            }
            let len = state.stack_rows.len();
            state.scope_cursor = (state.scope_cursor + 1) % len;
            KeysManagerOutcome::Changed
        }
        KeyCode::Enter => {
            let Some((id, label, _)) = state.stack_rows.get(state.scope_cursor).cloned() else {
                return KeysManagerOutcome::Unchanged;
            };
            state.mode = KeysManagerMode::StackDirection {
                stack_id: id,
                stack_label: label,
            };
            state.scope_cursor = 0;
            state.error = None;
            KeysManagerOutcome::Changed
        }
        _ => KeysManagerOutcome::Unchanged,
    }
}

fn handle_stack_direction(
    state: &mut KeysManagerState,
    key: &KeyEvent,
    stack_id: String,
    stack_label: String,
) -> KeysManagerOutcome {
    match key.code {
        KeyCode::Esc => {
            state.mode = KeysManagerMode::StackPick;
            KeysManagerOutcome::Changed
        }
        KeyCode::Up | KeyCode::Down | KeyCode::Char('j') | KeyCode::Char('k') => {
            state.scope_cursor = 1 - state.scope_cursor.min(1);
            KeysManagerOutcome::Changed
        }
        KeyCode::Enter => {
            let into = state.scope_cursor == 0;
            let dir = if into {
                "stack → omnitrix (import)"
            } else {
                "omnitrix → stack (export)"
            };
            state.mode = KeysManagerMode::StackConfirm {
                stack_id: stack_id.clone(),
                stack_label: stack_label.clone(),
                into_omnitrix: into,
                lines: vec![
                    format!("{stack_label}  ·  {dir}"),
                    "Enter: çalıştır (merge, conflict skip)".into(),
                    "Esc: geri".into(),
                ],
                conflict_count: 0,
            };
            KeysManagerOutcome::Action(Action::KeychainStackPreview {
                stack_id,
                into_omnitrix: into,
            })
        }
        _ => KeysManagerOutcome::Unchanged,
    }
}

/// Kategori ekranına girerken türetilmiş satırları kurar: genel gruplar
/// (yüksek bakiyeli, çoklu anahtar, key tipleri) + sağlayıcı kategorileri.
fn enter_categories(state: &mut KeysManagerState) {
    use std::collections::BTreeMap;
    let mut rows = vec![CategoryFilter::All];
    let mut prov_counts: BTreeMap<&str, usize> = BTreeMap::new();
    let mut types: Vec<xai_omni_keychain::KeyType> = Vec::new();
    for e in &state.entries {
        *prov_counts.entry(e.provider_id.as_str()).or_insert(0) += 1;
        if e.key_type != xai_omni_keychain::KeyType::Unknown && !types.contains(&e.key_type) {
            types.push(e.key_type.clone());
        }
    }
    if state
        .entries
        .iter()
        .any(|e| e.balance.is_some_and(|b| b >= 10.0))
    {
        rows.push(CategoryFilter::HighBalance);
    }
    if prov_counts.values().any(|c| *c >= 3) {
        rows.push(CategoryFilter::MultiKey);
    }
    for t in types {
        rows.push(CategoryFilter::KeyType(t));
    }
    for (provider, _count) in prov_counts {
        rows.push(CategoryFilter::Provider(provider.to_string()));
    }
    state.category_rows = rows;
    state.category_cursor = 0;
}

/// Kategori listesi: Enter → Browse'ı o filtreyle açar. Kategori "ayarı"
/// değil, türetilmiş görünüm filtresidir.
fn handle_categories(state: &mut KeysManagerState, key: &KeyEvent) -> KeysManagerOutcome {
    match key.code {
        KeyCode::Esc => {
            state.mode = KeysManagerMode::Browse;
            KeysManagerOutcome::Changed
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if state.category_rows.is_empty() {
                return KeysManagerOutcome::Unchanged;
            }
            state.category_cursor =
                (state.category_cursor + state.category_rows.len() - 1) % state.category_rows.len();
            KeysManagerOutcome::Changed
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if state.category_rows.is_empty() {
                return KeysManagerOutcome::Unchanged;
            }
            state.category_cursor = (state.category_cursor + 1) % state.category_rows.len();
            KeysManagerOutcome::Changed
        }
        KeyCode::Enter => {
            let Some(filter) = state.category_rows.get(state.category_cursor).cloned() else {
                return KeysManagerOutcome::Unchanged;
            };
            state.category_filter = Some(filter);
            state.selected = 0;
            state.scroll_offset = 0;
            state.mode = KeysManagerMode::Browse;
            state.error = None;
            KeysManagerOutcome::Changed
        }
        _ => KeysManagerOutcome::Unchanged,
    }
}

/// Export kapsamı seçimi: 0 = All, 1.. = kategori.
fn handle_export_scope(state: &mut KeysManagerState, key: &KeyEvent) -> KeysManagerOutcome {
    match key.code {
        KeyCode::Esc => {
            state.mode = KeysManagerMode::Browse;
            KeysManagerOutcome::Changed
        }
        KeyCode::Up | KeyCode::Char('k') => {
            let len = state.categories.len() + 1;
            state.scope_cursor = (state.scope_cursor + len - 1) % len;
            KeysManagerOutcome::Changed
        }
        KeyCode::Down | KeyCode::Char('j') => {
            let len = state.categories.len() + 1;
            state.scope_cursor = (state.scope_cursor + 1) % len;
            KeysManagerOutcome::Changed
        }
        KeyCode::Enter => {
            let scope = if state.scope_cursor == 0 {
                ExportScope::All
            } else {
                let name = state
                    .categories
                    .get(state.scope_cursor - 1)
                    .cloned()
                    .unwrap_or_default();
                ExportScope::Categories(vec![name])
            };
            state.mode = KeysManagerMode::ExportPassword { scope };
            state.master_editor.reset();
            state.show_master = false;
            state.error = None;
            KeysManagerOutcome::Changed
        }
        _ => KeysManagerOutcome::Unchanged,
    }
}

/// Export şifresi (maskeli) → `KeychainExport`.
fn handle_export_password(
    state: &mut KeysManagerState,
    key: &KeyEvent,
    scope: ExportScope,
) -> KeysManagerOutcome {
    match key.code {
        KeyCode::Esc => {
            state.mode = KeysManagerMode::ExportScope;
            KeysManagerOutcome::Changed
        }
        KeyCode::Tab => {
            state.show_master = !state.show_master;
            KeysManagerOutcome::Changed
        }
        KeyCode::Char('t') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.show_master = !state.show_master;
            KeysManagerOutcome::Changed
        }
        KeyCode::Enter => {
            let password = state.master_editor.text().to_string();
            if password.is_empty() {
                state.error = Some("export şifresi boş olamaz".to_string());
                return KeysManagerOutcome::Changed;
            }
            KeysManagerOutcome::Action(Action::KeychainExport {
                scope,
                password: Zeroizing::new(password),
            })
        }
        _ => {
            state.master_editor.handle_key(key);
            KeysManagerOutcome::Changed
        }
    }
}

/// Import şifresi → `KeychainImport`.
fn handle_import_password(
    state: &mut KeysManagerState,
    key: &KeyEvent,
    path: &str,
) -> KeysManagerOutcome {
    match key.code {
        KeyCode::Esc => {
            state.mode = KeysManagerMode::ImportPath;
            KeysManagerOutcome::Changed
        }
        KeyCode::Tab => {
            state.show_master = !state.show_master;
            KeysManagerOutcome::Changed
        }
        KeyCode::Char('t') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.show_master = !state.show_master;
            KeysManagerOutcome::Changed
        }
        KeyCode::Enter => {
            let password = state.master_editor.text().to_string();
            if password.is_empty() {
                state.error = Some("import şifresi boş olamaz".to_string());
                return KeysManagerOutcome::Changed;
            }
            KeysManagerOutcome::Action(Action::KeychainImport {
                path: path.to_string(),
                password: Zeroizing::new(password),
            })
        }
        _ => {
            state.master_editor.handle_key(key);
            KeysManagerOutcome::Changed
        }
    }
}

// ---------------------------------------------------------------------------
// Mouse (minimal): satır tıklama = seçim.
// ---------------------------------------------------------------------------

const SHORTCUT_ENTER: usize = 1;
const SHORTCUT_ESCAPE: usize = 2;
const SHORTCUT_TAB: usize = 3;
const SHORTCUT_TOGGLE_SHOW: usize = 4;
const SHORTCUT_CONFIRM: usize = 5;
const SHORTCUT_UP: usize = 6;
const SHORTCUT_DOWN: usize = 7;
const SHORTCUT_REVEAL: usize = 10;
const SHORTCUT_ADD: usize = 11;
const SHORTCUT_EDIT: usize = 12;
const SHORTCUT_REMOVE: usize = 13;
const SHORTCUT_REMOVE_CATEGORY: usize = 14;
const SHORTCUT_CATEGORIES: usize = 15;
const SHORTCUT_EXPORT: usize = 16;
const SHORTCUT_IMPORT: usize = 17;
const SHORTCUT_STACK: usize = 18;

/// Translate a footer click into the exact key event used by keyboard input.
/// This keeps mouse and keyboard semantics on one state-machine path.
pub fn handle_keys_manager_shortcut(state: &mut KeysManagerState, id: usize) -> KeysManagerOutcome {
    let (code, modifiers) = match id {
        SHORTCUT_ENTER => (KeyCode::Enter, KeyModifiers::NONE),
        SHORTCUT_ESCAPE => (KeyCode::Esc, KeyModifiers::NONE),
        SHORTCUT_TAB => (KeyCode::Tab, KeyModifiers::NONE),
        SHORTCUT_TOGGLE_SHOW => (KeyCode::Char('t'), KeyModifiers::CONTROL),
        SHORTCUT_CONFIRM => (KeyCode::Char('y'), KeyModifiers::NONE),
        SHORTCUT_UP => (KeyCode::Up, KeyModifiers::NONE),
        SHORTCUT_DOWN => (KeyCode::Down, KeyModifiers::NONE),
        SHORTCUT_REVEAL => (KeyCode::Char('r'), KeyModifiers::NONE),
        SHORTCUT_ADD => (KeyCode::Char('a'), KeyModifiers::NONE),
        SHORTCUT_EDIT => (KeyCode::Char('e'), KeyModifiers::NONE),
        SHORTCUT_REMOVE => (KeyCode::Char('x'), KeyModifiers::NONE),
        SHORTCUT_REMOVE_CATEGORY => (KeyCode::Char('X'), KeyModifiers::SHIFT),
        SHORTCUT_CATEGORIES => (KeyCode::Char('c'), KeyModifiers::NONE),
        SHORTCUT_EXPORT => (KeyCode::Char('E'), KeyModifiers::SHIFT),
        SHORTCUT_IMPORT => (KeyCode::Char('I'), KeyModifiers::SHIFT),
        SHORTCUT_STACK => (KeyCode::Char('S'), KeyModifiers::SHIFT),
        _ => return KeysManagerOutcome::Unchanged,
    };
    handle_keys_manager_key(state, &KeyEvent::new(code, modifiers))
}

pub fn handle_keys_manager_mouse(
    state: &mut KeysManagerState,
    kind: MouseEventKind,
    column: u16,
    row: u16,
) -> KeysManagerOutcome {
    if state.mode != KeysManagerMode::Browse {
        return KeysManagerOutcome::Unchanged;
    }
    if !matches!(
        kind,
        MouseEventKind::Down(crossterm::event::MouseButton::Left)
    ) {
        return KeysManagerOutcome::Unchanged;
    }
    for (i, rect) in state.row_rects.iter().enumerate() {
        if column >= rect.x
            && column < rect.x + rect.width
            && row >= rect.y
            && row < rect.y + rect.height
        {
            // `row_rects` yalnızca scroll_offset'ten sonraki satırları içerir;
            // tıklanan görsel satırın gerçek girdi indeksi offset kaydırılır.
            state.selected = state.scroll_offset + i;
            state.error = None;
            return KeysManagerOutcome::Changed;
        }
    }
    KeysManagerOutcome::Unchanged
}

// ---------------------------------------------------------------------------
// Render
// ---------------------------------------------------------------------------

fn centered_vertical_margin(area_height: u16, desired_height: u16) -> u16 {
    let spare = area_height.saturating_sub(desired_height);
    spare.saturating_add(1).saturating_div(2)
}

/// Keep short key lists and small forms compact while allowing long lists to
/// grow to a useful (but bounded) viewport.  `ModalSizing` derives height from
/// the vertical margin, so this is the single source of truth for every mode.
fn keys_modal_sizing(area: Rect, state: &KeysManagerState, compact: bool) -> ModalSizing {
    let (desired_height, footer_lines) = match &state.mode {
        KeysManagerMode::Browse => {
            let visible_rows = state.visible_entries().len().clamp(1, 10) as u16;
            (visible_rows + 15, 3)
        }
        KeysManagerMode::Unlock { .. } => (12, 2),
        KeysManagerMode::Reveal { .. } => (14, 2),
        KeysManagerMode::ConfirmRemove { .. } | KeysManagerMode::ConfirmRemoveCategory { .. } => {
            (10, 2)
        }
        KeysManagerMode::Add | KeysManagerMode::Edit { .. } => (17, 2),
        KeysManagerMode::Categories => {
            let rows = state.category_rows.len().clamp(1, 10) as u16;
            (rows + 11, 2)
        }
        KeysManagerMode::ExportScope => {
            let rows = (state.categories.len() + 1).clamp(1, 10) as u16;
            (rows + 11, 2)
        }
        KeysManagerMode::StackPick => {
            let rows = state.stack_rows.len().clamp(1, 10) as u16;
            (rows + 11, 2)
        }
        KeysManagerMode::StackConfirm { lines, .. } => {
            let rows = lines.len().clamp(1, 10) as u16;
            (rows + 12, 2)
        }
        KeysManagerMode::ExportPassword { .. }
        | KeysManagerMode::ImportPassword { .. }
        | KeysManagerMode::ImportPath
        | KeysManagerMode::StackDirection { .. } => (13, 2),
        KeysManagerMode::ExportDone { .. }
        | KeysManagerMode::ImportDone { .. }
        | KeysManagerMode::StackDone { .. } => (12, 2),
    };

    ModalSizing {
        width_pct: 0.70,
        max_width: 100,
        min_width: 60,
        v_margin: centered_vertical_margin(area.height, desired_height),
        h_pad: 2,
        v_pad: 1,
        footer_lines,
    }
    .with_compact(compact)
}

pub fn render_keys_manager(
    buf: &mut Buffer,
    area: Rect,
    state: &mut KeysManagerState,
    compact: bool,
) {
    let shortcuts: Vec<Shortcut<'_>> = match &state.mode {
        KeysManagerMode::Unlock { .. } => vec![
            Shortcut {
                label: "Enter unlock",
                clickable: true,
                id: SHORTCUT_ENTER,
            },
            Shortcut {
                label: "ctrl+t show",
                clickable: true,
                id: SHORTCUT_TOGGLE_SHOW,
            },
            Shortcut {
                label: "Esc close",
                clickable: true,
                id: SHORTCUT_ESCAPE,
            },
        ],
        KeysManagerMode::Browse => vec![
            Shortcut {
                label: "\u{2191} up",
                clickable: true,
                id: SHORTCUT_UP,
            },
            Shortcut {
                label: "\u{2193} down",
                clickable: true,
                id: SHORTCUT_DOWN,
            },
            Shortcut {
                label: "r reveal",
                clickable: true,
                id: SHORTCUT_REVEAL,
            },
            Shortcut {
                label: "a add",
                clickable: true,
                id: SHORTCUT_ADD,
            },
            Shortcut {
                label: "e edit",
                clickable: true,
                id: SHORTCUT_EDIT,
            },
            Shortcut {
                label: "x remove",
                clickable: true,
                id: SHORTCUT_REMOVE,
            },
            Shortcut {
                label: "X remove group",
                clickable: true,
                id: SHORTCUT_REMOVE_CATEGORY,
            },
            Shortcut {
                label: "c categories",
                clickable: true,
                id: SHORTCUT_CATEGORIES,
            },
            Shortcut {
                label: "E export",
                clickable: true,
                id: SHORTCUT_EXPORT,
            },
            Shortcut {
                label: "I import",
                clickable: true,
                id: SHORTCUT_IMPORT,
            },
            Shortcut {
                label: "S stack",
                clickable: true,
                id: SHORTCUT_STACK,
            },
        ],
        KeysManagerMode::Reveal { .. } => vec![Shortcut {
            label: "Esc hide",
            clickable: true,
            id: SHORTCUT_ESCAPE,
        }],
        KeysManagerMode::ConfirmRemove { .. } | KeysManagerMode::ConfirmRemoveCategory { .. } => {
            vec![Shortcut {
                label: "y confirm",
                clickable: true,
                id: SHORTCUT_CONFIRM,
            }]
        }
        KeysManagerMode::Add { .. } => vec![
            Shortcut {
                label: "Tab field",
                clickable: true,
                id: SHORTCUT_TAB,
            },
            Shortcut {
                label: "Enter save",
                clickable: true,
                id: SHORTCUT_ENTER,
            },
            Shortcut {
                label: "Esc cancel",
                clickable: true,
                id: SHORTCUT_ESCAPE,
            },
        ],
        KeysManagerMode::Edit { .. } => vec![
            Shortcut {
                label: "Tab field",
                clickable: true,
                id: SHORTCUT_TAB,
            },
            Shortcut {
                label: "Enter save",
                clickable: true,
                id: SHORTCUT_ENTER,
            },
            Shortcut {
                label: "Esc cancel",
                clickable: true,
                id: SHORTCUT_ESCAPE,
            },
        ],
        KeysManagerMode::Categories => vec![
            Shortcut {
                label: "\u{2191} up",
                clickable: true,
                id: SHORTCUT_UP,
            },
            Shortcut {
                label: "\u{2193} down",
                clickable: true,
                id: SHORTCUT_DOWN,
            },
            Shortcut {
                label: "Enter set default",
                clickable: true,
                id: SHORTCUT_ENTER,
            },
            Shortcut {
                label: "Esc back",
                clickable: true,
                id: SHORTCUT_ESCAPE,
            },
        ],
        KeysManagerMode::ExportScope => vec![
            Shortcut {
                label: "\u{2191} up",
                clickable: true,
                id: SHORTCUT_UP,
            },
            Shortcut {
                label: "\u{2193} down",
                clickable: true,
                id: SHORTCUT_DOWN,
            },
            Shortcut {
                label: "Enter select scope",
                clickable: true,
                id: SHORTCUT_ENTER,
            },
            Shortcut {
                label: "Esc back",
                clickable: true,
                id: SHORTCUT_ESCAPE,
            },
        ],
        KeysManagerMode::ExportPassword { .. } | KeysManagerMode::ImportPassword { .. } => vec![
            Shortcut {
                label: "Enter submit",
                clickable: true,
                id: SHORTCUT_ENTER,
            },
            Shortcut {
                label: "ctrl+t show",
                clickable: true,
                id: SHORTCUT_TOGGLE_SHOW,
            },
            Shortcut {
                label: "Esc back",
                clickable: true,
                id: SHORTCUT_ESCAPE,
            },
        ],
        KeysManagerMode::ExportDone { .. }
        | KeysManagerMode::ImportDone { .. }
        | KeysManagerMode::StackDone { .. } => vec![Shortcut {
            label: "Enter/Esc ok",
            clickable: true,
            id: SHORTCUT_ENTER,
        }],
        KeysManagerMode::ImportPath => vec![
            Shortcut {
                label: "Enter next",
                clickable: true,
                id: SHORTCUT_ENTER,
            },
            Shortcut {
                label: "Esc back",
                clickable: true,
                id: SHORTCUT_ESCAPE,
            },
        ],
        KeysManagerMode::StackPick
        | KeysManagerMode::StackDirection { .. }
        | KeysManagerMode::StackConfirm { .. } => vec![
            Shortcut {
                label: "\u{2191} up",
                clickable: true,
                id: SHORTCUT_UP,
            },
            Shortcut {
                label: "\u{2193} down",
                clickable: true,
                id: SHORTCUT_DOWN,
            },
            Shortcut {
                label: "Enter",
                clickable: true,
                id: SHORTCUT_ENTER,
            },
            Shortcut {
                label: "Esc back",
                clickable: true,
                id: SHORTCUT_ESCAPE,
            },
        ],
    };
    let config = ModalWindowConfig {
        title: state
            .title_override
            .as_deref()
            .unwrap_or("API Keys (Keychain)"),
        tabs: None,
        shortcuts: &shortcuts,
        sizing: keys_modal_sizing(area, state, compact),
        fold_info: None,
    };
    let theme = Theme::current();
    let Some(mca) = crate::views::modal_window::render_modal_window(
        buf,
        area,
        &mut state.window,
        &config,
        &theme,
    ) else {
        return;
    };
    // Arm'lar `state`'i `&mut` alırken scrutinee borrow'u canlı kalmasın
    // diye modu klonlarız (render_connect_flow deseni).
    let mode = state.mode.clone();
    match mode {
        KeysManagerMode::Unlock { error } => {
            render_unlock(
                buf,
                mca.content,
                mca.inner_x,
                mca.inner_width,
                state,
                &error,
                &theme,
            );
        }
        KeysManagerMode::Browse => {
            render_browse(
                buf,
                mca.content,
                mca.inner_x,
                mca.inner_width,
                state,
                &theme,
            );
        }
        KeysManagerMode::Reveal { id, full_key } => {
            render_reveal(
                buf,
                mca.content,
                mca.inner_x,
                mca.inner_width,
                state,
                &id,
                &full_key,
                &theme,
            );
        }
        KeysManagerMode::ConfirmRemove { .. } => {
            render_confirm(
                buf,
                mca.content,
                mca.inner_x,
                mca.inner_width,
                state,
                &theme,
                "kayıt silinsin mi? (y/n)",
            );
        }
        KeysManagerMode::ConfirmRemoveCategory { .. } => {
            render_confirm(
                buf,
                mca.content,
                mca.inner_x,
                mca.inner_width,
                state,
                &theme,
                "kategori (ve içindeki tüm key'ler) silinsin mi? (y/n)",
            );
        }
        KeysManagerMode::Add => {
            render_add_form(
                buf,
                mca.content,
                mca.inner_x,
                mca.inner_width,
                state,
                &theme,
            );
        }
        KeysManagerMode::Edit { .. } => {
            render_edit_form(
                buf,
                mca.content,
                mca.inner_x,
                mca.inner_width,
                state,
                &theme,
            );
        }
        KeysManagerMode::Categories => {
            render_categories(
                buf,
                mca.content,
                mca.inner_x,
                mca.inner_width,
                state,
                &theme,
            );
        }
        KeysManagerMode::ExportScope => {
            render_export_scope(
                buf,
                mca.content,
                mca.inner_x,
                mca.inner_width,
                state,
                &theme,
            );
        }
        KeysManagerMode::ExportPassword { .. } | KeysManagerMode::ImportPassword { .. } => {
            render_password_input(
                buf,
                mca.content,
                mca.inner_x,
                mca.inner_width,
                state,
                &theme,
            );
        }
        KeysManagerMode::ExportDone { path, count } => {
            render_done(
                buf,
                mca.content,
                mca.inner_x,
                mca.inner_width,
                state,
                &theme,
                &format!("export edildi: {path} ({count} key)"),
            );
        }
        KeysManagerMode::ImportDone { summary } => {
            render_done(
                buf,
                mca.content,
                mca.inner_x,
                mca.inner_width,
                state,
                &theme,
                &format_import_summary(&summary),
            );
        }
        KeysManagerMode::ImportPath => {
            render_path_input(
                buf,
                mca.content,
                mca.inner_x,
                mca.inner_width,
                state,
                &theme,
            );
        }
        KeysManagerMode::StackPick => {
            render_stack_pick(
                buf,
                mca.content,
                mca.inner_x,
                mca.inner_width,
                state,
                &theme,
            );
        }
        KeysManagerMode::StackDirection { stack_label, .. } => {
            render_stack_direction(
                buf,
                mca.content,
                mca.inner_x,
                mca.inner_width,
                state,
                &stack_label,
                &theme,
            );
        }
        KeysManagerMode::StackConfirm { lines, .. } => {
            render_stack_lines(
                buf,
                mca.content,
                mca.inner_x,
                mca.inner_width,
                state,
                &lines,
                &theme,
            );
        }
        KeysManagerMode::StackDone { message } => {
            render_done(
                buf,
                mca.content,
                mca.inner_x,
                mca.inner_width,
                state,
                &theme,
                &message,
            );
        }
    }
}

fn render_line(buf: &mut Buffer, x: u16, y: u16, width: u16, text: &str, style: Style) {
    if width == 0 {
        return;
    }
    buf.set_line(
        x,
        y,
        &Line::from(Span::styled(text.to_string(), style)),
        width,
    );
}

fn masked_render(text: &str) -> String {
    "\u{2022}".repeat(text.chars().count())
}

/// Maskeli giriş satırı: `label` + `•` + cursor (render_masked_editor modeli).
fn render_masked_input(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    width: u16,
    theme: &Theme,
    label: &str,
    editor: &LineEditor,
    reveal: bool,
) {
    let label_w = label.len() as u16;
    let input_width = width.saturating_sub(label_w) as usize;
    render_line(buf, x, y, width, label, Style::default().fg(theme.gray));
    let input_x = x + label_w;
    let viewport = editor.viewport(input_width);
    let text = editor.text();
    let displayed = if text.is_empty() {
        ""
    } else {
        &text[viewport.visible_byte_range]
    };
    if !displayed.is_empty() {
        let shown = if reveal {
            displayed.to_string()
        } else {
            masked_render(displayed)
        };
        buf.set_span(
            input_x,
            y,
            &Span::styled(&shown, Style::default().fg(theme.text_primary)),
            shown.width() as u16,
        );
    }
    let cursor_col = viewport
        .cursor_display_column
        .min(input_width.saturating_sub(1));
    let cursor_x = input_x + cursor_col as u16;
    if cursor_x < x + width
        && let Some(cell) = buf.cell_mut((cursor_x, y))
    {
        cell.set_style(Style::default().fg(theme.bg_base).bg(theme.text_primary));
    }
}

/// Son satıra hata mesajı (yoksa dokunmaz).
fn render_error_line(
    buf: &mut Buffer,
    inner_x: u16,
    inner_width: u16,
    state: &KeysManagerState,
    theme: &Theme,
    y: u16,
) {
    if let Some(err) = &state.error {
        render_line(
            buf,
            inner_x,
            y,
            inner_width,
            &format!("\u{2717} {err}"),
            Style::default().fg(theme.accent_error),
        );
    }
}

fn render_unlock(
    buf: &mut Buffer,
    content: Rect,
    inner_x: u16,
    inner_width: u16,
    state: &KeysManagerState,
    error: &Option<String>,
    theme: &Theme,
) {
    let mut y = content.y;
    render_line(
        buf,
        inner_x,
        y,
        inner_width,
        "Keychain kilitli \u{2014} master password ile açın",
        Style::default()
            .fg(theme.text_primary)
            .add_modifier(Modifier::BOLD),
    );
    y += 1;
    if let Some(e) = error {
        render_line(
            buf,
            inner_x,
            y,
            inner_width,
            &format!("\u{2717} {e}"),
            Style::default().fg(theme.accent_error),
        );
        y += 1;
    }
    if y + 1 < content.y + content.height {
        render_masked_input(
            buf,
            inner_x,
            y,
            inner_width,
            theme,
            " master password: ",
            &state.master_editor,
            state.show_master,
        );
        y += 1;
        render_line(
            buf,
            inner_x,
            y,
            inner_width,
            "Enter: aç · ctrl+t: göster/gizle · Esc: kapat",
            Style::default().fg(theme.gray_dim),
        );
    }
}

/// Tablo sütun genişlikleri (modal iç genişliğine sığar; keys_cmd formatıyla
/// uyumlu — ID sütunu modalda yok).
const COL_KATEGORI: usize = 12;
const COL_PROVIDER: usize = 14;
const COL_MASKELI: usize = 16;
const COL_MODEL: usize = 14;
const COL_LAST: usize = 12;

fn render_browse(
    buf: &mut Buffer,
    content: Rect,
    inner_x: u16,
    inner_width: u16,
    state: &mut KeysManagerState,
    theme: &Theme,
) {
    if content.height == 0 {
        return;
    }
    let mut y = content.y;
    let filter_label = state
        .category_filter
        .as_ref()
        .filter(|f| **f != CategoryFilter::All)
        .map(|f| format!(" \u{2014} {}", f.label()))
        .unwrap_or_default();
    let header = format!(
        "{}  {}  {}  {}  {}{}",
        crate::keys_cmd::pad("KATEGORI", COL_KATEGORI),
        crate::keys_cmd::pad("PROVIDER", COL_PROVIDER),
        crate::keys_cmd::pad("MASKELI", COL_MASKELI),
        crate::keys_cmd::pad("MODEL", COL_MODEL),
        crate::keys_cmd::pad("SON_KULLANIM", COL_LAST),
        filter_label,
    );
    render_line(
        buf,
        inner_x,
        y,
        inner_width,
        &header,
        Style::default()
            .fg(theme.gray_bright)
            .add_modifier(Modifier::BOLD),
    );
    y += 1;

    let action_bar_y = content.y + content.height.saturating_sub(1);
    let rows_area_end = action_bar_y;
    let list_height = rows_area_end.saturating_sub(y);

    let visible = state.visible_entries();

    // Seçimi görünür alana clamp et (scroll).
    if state.selected < state.scroll_offset {
        state.scroll_offset = state.selected;
    }
    if list_height > 0 && state.selected >= state.scroll_offset + list_height as usize {
        state.scroll_offset = state.selected - list_height as usize + 1;
    }

    state.list_rect = Rect::new(inner_x, y, inner_width, list_height);
    state.row_rects.clear();
    for (i, entry) in visible.iter().enumerate() {
        if i < state.scroll_offset {
            continue;
        }
        if y >= rows_area_end {
            break;
        }
        let selected = i == state.selected;
        let style = if selected {
            Style::default()
                .fg(theme.text_primary)
                .add_modifier(Modifier::REVERSED)
        } else {
            Style::default().fg(theme.text_secondary)
        };
        let category_cell = match entry.balance {
            Some(b) if b >= 1.0 => format!("{} \u{00b7} ${b:.1}", entry.category),
            _ => entry.category.clone(),
        };
        let row = format!(
            "{}  {}  {}  {}  {}",
            crate::keys_cmd::pad(&category_cell, COL_KATEGORI),
            crate::keys_cmd::pad(&entry.provider_id, COL_PROVIDER),
            crate::keys_cmd::pad(&entry.masked, COL_MASKELI),
            crate::keys_cmd::pad(entry.model_id.as_deref().unwrap_or("-"), COL_MODEL),
            crate::keys_cmd::pad(
                entry.last_used.as_deref().map(short_date).unwrap_or("-"),
                COL_LAST,
            ),
        );
        render_line(buf, inner_x, y, inner_width, &row, style);
        state.row_rects.push(Rect::new(inner_x, y, inner_width, 1));
        y += 1;
    }

    // Action çubuğu (altta); hata varsa onun yerine hata gösterilir.
    let action_bar = match &state.error {
        Some(err) => format!("\u{2717} {err}"),
        None if state.notice.is_some() => state.notice.clone().unwrap_or_default(),
        None => {
            "r reveal · a add · e edit · x remove · X kategori · c kategoriler · E export · I import · S stack"
                .to_string()
        }
    };
    render_line(
        buf,
        inner_x,
        action_bar_y,
        inner_width,
        &action_bar,
        Style::default().fg(theme.gray_dim),
    );
}

fn render_reveal(
    buf: &mut Buffer,
    content: Rect,
    inner_x: u16,
    inner_width: u16,
    state: &KeysManagerState,
    id: &str,
    full_key: &str,
    theme: &Theme,
) {
    let mut y = content.y;
    render_line(
        buf,
        inner_x,
        y,
        inner_width,
        &format!("Tam key \u{2014} {id}"),
        Style::default()
            .fg(theme.text_primary)
            .add_modifier(Modifier::BOLD),
    );
    y += 1;
    render_line(
        buf,
        inner_x,
        y,
        inner_width,
        "Bu key yalnızca RAM'de tutulur; kopyalamak için seçin.",
        Style::default().fg(theme.gray),
    );
    y += 1;
    if y < content.y + content.height {
        let lines = wrap_key(full_key, inner_width as usize);
        for line in lines {
            if y >= content.y + content.height {
                break;
            }
            render_line(
                buf,
                inner_x,
                y,
                inner_width,
                &line,
                Style::default().fg(theme.text_primary),
            );
            y += 1;
        }
    }
    if y < content.y + content.height {
        render_line(
            buf,
            inner_x,
            y,
            inner_width,
            "Esc: gizle",
            Style::default().fg(theme.gray_dim),
        );
    }
    render_error_line(
        buf,
        inner_x,
        inner_width,
        state,
        theme,
        content.y + content.height - 1,
    );
}

/// Uzun key'i satır genişliğine böler.
fn wrap_key(key: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![key.to_string()];
    }
    let mut out = Vec::new();
    let mut rest = key;
    while !rest.is_empty() {
        let mut take = 0;
        let mut w = 0;
        for ch in rest.chars() {
            let cw = ch.width().unwrap_or(0).max(1);
            if w + cw > width {
                break;
            }
            w += cw;
            take += ch.len_utf8();
        }
        if take == 0 {
            take = rest.len();
        }
        out.push(rest[..take].to_string());
        rest = &rest[take..];
    }
    out
}

fn render_confirm(
    buf: &mut Buffer,
    content: Rect,
    inner_x: u16,
    inner_width: u16,
    state: &KeysManagerState,
    theme: &Theme,
    prompt: &str,
) {
    let mut y = content.y;
    render_line(
        buf,
        inner_x,
        y,
        inner_width,
        "Onay gerekli",
        Style::default()
            .fg(theme.text_primary)
            .add_modifier(Modifier::BOLD),
    );
    y += 1;
    render_line(
        buf,
        inner_x,
        y,
        inner_width,
        prompt,
        Style::default().fg(theme.text_primary),
    );
    y += 1;
    render_line(
        buf,
        inner_x,
        y,
        inner_width,
        "y: evet · diğer tuş: vazgeç",
        Style::default().fg(theme.gray_dim),
    );
    render_error_line(
        buf,
        inner_x,
        inner_width,
        state,
        theme,
        content.y + content.height - 1,
    );
}

const ADD_FIELD_LABELS: [&str; 4] = ["provider", "api key", "model", "base url"];

fn render_add_form(
    buf: &mut Buffer,
    content: Rect,
    inner_x: u16,
    inner_width: u16,
    state: &KeysManagerState,
    theme: &Theme,
) {
    render_line(
        buf,
        inner_x,
        content.y,
        inner_width,
        "Yeni key (kategori otomatik: sağlayıcıya göre)",
        Style::default()
            .fg(theme.text_primary)
            .add_modifier(Modifier::BOLD),
    );
    let editors: [&LineEditor; 4] = [
        &state.provider_editor,
        &state.key_editor,
        &state.model_editor,
        &state.base_url_editor,
    ];
    let mut y = content.y + 2;
    for (i, (label, editor)) in ADD_FIELD_LABELS.iter().zip(editors.iter()).enumerate() {
        if y + 1 >= content.y + content.height {
            break;
        }
        let focused = i == state.form_field;
        let prefix = if focused { "\u{25b8} " } else { "  " };
        let reveal = i != 1 || state.show_master;
        render_masked_input(
            buf,
            inner_x,
            y,
            inner_width,
            theme,
            &format!("{prefix}{label}: "),
            editor,
            reveal,
        );
        y += 1;
    }
    if y < content.y + content.height {
        render_line(
            buf,
            inner_x,
            y,
            inner_width,
            "Enter: kaydet · Tab: alan değiştir · ctrl+t: key'i göster · Esc: vazgeç",
            Style::default().fg(theme.gray_dim),
        );
    }
    render_error_line(
        buf,
        inner_x,
        inner_width,
        state,
        theme,
        content.y + content.height - 1,
    );
}

const EDIT_FIELD_LABELS: [&str; 3] = ["model", "base url", "api key (opsiyonel)"];

fn render_edit_form(
    buf: &mut Buffer,
    content: Rect,
    inner_x: u16,
    inner_width: u16,
    state: &KeysManagerState,
    theme: &Theme,
) {
    render_line(
        buf,
        inner_x,
        content.y,
        inner_width,
        "Kaydı düzenle (boş alanlar değiştirilmez)",
        Style::default()
            .fg(theme.text_primary)
            .add_modifier(Modifier::BOLD),
    );
    let editors: [&LineEditor; 3] = [
        &state.model_editor,
        &state.base_url_editor,
        &state.key_editor,
    ];
    let mut y = content.y + 2;
    for (i, (label, editor)) in EDIT_FIELD_LABELS.iter().zip(editors.iter()).enumerate() {
        if y + 1 >= content.y + content.height {
            break;
        }
        let focused = i == state.form_field;
        let prefix = if focused { "\u{25b8} " } else { "  " };
        let reveal = i != 2 || state.show_master;
        render_masked_input(
            buf,
            inner_x,
            y,
            inner_width,
            theme,
            &format!("{prefix}{label}: "),
            editor,
            reveal,
        );
        y += 1;
    }
    if y < content.y + content.height {
        render_line(
            buf,
            inner_x,
            y,
            inner_width,
            "Enter: kaydet · Tab: alan değiştir · ctrl+t: key'i göster · Esc: vazgeç",
            Style::default().fg(theme.gray_dim),
        );
    }
    render_error_line(
        buf,
        inner_x,
        inner_width,
        state,
        theme,
        content.y + content.height - 1,
    );
}

fn render_categories(
    buf: &mut Buffer,
    content: Rect,
    inner_x: u16,
    inner_width: u16,
    state: &KeysManagerState,
    theme: &Theme,
) {
    render_line(
        buf,
        inner_x,
        content.y,
        inner_width,
        "Kategoriler (otomatik; Enter: filtrele)",
        Style::default()
            .fg(theme.text_primary)
            .add_modifier(Modifier::BOLD),
    );
    let mut y = content.y + 1;
    if state.category_rows.is_empty() {
        render_line(
            buf,
            inner_x,
            y,
            inner_width,
            "(kayıt yok)",
            Style::default().fg(theme.gray),
        );
        return;
    }
    for (i, filter) in state.category_rows.iter().enumerate() {
        if y >= content.y + content.height {
            break;
        }
        let selected = i == state.category_cursor;
        let style = if selected {
            Style::default()
                .fg(theme.text_primary)
                .add_modifier(Modifier::REVERSED)
        } else {
            Style::default().fg(theme.text_secondary)
        };
        let count = state.visible_entries_for(filter).len();
        let active = state.category_filter.as_ref().is_some_and(|f| f == filter);
        let suffix = if active { " (aktif)" } else { "" };
        render_line(
            buf,
            inner_x,
            y,
            inner_width,
            &format!("{}  [{count}]{suffix}", filter.label()),
            style,
        );
        y += 1;
    }
}

fn render_export_scope(
    buf: &mut Buffer,
    content: Rect,
    inner_x: u16,
    inner_width: u16,
    state: &KeysManagerState,
    theme: &Theme,
) {
    render_line(
        buf,
        inner_x,
        content.y,
        inner_width,
        "Export kapsamı (Enter: seç)",
        Style::default()
            .fg(theme.text_primary)
            .add_modifier(Modifier::BOLD),
    );
    let mut y = content.y + 1;
    let rows: Vec<String> = std::iter::once("All".to_string())
        .chain(state.categories.iter().cloned())
        .collect();
    for (i, row) in rows.iter().enumerate() {
        if y >= content.y + content.height {
            break;
        }
        let selected = i == state.scope_cursor;
        let style = if selected {
            Style::default()
                .fg(theme.text_primary)
                .add_modifier(Modifier::REVERSED)
        } else {
            Style::default().fg(theme.text_secondary)
        };
        render_line(buf, inner_x, y, inner_width, row, style);
        y += 1;
    }
}

fn render_password_input(
    buf: &mut Buffer,
    content: Rect,
    inner_x: u16,
    inner_width: u16,
    state: &KeysManagerState,
    theme: &Theme,
) {
    let title = match &state.mode {
        KeysManagerMode::ExportPassword { .. } => {
            "Export şifresi (bu dosyayı açmak için kullanılır)"
        }
        _ => "Import şifresi",
    };
    let mut y = content.y;
    render_line(
        buf,
        inner_x,
        y,
        inner_width,
        title,
        Style::default()
            .fg(theme.text_primary)
            .add_modifier(Modifier::BOLD),
    );
    y += 1;
    if y < content.y + content.height {
        render_masked_input(
            buf,
            inner_x,
            y,
            inner_width,
            theme,
            " şifre: ",
            &state.master_editor,
            state.show_master,
        );
        y += 1;
    }
    if y < content.y + content.height {
        render_line(
            buf,
            inner_x,
            y,
            inner_width,
            "Enter: onayla · ctrl+t: göster/gizle · Esc: geri",
            Style::default().fg(theme.gray_dim),
        );
    }
    render_error_line(
        buf,
        inner_x,
        inner_width,
        state,
        theme,
        content.y + content.height - 1,
    );
}

fn render_path_input(
    buf: &mut Buffer,
    content: Rect,
    inner_x: u16,
    inner_width: u16,
    state: &KeysManagerState,
    theme: &Theme,
) {
    render_line(
        buf,
        inner_x,
        content.y,
        inner_width,
        "Import dosya yolu (keychain-export-*.omx)",
        Style::default()
            .fg(theme.text_primary)
            .add_modifier(Modifier::BOLD),
    );
    let mut y = content.y + 1;
    if y < content.y + content.height {
        render_line(
            buf,
            inner_x,
            y,
            inner_width,
            " yol: ",
            Style::default().fg(theme.gray),
        );
        let viewport = state
            .path_editor
            .viewport(inner_width.saturating_sub(6) as usize);
        let text = state.path_editor.text();
        let displayed = if text.is_empty() {
            ""
        } else {
            &text[viewport.visible_byte_range]
        };
        if !displayed.is_empty() {
            buf.set_span(
                inner_x + 6,
                y,
                &Span::styled(displayed, Style::default().fg(theme.text_primary)),
                displayed.width() as u16,
            );
        }
        y += 1;
    }
    if y < content.y + content.height {
        render_line(
            buf,
            inner_x,
            y,
            inner_width,
            "Enter: şifre adımı · Esc: geri",
            Style::default().fg(theme.gray_dim),
        );
    }
    render_error_line(
        buf,
        inner_x,
        inner_width,
        state,
        theme,
        content.y + content.height - 1,
    );
}

fn render_done(
    buf: &mut Buffer,
    content: Rect,
    inner_x: u16,
    inner_width: u16,
    state: &KeysManagerState,
    theme: &Theme,
    message: &str,
) {
    let mut y = content.y;
    render_line(
        buf,
        inner_x,
        y,
        inner_width,
        message,
        Style::default().fg(theme.accent_success),
    );
    y += 1;
    render_line(
        buf,
        inner_x,
        y,
        inner_width,
        "Enter / Esc: devam",
        Style::default().fg(theme.gray_dim),
    );
    render_error_line(
        buf,
        inner_x,
        inner_width,
        state,
        theme,
        content.y + content.height - 1,
    );
}

fn render_stack_pick(
    buf: &mut Buffer,
    content: Rect,
    inner_x: u16,
    inner_width: u16,
    state: &KeysManagerState,
    theme: &Theme,
) {
    let mut y = content.y;
    render_line(
        buf,
        inner_x,
        y,
        inner_width,
        "Harici stack seç (tespit edilenler)",
        Style::default()
            .fg(theme.text_primary)
            .add_modifier(Modifier::BOLD),
    );
    y += 1;
    if state.stack_rows.is_empty() {
        render_line(
            buf,
            inner_x,
            y,
            inner_width,
            "stack bulunamadı",
            Style::default().fg(theme.accent_error),
        );
        return;
    }
    for (i, (id, label, path)) in state.stack_rows.iter().enumerate() {
        if y >= content.y + content.height {
            break;
        }
        let selected = i == state.scope_cursor;
        let style = if selected {
            Style::default()
                .fg(theme.text_primary)
                .add_modifier(Modifier::REVERSED)
        } else {
            Style::default().fg(theme.text_secondary)
        };
        render_line(
            buf,
            inner_x,
            y,
            inner_width,
            &format!("{id:<14}  {label:<18}  {path}"),
            style,
        );
        y += 1;
    }
}

fn render_stack_direction(
    buf: &mut Buffer,
    content: Rect,
    inner_x: u16,
    inner_width: u16,
    state: &KeysManagerState,
    stack_label: &str,
    theme: &Theme,
) {
    let mut y = content.y;
    render_line(
        buf,
        inner_x,
        y,
        inner_width,
        &format!("{stack_label} — yön seç"),
        Style::default()
            .fg(theme.text_primary)
            .add_modifier(Modifier::BOLD),
    );
    y += 1;
    let opts = [
        "stack → omnitrix (import, merge)",
        "omnitrix → stack (export, merge)",
    ];
    for (i, label) in opts.iter().enumerate() {
        let selected = i == state.scope_cursor.min(1);
        let style = if selected {
            Style::default()
                .fg(theme.text_primary)
                .add_modifier(Modifier::REVERSED)
        } else {
            Style::default().fg(theme.text_secondary)
        };
        render_line(buf, inner_x, y, inner_width, label, style);
        y += 1;
    }
}

fn render_stack_lines(
    buf: &mut Buffer,
    content: Rect,
    inner_x: u16,
    inner_width: u16,
    state: &KeysManagerState,
    lines: &[String],
    theme: &Theme,
) {
    let mut y = content.y;
    for (i, line) in lines.iter().enumerate() {
        if y >= content.y + content.height {
            break;
        }
        let style = if i == 0 {
            Style::default()
                .fg(theme.text_primary)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.text_secondary)
        };
        render_line(buf, inner_x, y, inner_width, line, style);
        y += 1;
    }
    render_error_line(
        buf,
        inner_x,
        inner_width,
        state,
        theme,
        content.y + content.height.saturating_sub(1),
    );
}

// ---------------------------------------------------------------------------
// Testler
// ---------------------------------------------------------------------------

#[cfg(test)]
#[path = "keys_manager_tests.rs"]
mod tests;

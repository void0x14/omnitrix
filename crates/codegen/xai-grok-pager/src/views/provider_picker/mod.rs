//! /connect wizard: provider → key → model → apply.
//!
//! Task 7 kapsamı: durum makinesi + provider adımı (canlı liste, rozetler,
//! fuzzy arama). BaseUrl/Key/Category/Model/Apply adımları placeholder
//! render + Esc→Back navigasyonu üretir; gerçek ekranlar Task 8'de gelir.

mod providers;

use crossterm::event::{Event, KeyCode, KeyEventKind};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;
use zeroize::Zeroizing;

use xai_grok_shell::sampling::ApiBackend;
use xai_grok_shell::util::models_dev::{
    CacheSource, CatalogCache, ModelInfo, api_backend_for_provider, base_url_for_provider,
};
use xai_omni_keychain::KeyEntry;

use crate::views::picker::{
    PickerConfig, PickerEntry, PickerOutcome, PickerRow, PickerState, handle_picker_input,
    render_picker_content_with_scrollbar_x,
};

use self::providers::{EnvMap, ProviderBadge, ProviderRow, filter_provider_rows, provider_rows};

/// Wizard adımı.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectStep {
    /// Provider seçimi (picker: fuzzy, rozetler, custom satırlar).
    Provider,
    /// Custom provider'larda base URL girişi.
    BaseUrl,
    /// Key girişi / keychain seçimi / env seçimi.
    Key,
    /// Kategori seçimi (opsiyonel, default kullanılabilir).
    Category,
    /// Model seçimi.
    Model,
    /// Config yazma + switch (async).
    Apply,
    /// Tamamlandı.
    Done,
    /// Geri dönülemez adım hatası.
    Error(String),
}

/// Key kaynağı seçimi.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyMode {
    /// Yeni key elle girilecek (masked input).
    New,
    /// Mevcut keychain kaydı (id).
    Keychain(String),
    /// Ortam değişkeninden key (ad).
    Env(String),
}

/// Seçilen provider'ın özeti.
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderSelection {
    pub provider_id: String,
    pub label: String,
    pub is_custom: bool,
    pub backend: ApiBackend,
    /// Default/önerilen base URL.
    pub base_url: Option<String>,
    /// Canlı model listesi (models.dev veya fetch).
    pub models: Vec<ModelInfo>,
}

/// Provider connect wizard durum makinesi.
///
/// `catalog` başlangıçta boş (`CacheSource::Offline`) olabilir; dispatch
/// `Effect::FetchModelsCatalog` ile async doldurur ([`Self::set_catalog`]).
pub struct ProviderConnectFlow {
    pub step: ConnectStep,
    pub catalog: CatalogCache,
    pub selected_provider: Option<ProviderSelection>,
    pub key_mode: KeyMode,
    pub draft_key: Zeroizing<String>,
    pub base_url_draft: String,
    pub selected_model: Option<String>,
    pub error: Option<String>,
    pub keychain_entries: Vec<KeyEntry>,
    // ── Task 7 render/input ekleri ──
    /// Provider adımının satırları (katalog + config + custom; bir kez
    /// kurulur, `set_catalog` ile tazelenir).
    pub rows: Vec<ProviderRow>,
    /// Provider adımının paylaşılan picker state'i (query, seçim, scroll).
    pub picker: PickerState,
}

impl ProviderConnectFlow {
    /// Yeni wizard: provider adımında, type-to-find arama açık başlar.
    ///
    /// Config `[model_providers.*]` kayıtları şimdilik boş geçilir (Task 8
    /// dispatch'ten gerçek config'i enjekte eder); env görünümü process
    /// ortamından (`std::env::var_os`) doldurulur.
    pub fn new(catalog: CatalogCache, keychain_entries: Vec<KeyEntry>) -> Self {
        let env_names: Vec<String> = catalog
            .providers
            .values()
            .filter_map(|p| p.env.first().cloned())
            .collect();
        let env = EnvMap::from_env_vars(&env_names);
        let rows = provider_rows(
            &catalog,
            &keychain_entries,
            &indexmap::IndexMap::new(),
            &env,
        );
        Self {
            step: ConnectStep::Provider,
            catalog,
            selected_provider: None,
            key_mode: KeyMode::New,
            draft_key: Zeroizing::new(String::new()),
            base_url_draft: String::new(),
            selected_model: None,
            error: None,
            keychain_entries,
            rows,
            picker: PickerState::input_active(),
        }
    }

    /// Katalog async yüklendikten sonra satırları tazeler (hata temizlenir).
    pub fn set_catalog(&mut self, catalog: CatalogCache) {
        self.catalog = catalog;
        self.error = None;
        let env_names: Vec<String> = self
            .catalog
            .providers
            .values()
            .filter_map(|p| p.env.first().cloned())
            .collect();
        let env = EnvMap::from_env_vars(&env_names);
        self.rows = provider_rows(
            &self.catalog,
            &self.keychain_entries,
            &indexmap::IndexMap::new(),
            &env,
        );
    }

    /// Seçili satırı `ProviderSelection`'a çevirir ve adımı ilerletir:
    /// custom → `BaseUrl`, builtin → `Key`.
    fn select_row(&mut self, row: &ProviderRow) {
        let catalog_provider = self.catalog.providers.get(&row.provider_id).cloned();
        let models: Vec<ModelInfo> = catalog_provider
            .as_ref()
            .map(|p| p.models.values().cloned().collect())
            .unwrap_or_default();
        let backend = catalog_provider
            .as_ref()
            .map(api_backend_for_provider)
            .unwrap_or_else(|| {
                if row.provider_id == "custom-anthropic" {
                    ApiBackend::Messages
                } else {
                    ApiBackend::ChatCompletions
                }
            });
        let base_url = row
            .base_url
            .clone()
            .or_else(|| catalog_provider.as_ref().and_then(base_url_for_provider));
        self.selected_provider = Some(ProviderSelection {
            provider_id: row.provider_id.clone(),
            label: row.label.clone(),
            is_custom: row.is_custom,
            backend,
            base_url: base_url.clone(),
            models,
        });
        if row.is_custom {
            self.base_url_draft = base_url.unwrap_or_default();
            self.step = ConnectStep::BaseUrl;
        } else {
            self.step = ConnectStep::Key;
        }
    }
}

/// `handle_connect_input` çıktısı: modal katmanı bu sonuçlarla modalı
/// kapatır / aksiyon üretir (Task 8'de `Apply` → `Action::ConnectProvider`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectOutcome {
    /// Adım ilerledi (task içinde yapıldı; modal aynı kalır).
    Next,
    /// Geri adım.
    Back,
    /// Wizard iptal edildi (modal kapanır).
    Cancel,
    PickProvider(String),
    PickBaseUrl(String),
    PickKeyMode(KeyMode),
    PickModel(String),
    /// Apply adımı onaylandı — Task 8'de config yazma + switch başlar.
    Apply,
    /// Görsel değişiklik / işlenmedi.
    Nothing,
}

/// Wizard input'u: adıma göre yönlendirir.
///
/// - Provider: picker input'u (fuzzy, gezinme, Enter seç, Esc çık).
/// - Diğer adımlar (Task 8 placeholder): yalnızca Esc → geri.
pub fn handle_connect_input(flow: &mut ProviderConnectFlow, ev: &Event) -> ConnectOutcome {
    match flow.step {
        ConnectStep::Provider => handle_provider_step_input(flow, ev),
        ConnectStep::BaseUrl
        | ConnectStep::Key
        | ConnectStep::Category
        | ConnectStep::Model
        | ConnectStep::Apply
        | ConnectStep::Done => handle_placeholder_step_input(flow, ev),
        ConnectStep::Error(_) => {
            // Hata ekranından Esc → provider listesine dön.
            if let Event::Key(key) = ev
                && key.kind == KeyEventKind::Press
                && key.code == KeyCode::Esc
            {
                flow.error = None;
                flow.step = ConnectStep::Provider;
                return ConnectOutcome::Back;
            }
            ConnectOutcome::Nothing
        }
    }
}

/// Provider adımı: picker'a yönlendirir; Esc boş query ile iptaldir.
fn handle_provider_step_input(flow: &mut ProviderConnectFlow, ev: &Event) -> ConnectOutcome {
    let filtered = filter_provider_rows(&flow.rows, flow.picker.query());
    let entry_count = filtered.len();
    let config = PickerConfig {
        title: None,
        show_search_hint: false,
        expandable: false,
        esc_clears_query: true,
        shortcuts: Some(crate::views::picker::picker_shortcuts()),
        pending_hint: None,
        non_selectable: &[],
        non_selectable_clickable: &[],
        shortcuts_area: None,
        tabs: None,
        active_tab: 0,
        filter_label: None,
        filter_key_hint: None,
        filter_active: false,
        header_note: None,
        action_keys: &[],
        disable_search: false,
        compact_bottom_bar: false,
        search_only_on_slash: false,
        vim_normal_first: crate::appearance::cache::load_vim_mode(),
    };
    match handle_picker_input(ev, &mut flow.picker, entry_count, &config) {
        PickerOutcome::Selected(i) => match filtered.get(i) {
            Some(row) => {
                let row = row.clone();
                flow.select_row(&row);
                ConnectOutcome::PickProvider(row.provider_id)
            }
            None => ConnectOutcome::Nothing,
        },
        // esc_clears_query yukarıda halletti: boş query ile Esc → iptal.
        PickerOutcome::Closed => ConnectOutcome::Cancel,
        PickerOutcome::QueryChanged | PickerOutcome::Changed => ConnectOutcome::Nothing,
        PickerOutcome::Unchanged => ConnectOutcome::Nothing,
        _ => ConnectOutcome::Nothing,
    }
}

/// Task 8 placeholder adımları: yalnızca Esc → geri (Provider'a dön).
fn handle_placeholder_step_input(flow: &mut ProviderConnectFlow, ev: &Event) -> ConnectOutcome {
    if let Event::Key(key) = ev
        && key.kind == KeyEventKind::Press
        && key.code == KeyCode::Esc
    {
        flow.step = ConnectStep::Provider;
        return ConnectOutcome::Back;
    }
    ConnectOutcome::Nothing
}

/// Adım başlığı (placeholder render için).
fn step_title(step: &ConnectStep) -> &'static str {
    match step {
        ConnectStep::Provider => "Provider",
        ConnectStep::BaseUrl => "BaseUrl",
        ConnectStep::Key => "Key",
        ConnectStep::Category => "Category",
        ConnectStep::Model => "Model",
        ConnectStep::Apply => "Apply",
        ConnectStep::Done => "Done",
        ConnectStep::Error(_) => "Error",
    }
}

/// Wizard'ı modal content alanına çizer. Provider adımı paylaşılan picker
/// primitifleriyle (arama çubuğu + rozetli satırlar + scrollbar) çizilir;
/// diğer adımlar ortalanmış placeholder metin üretir (Task 8 gerçek ekranlar).
pub fn render_connect_flow(
    buf: &mut Buffer,
    content: Rect,
    inner_x: u16,
    inner_width: u16,
    theme: &crate::theme::Theme,
    flow: &mut ProviderConnectFlow,
) {
    // Adımı klonla: placeholder arm'ları `flow`'u `&mut` almazken Provider
    // arm'ı `&mut flow` ister — scrutinee borrow'u arm'lar arasında canlı
    // tutmamak için klon üzerinden eşleşiriz (String yalnızca Error'da).
    let step = flow.step.clone();
    match step {
        ConnectStep::Provider => {
            render_provider_step(buf, content, inner_x, inner_width, theme, flow);
        }
        ConnectStep::Error(msg) => {
            render_placeholder_step(buf, content, theme, &format!("Hata: {msg}"));
        }
        step => render_placeholder_step(
            buf,
            content,
            theme,
            &format!("Adim: {} \u{2014} Task 8", step_title(&step)),
        ),
    }
}

/// Provider adımı: arama çubuğu + divider + filtrelenmiş satırlar.
fn render_provider_step(
    buf: &mut Buffer,
    content: Rect,
    inner_x: u16,
    inner_width: u16,
    theme: &crate::theme::Theme,
    flow: &mut ProviderConnectFlow,
) {
    use crate::views::picker::render_picker_search_bar;

    // Katalog hâlâ yükleniyorsa (Offline) spinner göster — satırlar gelince
    // liste çizilir. Fetch hatası `flow.error`'a düştüyse mesaj gösterilir.
    let loading = flow.catalog.source == CacheSource::Offline;
    if let Some(err) = &flow.error {
        render_placeholder_step(buf, content, theme, &format!("Katalog yüklenemedi: {err}"));
        return;
    }

    render_picker_search_bar(
        buf,
        content.x,
        content.y,
        content.width,
        theme,
        &flow.picker,
        flow.picker.search_active,
        true,
        Some(theme.bg_base),
    );
    let sep_y = content.y + 1;
    if sep_y < content.y + content.height {
        crate::views::picker::render_divider(
            buf,
            inner_x,
            sep_y,
            inner_width,
            theme,
            Some(theme.bg_base),
        );
    }
    let entries_start_y = sep_y + 1;
    let search_bar_rect = Rect::new(content.x, content.y, content.width, 1);
    let entries_area = Rect {
        x: content.x,
        y: entries_start_y,
        width: content.width,
        height: content
            .height
            .saturating_sub(entries_start_y.saturating_sub(content.y)),
    };

    let filtered = filter_provider_rows(&flow.rows, flow.picker.query());
    let badge_labels: Vec<String> = filtered.iter().map(|r| r.badge.label()).collect();
    let entries: Vec<PickerEntry> = filtered
        .iter()
        .enumerate()
        .map(|(i, row)| {
            PickerEntry::Row(PickerRow {
                label: &row.label,
                right_label: row.base_url.as_deref().unwrap_or(""),
                selected: flow.picker.hovered == Some(i)
                    || (flow.picker.hovered.is_none() && i == flow.picker.selected),
                expanded: false,
                fields: &[],
                description_lines: &[],
                summary_lines: &[],
                dimmed: false,
                indent: 0,
                badge: &badge_labels[i],
                badge_color: Some(badge_color(&row.badge, theme)),
                collapsible: false,
                underline_last_desc: false,
            })
        })
        .collect();
    let content_hit = render_picker_content_with_scrollbar_x(
        buf,
        entries_area,
        theme,
        &mut flow.picker,
        &entries,
        &[],
        &[],
        Some(theme.bg_base),
        loading,
        0,
        inner_x + inner_width - 1,
    );
    flow.picker.hit_areas = Some(crate::views::picker::PickerHitAreas {
        close_button: Rect::default(),
        search_bar: search_bar_rect,
        item_rects: content_hit.item_rects,
        entry_indices: content_hit.entry_indices,
        tab_rects: vec![],
        filter_rect: None,
    });
}

/// Rozet rengi: keychain yeşil, env mavi, yeni gri.
fn badge_color(badge: &ProviderBadge, theme: &crate::theme::Theme) -> Color {
    match badge {
        ProviderBadge::Keychain => theme.accent_system,
        ProviderBadge::Env(_) => theme.accent_user,
        ProviderBadge::New => theme.gray,
    }
}

/// Ortalanmış placeholder metin (Task 8 gerçek ekranlarla değişecek).
fn render_placeholder_step(
    buf: &mut Buffer,
    content: Rect,
    theme: &crate::theme::Theme,
    text: &str,
) {
    if content.height == 0 || content.width == 0 {
        return;
    }
    let style = Style::default().fg(theme.gray).bg(theme.bg_base);
    let line = Line::from(Span::styled(text, style));
    let x = content.x + content.width.saturating_sub(text.width() as u16) / 2;
    let y = content.y + content.height / 2;
    buf.set_line(x, y, &line, content.width);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEvent, KeyModifiers};
    use xai_grok_shell::util::models_dev::ProviderCatalog;
    use xai_omni_keychain::KeySource;

    fn catalog_with_openai() -> CatalogCache {
        let mut models = indexmap::IndexMap::new();
        models.insert(
            "gpt-4o".to_string(),
            ModelInfo {
                id: "gpt-4o".to_string(),
                name: "GPT-4o".to_string(),
                description: None,
                reasoning: false,
                tool_call: true,
                temperature: true,
                limit: None,
                cost: None,
            },
        );
        CatalogCache {
            providers: indexmap::IndexMap::from([(
                "openai".to_string(),
                ProviderCatalog {
                    id: "openai".to_string(),
                    name: "OpenAI".to_string(),
                    env: vec!["OPENAI_API_KEY".to_string()],
                    npm: Some("@ai-sdk/openai".to_string()),
                    api: None,
                    doc: None,
                    models,
                },
            )]),
            fetched_at: None,
            source: CacheSource::Fresh,
        }
    }

    fn empty_catalog() -> CatalogCache {
        CatalogCache {
            providers: indexmap::IndexMap::new(),
            fetched_at: None,
            source: CacheSource::Offline,
        }
    }

    fn press(key: KeyCode) -> Event {
        Event::Key(KeyEvent::new(key, KeyModifiers::NONE))
    }

    fn key_down() -> Event {
        press(KeyCode::Down)
    }

    fn key_enter() -> Event {
        press(KeyCode::Enter)
    }

    fn key_esc() -> Event {
        press(KeyCode::Esc)
    }

    #[test]
    fn new_starts_at_provider_step_with_input_active() {
        let flow = ProviderConnectFlow::new(empty_catalog(), vec![]);
        assert_eq!(flow.step, ConnectStep::Provider);
        assert!(flow.picker.search_active);
        // Custom satırlar her zaman var (katalog boş olsa bile).
        assert_eq!(flow.rows.len(), 2);
        assert!(flow.rows.iter().all(|r| r.is_custom));
    }

    #[test]
    fn selecting_builtin_advances_to_key_step() {
        let mut flow = ProviderConnectFlow::new(catalog_with_openai(), vec![]);
        // "openai" ilk satır (katalog tek provider).
        let out = handle_connect_input(&mut flow, &key_enter());
        assert_eq!(out, ConnectOutcome::PickProvider("openai".to_string()));
        assert_eq!(flow.step, ConnectStep::Key);
        let sel = flow.selected_provider.expect("selected");
        assert_eq!(sel.provider_id, "openai");
        assert!(!sel.is_custom);
        assert_eq!(sel.backend, ApiBackend::Responses); // @ai-sdk/openai
        assert_eq!(sel.base_url.as_deref(), Some("https://api.openai.com/v1"));
        assert_eq!(sel.models.len(), 1);
        assert_eq!(sel.models[0].id, "gpt-4o");
    }

    #[test]
    fn selecting_custom_advances_to_base_url_step() {
        let mut flow = ProviderConnectFlow::new(empty_catalog(), vec![]);
        // Sıra: custom-openai (0), custom-anthropic (1) — ikisi de custom.
        let out = handle_connect_input(&mut flow, &key_enter());
        assert_eq!(
            out,
            ConnectOutcome::PickProvider("custom-openai".to_string())
        );
        assert_eq!(flow.step, ConnectStep::BaseUrl);
        assert_eq!(flow.base_url_draft, "https://api.openai.com/v1".to_string());
        assert!(flow.selected_provider.as_ref().unwrap().is_custom);
        assert_eq!(
            flow.selected_provider.as_ref().unwrap().backend,
            ApiBackend::ChatCompletions
        );
    }

    #[test]
    fn custom_anthropic_backend_is_messages() {
        let mut flow = ProviderConnectFlow::new(empty_catalog(), vec![]);
        let _ = handle_connect_input(&mut flow, &key_down()); // custom-anthropic
        let out = handle_connect_input(&mut flow, &key_enter());
        assert_eq!(
            out,
            ConnectOutcome::PickProvider("custom-anthropic".to_string())
        );
        assert_eq!(
            flow.selected_provider.as_ref().unwrap().backend,
            ApiBackend::Messages
        );
    }

    #[test]
    fn esc_from_provider_cancels() {
        let mut flow = ProviderConnectFlow::new(empty_catalog(), vec![]);
        let out = handle_connect_input(&mut flow, &key_esc());
        assert_eq!(out, ConnectOutcome::Cancel);
    }

    #[test]
    fn esc_from_placeholder_step_goes_back() {
        let mut flow = ProviderConnectFlow::new(empty_catalog(), vec![]);
        let _ = handle_connect_input(&mut flow, &key_enter()); // → BaseUrl
        assert_eq!(flow.step, ConnectStep::BaseUrl);
        let out = handle_connect_input(&mut flow, &key_esc());
        assert_eq!(out, ConnectOutcome::Back);
        assert_eq!(flow.step, ConnectStep::Provider);
    }

    #[test]
    fn esc_with_query_clears_query_not_cancel() {
        let mut flow = ProviderConnectFlow::new(empty_catalog(), vec![]);
        let _ = handle_connect_input(&mut flow, &press(KeyCode::Char('x')));
        assert_eq!(flow.picker.query(), "x");
        let out = handle_connect_input(&mut flow, &key_esc());
        assert_eq!(
            out,
            ConnectOutcome::Nothing,
            "query temizlenir, iptal olmaz"
        );
        assert!(flow.picker.query().is_empty());
        // İkinci Esc (boş query) artık iptal eder.
        let out = handle_connect_input(&mut flow, &key_esc());
        assert_eq!(out, ConnectOutcome::Cancel);
    }

    #[test]
    fn typing_filters_rows_and_selection_follows_filter() {
        let mut flow = ProviderConnectFlow::new(catalog_with_openai(), vec![]);
        let _ = handle_connect_input(&mut flow, &press(KeyCode::Char('a')));
        assert_eq!(flow.picker.query(), "a");
        // 'a' filtresi: "OpenAI" + "Custom provider (Anthropic compatible)"
        // — hepsi 'a' içerir; seçim ilk satırda kalır → openai.
        let filtered = filter_provider_rows(&flow.rows, flow.picker.query());
        assert_eq!(filtered[0].provider_id, "openai");
        let out = handle_connect_input(&mut flow, &key_enter());
        assert_eq!(out, ConnectOutcome::PickProvider("openai".to_string()));
    }

    #[test]
    fn set_catalog_refreshes_rows_and_clears_error() {
        let mut flow = ProviderConnectFlow::new(empty_catalog(), vec![]);
        flow.error = Some("deneme hatası".to_string());
        flow.set_catalog(catalog_with_openai());
        assert!(flow.error.is_none());
        assert_eq!(flow.rows.len(), 3); // openai + 2 custom
        assert_eq!(flow.rows[0].provider_id, "openai");
    }

    #[test]
    fn error_step_esc_returns_to_provider() {
        let mut flow = ProviderConnectFlow::new(empty_catalog(), vec![]);
        flow.step = ConnectStep::Error("bir şeyler ters gitti".to_string());
        let out = handle_connect_input(&mut flow, &key_esc());
        assert_eq!(out, ConnectOutcome::Back);
        assert_eq!(flow.step, ConnectStep::Provider);
    }

    #[test]
    fn placeholder_steps_ignore_other_keys() {
        let mut flow = ProviderConnectFlow::new(empty_catalog(), vec![]);
        flow.step = ConnectStep::Model;
        let out = handle_connect_input(&mut flow, &key_enter());
        assert_eq!(out, ConnectOutcome::Nothing);
        assert_eq!(flow.step, ConnectStep::Model);
    }

    #[test]
    fn keychain_entries_flow_through_to_rows() {
        let entry = KeyEntry {
            id: "k_openai".to_string(),
            category: "personal".to_string(),
            provider_id: "openai".to_string(),
            provider_label: "OpenAI".to_string(),
            masked: "sk-…a1b2".to_string(),
            model_id: None,
            base_url: None,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            last_used: None,
            source: KeySource::Manual,
        };
        let flow = ProviderConnectFlow::new(catalog_with_openai(), vec![entry]);
        assert_eq!(flow.rows[0].badge, ProviderBadge::Keychain);
    }

    #[test]
    fn key_mode_defaults_to_new() {
        let flow = ProviderConnectFlow::new(empty_catalog(), vec![]);
        assert_eq!(flow.key_mode, KeyMode::New);
        assert!(flow.draft_key.is_empty());
        assert!(flow.selected_model.is_none());
    }
}

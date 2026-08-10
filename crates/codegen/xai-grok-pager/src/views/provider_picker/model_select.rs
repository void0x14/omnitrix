//! Task 8: Model seçimi adımı.
//!
//! `selected_provider.models` listesinden fuzzy picker: `name` (+ `id` dim),
//! rozetler reasoning `[R]`, context `128k`, fiyat `$2.5/M`. Liste boşsa
//! (offline/custom) modals katmanı `Action::FetchProviderModels` tetikler
//! (openai-compatible `/models`); başarısızsa hata satırı + manuel model ID
//! girişi her zaman kullanılabilir (altta "manuel model ID gir…" satırı).

use crossterm::event::{Event, KeyCode, KeyEventKind};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use xai_grok_shell::util::models_dev::ModelInfo;

use crate::views::picker::{
    PickerConfig, PickerEntry, PickerHitAreas, PickerOutcome, PickerRow, handle_picker_input,
    render_picker_content_with_scrollbar_x,
};

use super::{ConnectOutcome, ConnectStep, ProviderConnectFlow};

/// `/models` fetch durumu (Model adımı; offline/custom fallback).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelFetchState {
    /// Fetch henüz tetiklenmedi.
    Idle,
    /// `Action::FetchProviderModels` iletildi; sonuç bekleniyor.
    Fetching,
    /// `data[].id` listesi `selected_provider.models`'a yüklendi.
    Loaded,
    /// Fetch başarısız; manuel ID girişi kullanılabilir.
    Failed(String),
}

/// Model adımına girerken geçici durumu sıfırla (editörler, picker).
pub(super) fn enter_model_step(flow: &mut ProviderConnectFlow) {
    flow.model_manual_mode = false;
    flow.model_editor.reset();
    flow.picker.reset();
    flow.picker.search_active = true;
}

/// Query ile modelleri filtreler (name + id, büyük/küçük harf duyarsız).
pub(super) fn filtered_models(flow: &ProviderConnectFlow) -> Vec<ModelInfo> {
    let q = flow.picker.query().trim().to_lowercase();
    let Some(sel) = flow.selected_provider.as_ref() else {
        return vec![];
    };
    if q.is_empty() {
        return sel.models.clone();
    }
    sel.models
        .iter()
        .filter(|m| m.name.to_lowercase().contains(&q) || m.id.to_lowercase().contains(&q))
        .cloned()
        .collect()
}

/// Rozet etiketi: `[R] 128k $2.5/M` (yalnızca dolu alanlar).
pub(super) fn model_badge(m: &ModelInfo) -> String {
    let mut parts = Vec::new();
    if m.reasoning {
        parts.push("[R]".to_string());
    }
    if let Some(lim) = &m.limit
        && lim.context > 0
    {
        parts.push(format_context(lim.context));
    }
    if let Some(cost) = &m.cost
        && cost.input > 0.0
    {
        parts.push(format!("${:.1}/M", cost.input));
    }
    parts.join(" ")
}

/// Token sayısını kısa biçime çevirir: 128000 → "128k", 2000000 → "2M".
pub(super) fn format_context(ctx: u64) -> String {
    if ctx >= 1_000_000 {
        format!("{}M", ctx / 1_000_000)
    } else if ctx >= 1_000 {
        format!("{}k", ctx / 1_000)
    } else {
        ctx.to_string()
    }
}

pub(super) fn handle_model_step_input(
    flow: &mut ProviderConnectFlow,
    ev: &Event,
) -> ConnectOutcome {
    if flow.model_manual_mode {
        return handle_model_manual_typing(flow, ev);
    }
    let filtered = filtered_models(flow);
    // +1: altta her zaman var olan "manuel model ID gir…" satırı.
    let entry_count = filtered.len() + 1;
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
        PickerOutcome::Selected(i) => {
            if i < filtered.len() {
                let m = filtered[i].clone();
                flow.selected_model = Some(m.id.clone());
                flow.apply_pending = false;
                flow.step = ConnectStep::Apply;
                ConnectOutcome::PickModel(m.id)
            } else {
                // Manuel satır: query'de yazılı ID varsa editöre taşı.
                flow.model_manual_mode = true;
                flow.model_editor.reset();
                let q = flow.picker.query().trim().to_string();
                if !q.is_empty() {
                    flow.model_editor.set_text(q);
                    flow.picker.clear_query();
                }
                ConnectOutcome::Nothing
            }
        }
        // Query'ye uyan model yok (pickler'da hiç satır kalmadıysa): manuel
        // girişe query ile geç.
        PickerOutcome::SubmitQuery => {
            let q = flow.picker.query().trim().to_string();
            if !q.is_empty() {
                flow.model_manual_mode = true;
                flow.model_editor.set_text(q);
                flow.picker.clear_query();
            }
            ConnectOutcome::Nothing
        }
        PickerOutcome::Closed => {
            flow.picker.reset();
            flow.step = ConnectStep::Key;
            super::key_input::enter_key_step(flow);
            ConnectOutcome::Back
        }
        PickerOutcome::QueryChanged
        | PickerOutcome::Changed
        | PickerOutcome::Unchanged
        | PickerOutcome::Expand(_)
        | PickerOutcome::Collapse(_)
        | PickerOutcome::Copy(_)
        | PickerOutcome::NonSelectableClick(_)
        | PickerOutcome::TabChanged(_)
        | PickerOutcome::FilterCycled
        | PickerOutcome::Action(_) => ConnectOutcome::Nothing,
    }
}

/// Manuel model ID girişi: Enter → onayla (boş olamaz), Esc → listeye dön.
fn handle_model_manual_typing(flow: &mut ProviderConnectFlow, ev: &Event) -> ConnectOutcome {
    match ev {
        Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
            KeyCode::Esc => {
                flow.model_manual_mode = false;
                flow.model_editor.reset();
                ConnectOutcome::Nothing
            }
            KeyCode::Enter => {
                let id = flow.model_editor.text().trim().to_string();
                if id.is_empty() {
                    return ConnectOutcome::Nothing;
                }
                flow.selected_model = Some(id.clone());
                flow.apply_pending = false;
                flow.step = ConnectStep::Apply;
                ConnectOutcome::PickModel(id)
            }
            _ => {
                flow.model_editor.handle_key(key);
                ConnectOutcome::Nothing
            }
        },
        Event::Paste(text) => {
            flow.model_editor.insert_paste(text);
            ConnectOutcome::Nothing
        }
        _ => ConnectOutcome::Nothing,
    }
}

pub(super) fn render_model_step(
    buf: &mut Buffer,
    content: Rect,
    inner_x: u16,
    inner_width: u16,
    theme: &crate::theme::Theme,
    flow: &mut ProviderConnectFlow,
) {
    if content.height == 0 || content.width == 0 {
        return;
    }
    // Başlık.
    let provider = flow
        .selected_provider
        .as_ref()
        .map(|s| s.label.as_str())
        .unwrap_or("");
    let mut y = content.y;
    buf.set_line(
        inner_x,
        y,
        &Line::from(Span::styled(
            format!("Model \u{2014} {provider}"),
            Style::default().fg(theme.gray).bg(theme.bg_base),
        )),
        inner_width,
    );
    y += 1;
    if y >= content.y + content.height {
        return;
    }

    // Arama çubuğu.
    let search_bar_rect = Rect::new(inner_x, y, inner_width, 1);
    crate::views::picker::render_picker_search_bar(
        buf,
        inner_x,
        y,
        inner_width,
        theme,
        &flow.picker,
        flow.picker.search_active,
        true,
        Some(theme.bg_base),
    );
    y += 1;
    let sep_y = y;
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
    let entries_area = Rect {
        x: inner_x,
        y: entries_start_y,
        width: inner_width,
        height: content
            .height
            .saturating_sub(entries_start_y.saturating_sub(content.y)),
    };
    if entries_area.height == 0 {
        return;
    }

    let filtered = filtered_models(flow);
    let badges: Vec<String> = filtered.iter().map(model_badge).collect();
    let manual_label = "manuel model ID gir\u{2026}";
    let mut entries: Vec<PickerEntry> = Vec::with_capacity(filtered.len() + 1);
    for (i, m) in filtered.iter().enumerate() {
        entries.push(PickerEntry::Row(PickerRow {
            label: &m.name,
            right_label: if m.name != m.id { m.id.as_str() } else { "" },
            selected: flow.picker.hovered == Some(i)
                || (flow.picker.hovered.is_none() && i == flow.picker.selected),
            expanded: false,
            fields: &[],
            description_lines: &[],
            summary_lines: &[],
            dimmed: false,
            indent: 0,
            badge: badges.get(i).map(String::as_str).unwrap_or(""),
            badge_color: Some(if m.reasoning {
                theme.accent_system
            } else {
                theme.gray_bright
            }),
            collapsible: false,
            underline_last_desc: false,
        }));
    }
    // Manuel ID satırı: her zaman son satır (dim).
    entries.push(PickerEntry::Row(PickerRow {
        label: manual_label,
        right_label: "",
        selected: flow.picker.hovered == Some(filtered.len())
            || (flow.picker.hovered.is_none() && flow.picker.selected == filtered.len()),
        expanded: false,
        fields: &[],
        description_lines: &[],
        summary_lines: &[],
        dimmed: true,
        indent: 0,
        badge: "",
        badge_color: None,
        collapsible: false,
        underline_last_desc: false,
    }));

    let content_hit = render_picker_content_with_scrollbar_x(
        buf,
        entries_area,
        theme,
        &mut flow.picker,
        &entries,
        &[],
        &[],
        Some(theme.bg_base),
        false,
        0,
        inner_x + inner_width - 1,
    );
    flow.picker.hit_areas = Some(PickerHitAreas {
        close_button: Rect::default(),
        search_bar: search_bar_rect,
        item_rects: content_hit.item_rects,
        entry_indices: content_hit.entry_indices,
        tab_rects: vec![],
        filter_rect: None,
    });

    // Manuel giriş modu satırı + fetch durumu.
    let bottom_y = content.y + content.height - 1;
    if flow.model_manual_mode {
        crate::views::picker::render_line_editor_search_bar(
            buf,
            inner_x,
            bottom_y,
            inner_width,
            theme,
            &flow.model_editor,
            true,
            false,
            Some(theme.bg_base),
        );
    } else if filtered.is_empty() {
        let hint = match &flow.models_fetch_state {
            ModelFetchState::Fetching => "modeller yükleniyor\u{2026}".to_string(),
            ModelFetchState::Failed(e) => {
                format!("\u{2717} model listesi alınamadı: {e} \u{2014} manuel ID girebilirsin")
            }
            ModelFetchState::Loaded | ModelFetchState::Idle => {
                "liste boş \u{2014} manuel ID girebilirsin".to_string()
            }
        };
        let style = if matches!(flow.models_fetch_state, ModelFetchState::Failed(_)) {
            Style::default().fg(theme.accent_error).bg(theme.bg_base)
        } else {
            Style::default().fg(theme.gray_dim).bg(theme.bg_base)
        };
        buf.set_line(
            inner_x,
            bottom_y,
            &Line::from(Span::styled(hint, style)),
            inner_width,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEvent, KeyModifiers};
    use xai_grok_shell::util::models_dev::{CacheSource, CatalogCache, ModelCost, ModelLimits};

    fn press(key: KeyCode) -> Event {
        Event::Key(KeyEvent::new(key, KeyModifiers::NONE))
    }

    fn catalog_with_models() -> CatalogCache {
        use xai_grok_shell::util::models_dev::ProviderCatalog;
        let mut models = indexmap::IndexMap::new();
        models.insert(
            "gpt-4o".to_string(),
            ModelInfo {
                id: "gpt-4o".to_string(),
                name: "GPT-4o".to_string(),
                description: None,
                reasoning: true,
                tool_call: true,
                temperature: true,
                limit: Some(ModelLimits {
                    context: 128_000,
                    output: 16_384,
                }),
                cost: Some(ModelCost {
                    input: 2.5,
                    output: 10.0,
                    cache_read: 1.25,
                }),
            },
        );
        models.insert(
            "gpt-4o-mini".to_string(),
            ModelInfo {
                id: "gpt-4o-mini".to_string(),
                name: "GPT-4o mini".to_string(),
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

    fn flow_at_model() -> ProviderConnectFlow {
        let catalog = catalog_with_models();
        let mut flow = ProviderConnectFlow::new(catalog, vec![]);
        // ModeSelect → Provider; builtin openai seç → Key → (yeni key) → Model.
        let _ = super::super::handle_connect_input(&mut flow, &press(KeyCode::Enter));
        let _ = super::super::handle_connect_input(&mut flow, &press(KeyCode::Enter));
        assert_eq!(flow.step, ConnectStep::Key);
        let _ = super::super::key_input::handle_key_step_input(&mut flow, &press(KeyCode::Enter));
        for c in "sk-x".chars() {
            let _ =
                super::super::key_input::handle_key_step_input(&mut flow, &press(KeyCode::Char(c)));
        }
        let _ = super::super::key_input::handle_key_step_input(&mut flow, &press(KeyCode::Enter));
        assert_eq!(flow.step, ConnectStep::Model);
        flow
    }

    #[test]
    fn badge_includes_reasoning_context_and_cost() {
        let m = ModelInfo {
            id: "gpt-4o".to_string(),
            name: "GPT-4o".to_string(),
            description: None,
            reasoning: true,
            tool_call: true,
            temperature: true,
            limit: Some(ModelLimits {
                context: 128_000,
                output: 0,
            }),
            cost: Some(ModelCost {
                input: 2.5,
                output: 0.0,
                cache_read: 0.0,
            }),
        };
        let badge = model_badge(&m);
        assert!(badge.contains("[R]"), "badge: {badge}");
        assert!(badge.contains("128k"), "badge: {badge}");
        assert!(badge.contains("$2.5/M"), "badge: {badge}");
    }

    #[test]
    fn badge_omits_unset_fields() {
        let m = ModelInfo {
            id: "gpt-4o-mini".to_string(),
            name: "GPT-4o mini".to_string(),
            description: None,
            reasoning: false,
            tool_call: true,
            temperature: true,
            limit: None,
            cost: None,
        };
        assert_eq!(model_badge(&m), "");
    }

    #[test]
    fn context_formatting() {
        assert_eq!(format_context(128_000), "128k");
        assert_eq!(format_context(2_000_000), "2M");
        assert_eq!(format_context(999), "999");
        assert_eq!(format_context(0), "0");
    }

    #[test]
    fn query_filters_models() {
        let mut flow = flow_at_model();
        assert_eq!(flow.selected_provider.as_ref().unwrap().models.len(), 2);
        flow.picker.set_query("mini");
        let filtered = filtered_models(&flow);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].id, "gpt-4o-mini");
    }

    #[test]
    fn enter_selects_model_and_advances_to_apply() {
        let mut flow = flow_at_model();
        let out = handle_model_step_input(&mut flow, &press(KeyCode::Enter));
        assert_eq!(out, ConnectOutcome::PickModel("gpt-4o".to_string()));
        assert_eq!(flow.step, ConnectStep::Apply);
        assert_eq!(flow.selected_model.as_deref(), Some("gpt-4o"));
    }

    #[test]
    fn enter_on_manual_row_opens_manual_mode() {
        let mut flow = flow_at_model();
        // Arama modundan ilk Down ilk satıra iner; 2 model + 1 manuel satır.
        // 3 Down → seçim manuel satırda (index 2 == filtered.len()).
        for _ in 0..3 {
            let _ = handle_model_step_input(&mut flow, &press(KeyCode::Down));
        }
        let out = handle_model_step_input(&mut flow, &press(KeyCode::Enter));
        assert_eq!(out, ConnectOutcome::Nothing);
        assert!(flow.model_manual_mode);
        assert_eq!(flow.step, ConnectStep::Model);
    }

    #[test]
    fn manual_id_confirmed_advances_to_apply() {
        let mut flow = flow_at_model();
        // Boş liste durumu: models'u boşalt, picker seçimi 0 = manuel satır.
        flow.selected_provider.as_mut().unwrap().models.clear();
        let _ = handle_model_step_input(&mut flow, &press(KeyCode::Enter));
        assert!(flow.model_manual_mode);
        for c in "my-model".chars() {
            let _ = handle_model_step_input(&mut flow, &press(KeyCode::Char(c)));
        }
        let out = handle_model_step_input(&mut flow, &press(KeyCode::Enter));
        assert_eq!(out, ConnectOutcome::PickModel("my-model".to_string()));
        assert_eq!(flow.step, ConnectStep::Apply);
        assert_eq!(flow.selected_model.as_deref(), Some("my-model"));
    }

    #[test]
    fn manual_empty_id_is_ignored() {
        let mut flow = flow_at_model();
        flow.selected_provider.as_mut().unwrap().models.clear();
        let _ = handle_model_step_input(&mut flow, &press(KeyCode::Enter));
        let out = handle_model_step_input(&mut flow, &press(KeyCode::Enter));
        assert_eq!(out, ConnectOutcome::Nothing);
        assert_eq!(flow.step, ConnectStep::Model);
        assert!(flow.model_manual_mode, "boş girişte modda kalır");
    }

    #[test]
    fn manual_esc_returns_to_list() {
        let mut flow = flow_at_model();
        flow.selected_provider.as_mut().unwrap().models.clear();
        let _ = handle_model_step_input(&mut flow, &press(KeyCode::Enter));
        let out = handle_model_step_input(&mut flow, &press(KeyCode::Esc));
        assert_eq!(out, ConnectOutcome::Nothing);
        assert!(!flow.model_manual_mode);
        assert_eq!(flow.step, ConnectStep::Model);
    }

    #[test]
    fn esc_from_model_goes_back_to_key() {
        let mut flow = flow_at_model();
        let out = handle_model_step_input(&mut flow, &press(KeyCode::Esc));
        assert_eq!(out, ConnectOutcome::Back);
        assert_eq!(flow.step, ConnectStep::Key);
    }

    #[test]
    fn submit_query_without_match_opens_manual_with_query() {
        let mut flow = flow_at_model();
        flow.picker.set_query("yok-boyle-bir-model");
        let out = handle_model_step_input(&mut flow, &press(KeyCode::Enter));
        assert_eq!(out, ConnectOutcome::Nothing);
        assert!(flow.model_manual_mode);
        assert_eq!(flow.model_editor.text(), "yok-boyle-bir-model");
    }

    #[test]
    fn render_without_models_does_not_panic_and_shows_hint() {
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect;
        let mut flow = flow_at_model();
        flow.selected_provider.as_mut().unwrap().models.clear();
        flow.models_fetch_state = ModelFetchState::Failed("bağlantı yok".to_string());
        let mut buf = Buffer::empty(Rect::new(0, 0, 80, 12));
        let theme = crate::theme::Theme::current();
        render_model_step(&mut buf, Rect::new(0, 0, 80, 12), 2, 76, &theme, &mut flow);
        let text: String = buf.content.iter().map(|c| c.symbol()).collect();
        assert!(text.contains("manuel"), "manuel giriş ipucu görünmeli");
    }
}

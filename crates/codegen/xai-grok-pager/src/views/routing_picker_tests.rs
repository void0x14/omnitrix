use std::fs;

use xai_grok_shell::util::routing_catalog::ModeFamily;

use super::modal::{PaletteCommand, default_palette_entries};
use super::routing_picker::RoutingPicker;

#[test]
fn command_palette_exposes_routing_picker_command() {
    let entries = default_palette_entries(true, crate::app::ScreenMode::Fullscreen);
    assert!(entries.iter().any(|entry| matches!(&entry.command, PaletteCommand::SlashCommand(command) if command == "/routing")));
}

#[test]
fn picker_fuzzy_search_and_family_filter_narrow_catalog_modes() {
    let dir = tempfile::tempdir().expect("gecici dizin");
    let mut picker = RoutingPicker::new(dir.path());
    picker.set_query("rnr");
    assert!(picker.filtered_modes().iter().any(|mode| mode.id == "rr"));

    picker.set_query("");
    picker.set_family(Some(ModeFamily::Privacy));
    assert!(!picker.filtered_modes().is_empty());
    assert!(
        picker
            .filtered_modes()
            .iter()
            .all(|mode| mode.family == ModeFamily::Privacy)
    );
}

#[test]
fn picker_selection_exposes_blurb_and_persists_via_routing_path() {
    let dir = tempfile::tempdir().expect("gecici dizin");
    let mut picker = RoutingPicker::new(dir.path());
    picker.set_query("rr");
    let selected = picker.selected_mode().expect("secili mod");
    assert!(!selected.blurb.is_empty());
    picker.apply_selected().expect("secim yazilmali");

    let written = fs::read_to_string(dir.path().join("config/routing.toml"))
        .expect("routing config yazilmali");
    assert!(written.contains("strategy = \"rr\""));
}

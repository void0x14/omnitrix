use std::fs;

use crate::app::cli::{RoutingArgs, RoutingCommand};
use crate::routing_cmd::{RoutingCommandOutput, run_at};

#[test]
fn list_and_explain_are_backed_by_the_catalog() {
    let dir = tempfile::tempdir().expect("gecici dizin");
    let list = run_at(
        RoutingArgs {
            command: RoutingCommand::List,
        },
        dir.path(),
    )
    .expect("liste calismali");
    assert!(matches!(list, RoutingCommandOutput::List(ref modes) if !modes.is_empty()));

    let help = run_at(
        RoutingArgs {
            command: RoutingCommand::Explain { id: "rr".into() },
        },
        dir.path(),
    )
    .expect("bilinen mod aciklanmali");
    assert!(
        matches!(help, RoutingCommandOutput::Explain(ref mode) if mode.id == "rr" && !mode.long_help.is_empty())
    );
}

#[test]
fn unknown_set_does_not_mutate_routing_config() {
    let dir = tempfile::tempdir().expect("gecici dizin");
    let config_dir = dir.path().join("config");
    fs::create_dir_all(&config_dir).expect("config dizini");
    let path = config_dir.join("routing.toml");
    let original = "strategy = \"rr\"\n";
    fs::write(&path, original).expect("ornek config");

    let err = run_at(
        RoutingArgs {
            command: RoutingCommand::Set {
                id: "missing-mode".into(),
            },
        },
        dir.path(),
    )
    .expect_err("bilinmeyen mod hata vermeli");
    assert!(err.to_string().contains("missing-mode"));
    assert_eq!(fs::read_to_string(path).expect("config okunur"), original);
}

#[test]
fn set_persists_canonical_catalog_id_for_show() {
    let dir = tempfile::tempdir().expect("gecici dizin");
    let result = run_at(
        RoutingArgs {
            command: RoutingCommand::Set { id: "rr".into() },
        },
        dir.path(),
    )
    .expect("bilinen mod yazilmali");
    assert!(matches!(result, RoutingCommandOutput::Set(ref id) if id == "rr"));
    let shown = run_at(
        RoutingArgs {
            command: RoutingCommand::Show,
        },
        dir.path(),
    )
    .expect("secilen mod okunmali");
    assert!(matches!(shown, RoutingCommandOutput::Show(ref mode) if mode.id == "rr"));
}

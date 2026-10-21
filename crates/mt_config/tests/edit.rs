use mt_config::edit::*;
use mt_config::{SplitKind, WorkspaceDef, WorkspacePane, WorkspaceTab};
use proptest::prelude::*;

fn leaf(cwd: Option<&str>, cmd: &[&str]) -> WorkspacePane {
    WorkspacePane {
        cwd: cwd.map(Into::into),
        command: cmd.iter().map(|s| s.to_string()).collect(),
        ..Default::default()
    }
}

fn shop() -> WorkspaceDef {
    WorkspaceDef {
        name: "shop".into(),
        tabs: vec![WorkspaceTab {
            title: Some("dev".into()),
            cwd: None,
            command: vec![],
            split: Some(SplitKind::Horizontal),
            percent: Some(60),
            panes: vec![
                leaf(Some("~/api"), &["nvim"]),
                WorkspacePane {
                    split: Some(SplitKind::Vertical),
                    panes: vec![leaf(None, &["cargo", "watch"]), leaf(None, &["htop"])],
                    ..Default::default()
                },
            ],
        }],
    }
}

const FILE: &str = "# my config\nfont_size = 14 # keep\n\n[[workspaces]]\nname = \"keep\"\ntabs = [ { title = \"x\" } ] # inline\n\n[tab_bar]\nshow = true\n";

#[test]
fn patch_keeps_untouched_bytes_and_round_trips() {
    let mut ws = read_workspaces(FILE).unwrap();
    ws.push(shop());
    let out = patch(FILE, &ws, "shop").unwrap();
    assert_eq!(read_workspaces(&out).unwrap(), ws);
    assert!(out.contains("# my config\nfont_size = 14 # keep"));
    assert!(out.contains("[tab_bar]\nshow = true"));
    assert!(out.contains("startup_workspace = \"shop\""));
    assert!(out.contains("[[workspaces.tabs.panes]]"));
}

#[test]
fn unchanged_input_is_identical() {
    let ws = read_workspaces(FILE).unwrap();
    assert_eq!(patch(FILE, &ws, "default").unwrap(), FILE);
}

#[test]
fn broken_file_is_refused() {
    assert!(matches!(
        patch("a = [", &[], "default"),
        Err(EditError::Unparsable(_))
    ));
}

#[test]
fn save_refuses_stale_writes_bak_and_marks_self_write() {
    let dir = std::env::temp_dir().join(format!("mt_edit_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("microterm.toml");
    std::fs::write(&p, FILE).unwrap();
    let h = content_hash(FILE);
    assert!(matches!(
        save(&p, h ^ 1, &[shop()], "shop"),
        Err(EditError::Stale)
    ));
    let out = save(&p, h, &[shop()], "shop").unwrap();
    assert_eq!(std::fs::read_to_string(&p).unwrap(), out);
    assert_eq!(
        std::fs::read_to_string(dir.join("microterm.toml.bak")).unwrap(),
        FILE
    );
    assert!(is_self_write(&out));
    assert!(!is_self_write(FILE));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn preview_marks_changes() {
    let d = preview("a\nb\n", "a\nc\n");
    assert_eq!(d[1].0, DiffTag::Removed);
    assert_eq!(d[2].0, DiffTag::Added);
}

fn arb_pane(depth: u32) -> BoxedStrategy<WorkspacePane> {
    let l = (
        proptest::option::of("[a-z/~]{1,6}"),
        proptest::collection::vec("[a-z]{1,5}", 0..3),
    )
        .prop_map(|(cwd, command)| WorkspacePane {
            cwd,
            command,
            ..Default::default()
        });
    if depth == 0 {
        return l.boxed();
    }
    prop_oneof![
        l,
        (
            any::<bool>(),
            proptest::option::of(1u8..100),
            arb_pane(depth - 1),
            arb_pane(depth - 1)
        )
            .prop_map(|(h, percent, a, b)| WorkspacePane {
                split: Some(if h {
                    SplitKind::Horizontal
                } else {
                    SplitKind::Vertical
                }),
                percent,
                panes: vec![a, b],
                ..Default::default()
            })
    ]
    .boxed()
}

fn arb_ws() -> impl Strategy<Value = Vec<WorkspaceDef>> {
    proptest::collection::vec(
        (proptest::collection::vec(arb_pane(3), 1..3), "[a-z]{1,4}").prop_map(|(ps, n)| {
            WorkspaceDef {
                name: n,
                tabs: ps
                    .into_iter()
                    .map(|p| WorkspaceTab {
                        title: None,
                        cwd: None,
                        command: if p.split.is_none() { p.command } else { vec![] },
                        split: p.split,
                        percent: p.percent,
                        panes: p.panes,
                    })
                    .collect(),
            }
        }),
        0..4,
    )
}

proptest! {
    #[test]
    fn random_trees_round_trip(ws in arb_ws()) {
        let out = patch(FILE, &ws, "default").unwrap();
        prop_assert_eq!(read_workspaces(&out).unwrap(), ws);
        prop_assert!(out.contains("# my config"));
    }
}

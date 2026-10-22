use super::*;

const FILE: &str = r#"# my snippets
[[snippets]]
id = "k8s-logs"                  # keep me
title = "Tail pod logs"
body = '''kubectl logs -f {{ input "Pod" }}'''
folder = "k8s"
tags = ["kubectl", "logs"]
run = true

# second
[[snippets]]
title = "Tail pod logs"
body = "echo hi"

[[snippets]]
title = "Plain"
body = '''
line one
line two'''
"#;

fn snip(id: &str, title: &str, body: &str) -> Snippet {
    Snippet {
        id: id.into(),
        explicit_id: true,
        title: title.into(),
        body: body.into(),
        ..Default::default()
    }
}

#[test]
fn loads_fields_and_derives_ids_in_file_order() {
    let l = load_str(FILE).unwrap();
    assert!(l.diagnostics.is_empty(), "{:?}", l.diagnostics);
    let ids: Vec<_> = l.set.snippets.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids, ["k8s-logs", "tail-pod-logs", "plain"]);
    let a = &l.set.snippets[0];
    assert!(a.explicit_id && a.run);
    assert_eq!((a.folder.as_str(), a.tags.len()), ("k8s", 2));
    assert!(!l.set.snippets[1].explicit_id);
    // A leading newline after `'''` is trimmed by TOML; bodies keep inner newlines.
    assert_eq!(l.set.snippets[2].body, "line one\nline two");
}

#[test]
fn derived_ids_avoid_later_explicit_ids_and_each_other() {
    let l = load_str(
        "[[snippets]]\ntitle = \"A b\"\nbody = \"x\"\n[[snippets]]\ntitle = \"A b\"\nbody = \"y\"\n\
         [[snippets]]\nid = \"a-b\"\ntitle = \"z\"\nbody = \"z\"\n",
    )
    .unwrap();
    let ids: Vec<_> = l.set.snippets.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids, ["a-b-2", "a-b-3", "a-b"]);
}

#[test]
fn duplicate_and_invalid_explicit_ids_skip_that_entry_only() {
    let l = load_str(
        "[[snippets]]\nid = \"a\"\ntitle = \"1\"\nbody = \"x\"\n\
         [[snippets]]\nid = \"a\"\ntitle = \"2\"\nbody = \"y\"\n\
         [[snippets]]\nid = \"Bad Id\"\ntitle = \"3\"\nbody = \"z\"\n\
         [[snippets]]\ntitle = \"4\"\nbody = \"w\"\n",
    )
    .unwrap();
    assert_eq!(l.set.snippets.len(), 2);
    assert_eq!(l.set.snippets[0].title, "1");
    assert_eq!(l.set.snippets[1].title, "4");
    let errs: Vec<_> = l
        .diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    assert_eq!(errs.len(), 2, "{errs:?}");
    assert!(errs[0].message.contains("duplicate") && errs[0].path == "snippets[1].id");
    assert!(errs[1].message.contains("invalid id"));
    assert_eq!(errs[0].line, 5);
}

#[test]
fn type_and_requirement_errors_are_fatal_and_path_addressed() {
    for (src, path, needle) in [
        (
            "[[snippets]]\ntitle = 3\nbody = \"x\"\n",
            "snippets[0].title",
            "string",
        ),
        (
            "[[snippets]]\ntitle = \"t\"\n",
            "snippets[0].body",
            "required",
        ),
        (
            "[[snippets]]\nbody = \"b\"\n",
            "snippets[0].title",
            "required",
        ),
        (
            "[[snippets]]\ntitle = \"t\"\nbody = \"\"\n",
            "snippets[0].body",
            "empty",
        ),
        (
            "[[snippets]]\ntitle = \"a\\nb\"\nbody = \"x\"\n",
            "snippets[0].title",
            "single line",
        ),
        (
            "[[snippets]]\ntitle = \"t\"\nbody = \"b\"\nrun = \"yes\"\n",
            "snippets[0].run",
            "true or false",
        ),
        (
            "[[snippets]]\ntitle = \"t\"\nbody = \"b\"\ntags = [1]\n",
            "snippets[0].tags",
            "strings",
        ),
    ] {
        let e = load_str(src).unwrap_err();
        assert_eq!(e[0].path, path, "{src}");
        assert!(e[0].message.contains(needle), "{e:?}");
        assert!(e[0].line >= 1);
    }
    let e = load_str("[[snippets]\n").unwrap_err();
    assert_eq!(e[0].path, "");
}

#[test]
fn limits() {
    let big = "x".repeat(MAX_BODY_BYTES + 1);
    let e = load_str(&format!("[[snippets]]\ntitle = \"t\"\nbody = \"{big}\"\n")).unwrap_err();
    assert!(e[0].message.contains("limit"), "{e:?}");
    let ok = "x".repeat(MAX_BODY_BYTES);
    assert!(load_str(&format!("[[snippets]]\ntitle = \"t\"\nbody = \"{ok}\"\n")).is_ok());
    let many = "[[snippets]]\ntitle = \"t\"\nbody = \"b\"\n".repeat(MAX_SNIPPETS + 1);
    assert!(load_str(&many).unwrap_err()[0].message.contains("at most"));
    let exactly = "[[snippets]]\ntitle = \"t\"\nbody = \"b\"\n".repeat(MAX_SNIPPETS);
    assert_eq!(load_str(&exactly).unwrap().set.snippets.len(), MAX_SNIPPETS);
}

#[test]
fn crlf_in_bodies_is_normalised() {
    let l = load_str("[[snippets]]\ntitle = \"t\"\nbody = \"a\\r\\nb\"\n").unwrap();
    assert_eq!(l.set.snippets[0].body, "a\nb");
}

#[test]
fn unknown_keys_warn_with_suggestions() {
    let l =
        load_str("[[snippets]]\ntitel = \"t\"\ntitle = \"t\"\nbody = \"b\"\n[snippet]\n").unwrap();
    let w: Vec<_> = l.diagnostics.iter().map(|d| d.message.as_str()).collect();
    assert!(
        w.iter()
            .any(|m| m.contains("\"titel\"") && m.contains("did you mean \"title\"")),
        "{w:?}"
    );
    assert!(
        w.iter()
            .any(|m| m.contains("\"snippet\"") && m.contains("\"snippets\"")),
        "{w:?}"
    );
    assert!(l
        .diagnostics
        .iter()
        .all(|d| d.severity == Severity::Warning));
    let first = &l.diagnostics[0];
    assert_eq!((first.line, first.col), (2, 1));
    assert!(first
        .render("snippets.toml")
        .starts_with("snippets.toml:2:1 — snippets[0].titel"));
}

#[test]
fn missing_file_is_an_empty_set() {
    let l = load(Path::new("/nonexistent/dir/snippets.toml")).unwrap();
    assert!(l.set.snippets.is_empty());
}

#[test]
fn upsert_update_keeps_everything_else_byte_identical() {
    let before = load_str(FILE).unwrap().set;
    let mut s = before.get("k8s-logs").unwrap().clone();
    s.title = "Follow pod logs".into();
    let out = upsert(FILE, &s).unwrap();
    assert_eq!(
        out,
        FILE.replace(
            "title = \"Tail pod logs\"\nbody = '''kubectl",
            "title = \"Follow pod logs\"\nbody = '''kubectl"
        )
    );
    assert!(out.contains("# keep me") && out.contains("# second") && out.contains("# my snippets"));
    let after = load_str(&out).unwrap().set;
    assert_eq!(after.get("k8s-logs").unwrap().title, "Follow pod logs");
    assert_eq!(after.snippets[1..], before.snippets[1..]);
}

#[test]
fn upsert_derived_id_entry_gains_an_explicit_id() {
    let l = load_str(FILE).unwrap().set;
    let mut s = l.get("tail-pod-logs").unwrap().clone();
    s.body = "echo changed".into();
    let out = upsert(FILE, &s).unwrap();
    let after = load_str(&out).unwrap().set;
    let got = after.get("tail-pod-logs").unwrap();
    assert!(got.explicit_id);
    assert_eq!(got.body, "echo changed");
    assert_eq!(after.snippets.len(), 3);
}

#[test]
fn upsert_appends_new_entries_with_literal_bodies() {
    let mut s = snip("new-one", "New one", "echo 'it''s' {{ uuid }}\nsecond line");
    s.folder = "misc".into();
    s.run = true;
    s.tags = vec!["a".into(), "b".into()];
    let out = upsert(FILE, &s).unwrap();
    assert!(out.starts_with(FILE));
    assert!(
        out.contains("body = '''\necho 'it''s' {{ uuid }}\nsecond line'''"),
        "{out}"
    );
    let got = load_str(&out).unwrap().set;
    let mut want = s.clone();
    want.explicit_id = true;
    assert_eq!(got.get("new-one"), Some(&want));
    // From nothing at all (the panel creates the file on first save).
    let fresh = upsert("", &snip("a", "A", "x")).unwrap();
    assert_eq!(load_str(&fresh).unwrap().set.snippets.len(), 1);
}

#[test]
fn bodies_with_triple_quotes_or_control_chars_fall_back_to_basic_strings() {
    for body in [
        "a ''' b",
        "esc \u{1b}[0m",
        "tab\there",
        "ends with '",
        "back\\slash \"q\"",
    ] {
        let out = upsert("", &snip("a", "A", body)).unwrap();
        assert_eq!(
            load_str(&out).unwrap().set.get("a").unwrap().body,
            body,
            "{out}"
        );
    }
    let out = upsert("", &snip("a", "A", "x ''' y")).unwrap();
    assert!(!out.contains("body = '''"), "{out}");
}

#[test]
fn optional_keys_are_dropped_when_cleared() {
    let l = load_str(FILE).unwrap().set;
    let mut s = l.get("k8s-logs").unwrap().clone();
    s.run = false;
    s.tags.clear();
    s.folder.clear();
    let out = upsert(FILE, &s).unwrap();
    let t = entry_text(&out, "k8s-logs").unwrap();
    assert!(
        !t.contains("run =") && !t.contains("tags") && !t.contains("folder"),
        "{t}"
    );
    assert!(t.contains("# keep me"));
}

#[test]
fn remove_and_reorder() {
    let out = remove(FILE, "tail-pod-logs").unwrap();
    let ids: Vec<_> = load_str(&out)
        .unwrap()
        .set
        .snippets
        .iter()
        .map(|s| s.id.clone())
        .collect();
    assert_eq!(ids, ["k8s-logs", "plain"]);
    assert!(remove(FILE, "nope").is_err());

    let out = reorder(FILE, "plain", Some("k8s-logs")).unwrap();
    let ids: Vec<_> = load_str(&out)
        .unwrap()
        .set
        .snippets
        .iter()
        .map(|s| s.id.clone())
        .collect();
    assert_eq!(ids, ["plain", "k8s-logs", "tail-pod-logs"]);
    let out = reorder(FILE, "k8s-logs", None).unwrap();
    let ids: Vec<_> = load_str(&out)
        .unwrap()
        .set
        .snippets
        .iter()
        .map(|s| s.id.clone())
        .collect();
    assert_eq!(ids, ["tail-pod-logs", "plain", "k8s-logs"]);
    let out = reorder(FILE, "k8s-logs", Some("plain")).unwrap();
    let ids: Vec<_> = load_str(&out)
        .unwrap()
        .set
        .snippets
        .iter()
        .map(|s| s.id.clone())
        .collect();
    assert_eq!(ids, ["tail-pod-logs", "k8s-logs", "plain"]);
}

#[test]
fn never_writes_a_file_that_does_not_parse() {
    for bad in ["[[snippets]\n", "[[snippets]]\ntitle = 1\nbody = \"x\"\n"] {
        assert!(
            matches!(
                upsert(bad, &snip("a", "A", "x")),
                Err(EditError::Unparsable(_))
            ),
            "{bad}"
        );
        assert!(remove(bad, "a").is_err());
    }
}

fn tmpdir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("mt_snip_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn save_writes_atomically_with_bak_and_marks_self_write() {
    let dir = tmpdir("save");
    let path = dir.join("snippets.toml");
    std::fs::write(&path, FILE).unwrap();
    let base = Base::of(FILE, Some("k8s-logs"));
    let mut s = load_str(FILE).unwrap().set.get("k8s-logs").unwrap().clone();
    s.description = "Follow".into();
    let out = save_snippet(&path, &base, &s).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), out);
    assert_eq!(
        std::fs::read_to_string(dir.join("snippets.toml.bak")).unwrap(),
        FILE
    );
    assert!(mt_config::atomic::is_self_write_at(&path, &out));
    assert!(!mt_config::atomic::is_self_write_at(&path, FILE));
    assert!(!dir.join("snippets.toml.tmp").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn save_creates_a_missing_file() {
    let dir = tmpdir("create");
    let path = dir.join("snippets.toml");
    let base = Base::of("", None);
    save_snippet(&path, &base, &snip("a", "A", "echo")).unwrap();
    assert_eq!(load(&path).unwrap().set.snippets.len(), 1);
    assert!(!dir.join("snippets.toml.bak").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn staleness_is_per_entry() {
    let dir = tmpdir("stale");
    let path = dir.join("snippets.toml");
    std::fs::write(&path, FILE).unwrap();
    let base = Base::of(FILE, Some("k8s-logs"));
    let mut edit = load_str(FILE).unwrap().set.get("k8s-logs").unwrap().clone();
    edit.title = "Mine".into();

    // Another entry changed on disk: the edit is re-applied to the fresh file.
    let other = FILE.replace("body = \"echo hi\"", "body = \"echo changed elsewhere\"");
    std::fs::write(&path, &other).unwrap();
    let out = save_snippet(&path, &base, &edit).unwrap();
    assert!(out.contains("echo changed elsewhere") && out.contains("title = \"Mine\""));

    // The edited entry itself changed on disk: refuse.
    let theirs = FILE.replace("run = true", "run = false");
    std::fs::write(&path, &theirs).unwrap();
    assert!(matches!(
        save_snippet(&path, &base, &edit),
        Err(EditError::Stale)
    ));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), theirs);

    // A new snippet whose id appeared meanwhile: refuse.
    let base_new = Base::of(FILE, None);
    let clash = snip("k8s-logs", "x", "y");
    std::fs::write(&path, &other).unwrap();
    assert!(matches!(
        save_snippet(&path, &base_new, &clash),
        Err(EditError::Stale)
    ));
    let _ = std::fs::remove_dir_all(&dir);
}

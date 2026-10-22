mod common;

use common::*;
use mt_snippets::{expand, sanitize, Fact, Mode, Secret, Session, Template, Val};

fn ok(src: &str) -> String {
    run(src).unwrap_or_else(|e| panic!("{src}: {e}"))
}

fn fails(src: &str) -> String {
    run(src).expect_err(src).reason
}

#[test]
fn generators_have_the_right_shape() {
    let u = ok("{{ uuid }}");
    assert_eq!(u.len(), 36);
    assert_eq!(&u[14..15], "4");
    assert!(matches!(&u[19..20], "8" | "9" | "a" | "b"));
    assert_eq!(u, u.to_lowercase());
    let u7 = ok("{{ uuid v7 }}");
    assert_eq!(&u7[14..15], "7");
    // v7 starts with the fixed clock's milliseconds.
    let ms = common::fixed_now().timestamp_millis() as u64;
    assert_eq!(&u7[..8], &format!("{:012x}", ms)[..8]);
    let l = ok("{{ ulid }}");
    assert_eq!(l.len(), 26);
    assert!(
        l.chars()
            .all(|c| "0123456789ABCDEFGHJKMNPQRSTVWXYZ".contains(c)),
        "{l}"
    );
    assert_eq!(ok("{{ nanoid }}").len(), 21);
    let n = ok("{{ nanoid len=8 }}");
    assert_eq!(n.len(), 8);
    assert!(n
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'));
    assert_eq!(ok("{{ hex 4 }}").len(), 8);
    assert!(ok("{{ hex 4 }}").chars().all(|c| c.is_ascii_hexdigit()));
    assert_eq!(ok("{{ base64_random 3 }}").len(), 4);
    assert_eq!(ok("{{ hex 0 }}"), "");
}

#[test]
fn same_seed_same_output() {
    assert_eq!(
        ok("{{ uuid }}{{ password }}"),
        ok("{{ uuid }}{{ password }}")
    );
}

#[test]
fn random_ranges() {
    for _ in 0..50 {
        let n: i64 = ok("{{ random 1 6 }}").parse().unwrap();
        assert!((1..=6).contains(&n));
    }
    assert_eq!(ok("{{ random 5 5 }}"), "5");
    let n: i64 = ok("{{ random -3 -1 }}").parse().unwrap();
    assert!((-3..=-1).contains(&n));
    assert!(fails("{{ random 3 1 }}").contains("greater"));
    ok(&format!("{{{{ random {} {} }}}}", i64::MIN, i64::MAX));
}

#[test]
fn random_string_and_password() {
    assert!(ok("{{ random_string 12 charset=hex }}")
        .chars()
        .all(|c| c.is_ascii_hexdigit()));
    assert!(ok("{{ random_string 30 charset=digits }}")
        .chars()
        .all(|c| c.is_ascii_digit()));
    assert!(ok("{{ random_string 30 charset=upper }}")
        .chars()
        .all(|c| c.is_ascii_uppercase()));
    assert!(ok("{{ random_string 30 charset=ab }}")
        .chars()
        .all(|c| c == 'a' || c == 'b'));
    assert_eq!(ok("{{ random_string 9 }}").len(), 9);
    assert!(fails("{{ random_string 5 charset=\"\" }}").contains("empty"));
    assert!(fails("{{ random_string 5000 }}").contains("4096"));
    for _ in 0..20 {
        let p = ok("{{ password }}");
        assert_eq!(p.chars().count(), 20);
        assert!(p.chars().any(|c| c.is_ascii_lowercase()));
        assert!(p.chars().any(|c| c.is_ascii_uppercase()));
        assert!(p.chars().any(|c| c.is_ascii_digit()));
        assert!(p.chars().any(|c| !c.is_ascii_alphanumeric()));
        let q = ok("{{ password len=8 symbols=false }}");
        assert!(q.chars().all(|c| c.is_ascii_alphanumeric()) && q.len() == 8);
    }
    assert!(fails("{{ password len=3 }}").contains("at least"));
}

#[test]
fn pick_picks_an_argument() {
    for _ in 0..20 {
        assert!(["a", "b", "c"].contains(&ok("{{ pick a b c }}").as_str()));
    }
    assert_eq!(ok("{{ pick \"only one\" }}"), "only one");
}

#[test]
fn lorem() {
    assert_eq!(
        ok("{{ lorem }}"),
        "lorem ipsum dolor sit amet consectetur adipiscing elit"
    );
    assert_eq!(ok("{{ lorem words=2 }}"), "lorem ipsum");
    assert_eq!(ok("{{ lorem words=0 }}"), "");
}

#[test]
fn counters_advance_only_on_commit() {
    let mut ctx = Mock::new();
    let t = Template::compile("{{ counter \"n\" start=10 step=5 }}-{{ counter \"n\" }}");
    let mut s = Session::new(&ctx);
    let a = expand(&t, &mut ctx, &mut s, Mode::Insert).unwrap().text;
    assert_eq!(a, "10-15", "the second call site continues from the first");
    // A preview does not touch the stored counter; the same session repeats its values.
    assert_eq!(
        expand(&t, &mut ctx, &mut s, Mode::Placeholder)
            .unwrap()
            .text,
        "10-15"
    );
    assert_eq!(ctx.counters.get("n"), None);
    s.commit(&mut ctx);
    assert_eq!(
        ctx.counters.get("n"),
        Some(&16),
        "next value after the last call site's step"
    );
    let mut s2 = Session::new(&ctx);
    assert_eq!(
        expand(&t, &mut ctx, &mut s2, Mode::Insert).unwrap().text,
        "16-21"
    );
}

#[test]
fn time_functions_use_one_clock() {
    assert_eq!(ok("{{ now }}"), "2026-03-04 05:06:07");
    assert_eq!(ok("{{ now utc=true }}"), "2026-03-04 03:06:07");
    assert_eq!(ok("{{ now \"%H-%M\" }}"), "05-06");
    assert_eq!(ok("{{ date }} {{ time }}"), "2026-03-04 05:06:07");
    assert_eq!(ok("{{ date utc=true }}"), "2026-03-04");
    let ts = common::fixed_now().timestamp();
    assert_eq!(ok("{{ timestamp }}"), ts.to_string());
    assert_eq!(ok("{{ timestamp_ms }}"), (ts * 1000).to_string());
    assert_eq!(ok("{{ iso8601 }}"), "2026-03-04T05:06:07+02:00");
    assert_eq!(ok("{{ iso8601 utc=true }}"), "2026-03-04T03:06:07+00:00");
    assert_eq!(ok("{{ date_offset \"-7d\" }}"), "2026-02-25");
    assert_eq!(ok("{{ date_offset \"+1w\" }}"), "2026-03-11");
    assert_eq!(ok("{{ date_offset \"2h\" \"%H:%M\" }}"), "07:06");
    assert_eq!(ok("{{ date_offset \"90m\" \"%H:%M\" }}"), "06:36");
    assert_eq!(ok("{{ date_offset \"-30s\" \"%S\" }}"), "37");
    assert!(fails("{{ date_offset \"7y\" }}").contains("bad offset"));
    assert!(fails("{{ date_offset \"d\" }}").contains("bad offset"));
    assert!(fails("{{ now \"%Q\" }}").contains("invalid time format"));
}

#[test]
fn system_functions_read_the_context() {
    let mut c = Mock::new()
        .fact(Fact::User, "ada")
        .fact(Fact::Hostname, "box")
        .fact(Fact::Home, "/home/ada")
        .fact(Fact::Shell, "/bin/zsh")
        .fact(Fact::Tmpdir, "/tmp")
        .fact(Fact::Cpus, "8")
        .fact(Fact::LocalIp { v6: false }, "10.0.0.2")
        .fact(Fact::LocalIp { v6: true }, "fd00::2")
        .fact(Fact::Version, "1.0.0")
        .env("AWS_PROFILE", "dev");
    let t = |s: &str, c: &mut Mock| run_with(s, c).unwrap();
    assert_eq!(t("{{ user }}@{{ hostname }}", &mut c), "ada@box");
    assert_eq!(
        t("{{ home }} {{ shell }} {{ tmpdir }} {{ cpus }}", &mut c),
        "/home/ada /bin/zsh /tmp 8"
    );
    assert_eq!(
        t("{{ local_ip }} {{ local_ip v6 }} {{ local_ip v4 }}", &mut c),
        "10.0.0.2 fd00::2 10.0.0.2"
    );
    assert_eq!(t("{{ microterm_version }}", &mut c), "1.0.0");
    assert_eq!(t("{{ env \"AWS_PROFILE\" }}", &mut c), "dev");
    assert_eq!(
        t("{{ os }}/{{ arch }}", &mut c),
        format!("{}/{}", std::env::consts::OS, std::env::consts::ARCH)
    );
    assert!(run_with("{{ env \"NOPE\" }}", &mut c)
        .unwrap_err()
        .reason
        .contains("not set"));
    assert_eq!(t("{{ env \"NOPE\" default=\"x\" }}", &mut c), "x");
}

#[test]
fn pane_facts() {
    let mut c = Mock::new()
        .fact(Fact::Cwd, "/work/api")
        .fact(Fact::PaneId, "7")
        .fact(Fact::TabId, "3")
        .fact(Fact::WindowId, "1")
        .fact(Fact::TabIndex, "2")
        .fact(Fact::TabTitle, "build")
        .fact(Fact::PaneTitle, "cargo")
        .fact(Fact::Workspace, "dev")
        .fact(Fact::Domain, "local")
        .fact(Fact::Cols, "120")
        .fact(Fact::Rows, "40")
        .fact(Fact::FontFamily, "Menlo")
        .fact(Fact::FontSize, "13")
        .fact(Fact::Theme, "Nord")
        .fact(Fact::FgProcess, "vim")
        .fact(Fact::ShellPid, "99")
        .fact(Fact::LastCommand(1), "ls -l")
        .fact(Fact::LastCommand(2), "make")
        .fact(Fact::LastOutput(1), "out1")
        .fact(Fact::Selection, "sel")
        .fact(Fact::Clipboard, "clip");
    let src = "{{cwd}}|{{cwd_basename}}|{{pane_id}}|{{tab_id}}|{{window_id}}|{{tab_index}}|{{tab_title}}|{{pane_title}}|\
               {{workspace}}|{{domain}}|{{cols}}x{{rows}}|{{font_family}}|{{font_size}}|{{theme}}|{{fg_process}}|\
               {{shell_pid}}|{{last_command}}|{{last_command n=2}}|{{last_output}}|{{selection}}|{{clipboard}}";
    assert_eq!(
        run_with(src, &mut c).unwrap(),
        "/work/api|api|7|3|1|2|build|cargo|dev|local|120x40|Menlo|13|Nord|vim|99|ls -l|make|out1|sel|clip"
    );
    let e = run_with("{{ remote_host }}", &mut c).unwrap_err();
    assert!(
        e.reason.contains("unavailable") && e.tag == "remote_host",
        "{e}"
    );
    assert!(e.to_string().contains("remote_host"));
    assert_eq!(
        run_with("{{ last_output n=2 default=\"-\" }}", &mut c).unwrap(),
        "-"
    );
}

fn tmp(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("mt_git_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn git_functions_read_head_without_a_process() {
    let root = tmp("repo");
    std::fs::create_dir_all(root.join(".git")).unwrap();
    std::fs::write(root.join(".git/HEAD"), "ref: refs/heads/feature/x\n").unwrap();
    std::fs::create_dir_all(root.join("src/deep")).unwrap();
    let cwd = root.join("src/deep");
    let mk = |cwd: &std::path::Path| {
        Mock::new()
            .fact(Fact::Cwd, cwd.to_str().unwrap())
            .fact(Fact::IsRemote, "false")
    };
    let mut c = mk(&cwd);
    assert_eq!(run_with("{{ git_branch }}", &mut c).unwrap(), "feature/x");
    assert_eq!(
        run_with("{{ git_root }}", &mut c).unwrap(),
        root.to_str().unwrap()
    );

    // Detached HEAD gives the short SHA.
    std::fs::write(
        root.join(".git/HEAD"),
        "0123456789abcdef0123456789abcdef01234567\n",
    )
    .unwrap();
    assert_eq!(run_with("{{ git_branch }}", &mut c).unwrap(), "0123456");

    // A worktree: `.git` is a file pointing at a private gitdir.
    let wt = tmp("worktree");
    let gitdir = root.join(".git/worktrees/wt");
    std::fs::create_dir_all(&gitdir).unwrap();
    std::fs::write(gitdir.join("HEAD"), "ref: refs/heads/wt-branch\n").unwrap();
    std::fs::write(wt.join(".git"), format!("gitdir: {}\n", gitdir.display())).unwrap();
    let mut c = mk(&wt);
    assert_eq!(run_with("{{ git_branch }}", &mut c).unwrap(), "wt-branch");
    assert_eq!(
        run_with("{{ git_root }}", &mut c).unwrap(),
        wt.to_str().unwrap()
    );

    // Relative gitdir.
    let wt2 = tmp("worktree_rel");
    std::fs::create_dir_all(wt2.join("meta")).unwrap();
    std::fs::write(wt2.join("meta/HEAD"), "ref: refs/heads/rel\n").unwrap();
    std::fs::write(wt2.join(".git"), "gitdir: meta\n").unwrap();
    assert_eq!(run_with("{{ git_branch }}", &mut mk(&wt2)).unwrap(), "rel");

    // Not in a repository, and remote panes.
    let plain = tmp("plain");
    let e = run_with("{{ git_branch }}", &mut mk(&plain)).unwrap_err();
    assert!(e.reason.contains("not inside a git repository"), "{e}");
    assert_eq!(
        run_with("{{ git_branch default=\"main\" }}", &mut mk(&plain)).unwrap(),
        "main"
    );
    let mut remote = mk(&cwd).fact(Fact::IsRemote, "true");
    assert!(run_with("{{ git_root }}", &mut remote)
        .unwrap_err()
        .reason
        .contains("remote"));
    for d in [root, wt, wt2, plain] {
        let _ = std::fs::remove_dir_all(d);
    }
}

/// Runs `{{ input "v" | FILTERS }}` with `v` = `input`.
fn filt(input: &str, filters: &str) -> Result<String, mt_snippets::RuntimeError> {
    let mut c = Mock::new();
    full(
        &format!("{{{{ input \"v\" | {filters} }}}}"),
        &mut c,
        &[("v", Val::plain(input))],
        Mode::Insert,
    )
    .map(|e| e.text)
}

#[test]
fn filters_golden() {
    let cases: &[(&str, &str, &str)] = &[
        ("Hello World", "upper", "HELLO WORLD"),
        ("Hello World", "lower", "hello world"),
        ("hello big WORLD", "title", "Hello Big World"),
        ("Hello big-World fooBar", "snake", "hello_big_world_foo_bar"),
        ("Hello big_World", "kebab", "hello-big-world"),
        ("hello big world", "camel", "helloBigWorld"),
        ("  Hello, World!  ", "slug", "hello-world"),
        ("  padded \n", "trim", "padded"),
        ("abcdefghij", "truncate 5", "abcd…"),
        ("abc", "truncate 5", "abc"),
        ("abcdefghij", "truncate 5 ellipsis=\"..\"", "abc.."),
        ("abcdefghij", "truncate 3 ellipsis=\"\"", "abc"),
        ("a b a", "replace \"a\" \"x\"", "x b x"),
        ("", "default \"fallback\"", "fallback"),
        ("set", "default \"fallback\"", "set"),
        ("x", "silent", ""),
        ("it's", "quote", "'it'\\''s'"),
        ("", "quote", "''"),
        (
            "a \"b\" $x `y` \\",
            "dquote",
            "\"a \\\"b\\\" \\$x \\`y\\` \\\\\"",
        ),
        ("a\"b\n\t", "json", "\"a\\\"b\\n\\t\""),
        ("a b&c/d~e", "urlencode", "a%20b%26c%2Fd~e"),
        ("hello", "base64", "aGVsbG8="),
        ("aGVsbG8=", "base64d", "hello"),
        (
            "abc",
            "sha256",
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        ),
        ("abc", "md5", "900150983cd24fb0d6963f7d28e17f72"),
        ("/a/b/c.txt", "basename", "c.txt"),
        ("/a/b/c/", "basename", "c"),
        ("/a/b/c.txt", "dirname", "/a/b"),
        ("/a", "dirname", "/"),
        ("c.txt", "dirname", "."),
        ("abc", "upper | lower | replace \"b\" \"-\"", "a-c"),
    ];
    for (input, filter, want) in cases {
        assert_eq!(filt(input, filter).unwrap(), *want, "{input:?} | {filter}");
    }
    assert!(filt("!!!", "base64d")
        .unwrap_err()
        .reason
        .contains("invalid base64"));
    assert!(filt("abc", "replace \"\" \"x\"")
        .unwrap_err()
        .reason
        .contains("empty"));
    assert_eq!(filt("!!!", "base64d default=\"bad\"").unwrap(), "bad");
}

#[test]
fn bindings_and_silent() {
    let mut c = Mock::new();
    let run = |src: &str, c: &mut Mock| {
        full(src, c, &[("v", Val::plain("x"))], Mode::Insert)
            .unwrap()
            .text
    };
    assert_eq!(
        run("{{ a := input \"v\" }}{{ $a }}{{ $a | upper }}", &mut c),
        "xxX"
    );
    assert_eq!(
        run("{{ a := input \"v\" | silent }}[{{ $a }}]", &mut c),
        "[x]"
    );
    let out = ok("{{ id := uuid }} {{ $id }}");
    let (a, b) = out.split_once(' ').unwrap();
    assert_eq!(a, b);
    let two = ok("{{ uuid }} {{ uuid }}");
    let (a, b) = two.split_once(' ').unwrap();
    assert_ne!(a, b);
}

#[test]
fn runtime_failures_name_the_tag_and_reason() {
    let e = run("{{ git_branch }}").unwrap_err();
    assert_eq!(e.tag, "git_branch");
    assert!(e.reason.contains("unavailable"));
    let e = filt("!!!", "base64d").unwrap_err();
    assert_eq!(e.tag, "base64d");
    let e = run("{{ nope }}").unwrap_err();
    assert!(e.reason.contains("template error"), "{e}");
}

#[test]
fn default_named_arg_covers_args_too() {
    assert_eq!(ok("{{ date_offset (git_branch) default=\"x\" }}"), "x");
    assert_eq!(ok("{{ now \"%Q\" default=\"never\" }}"), "never");
    assert_eq!(
        ok("{{ date_offset (git_branch default=\"1d\") }}"),
        "2026-03-05"
    );
}

#[test]
fn results_are_fresh_per_call_site_and_memoised_per_session() {
    let mut ctx = Mock::new();
    let t = Template::compile("{{ uuid }} {{ uuid }} {{ random_string 8 }}");
    let mut s = Session::new(&ctx);
    let a = expand(&t, &mut ctx, &mut s, Mode::Insert).unwrap().text;
    let parts: Vec<_> = a.split(' ').collect();
    assert_ne!(parts[0], parts[1], "two call sites, two uuids");
    // The same session returns the same text again: the preview is what gets inserted.
    assert_eq!(expand(&t, &mut ctx, &mut s, Mode::Insert).unwrap().text, a);
    // A forked session (another pane of a broadcast) generates fresh values.
    let mut f = s.fork();
    assert_ne!(expand(&t, &mut ctx, &mut f, Mode::Insert).unwrap().text, a);
    // A new session too.
    let mut s2 = Session::new(&ctx);
    assert_ne!(expand(&t, &mut ctx, &mut s2, Mode::Insert).unwrap().text, a);
}

#[test]
fn memo_follows_changed_arguments() {
    let mut ctx = Mock::new();
    let t = Template::compile("{{ random_string (input \"N\") }}");
    let mut s = Session::new(&ctx);
    s.set_prompt("N", Val::plain("4"));
    let a = expand(&t, &mut ctx, &mut s, Mode::Insert).unwrap().text;
    assert_eq!(a.len(), 4);
    s.set_prompt("N", Val::plain("6"));
    assert_eq!(
        expand(&t, &mut ctx, &mut s, Mode::Insert)
            .unwrap()
            .text
            .len(),
        6
    );
}

#[test]
fn prompts_by_mode() {
    let src = "{{ input \"Pod\" }}/{{ choice \"Env\" \"dev\" \"prod\" }}/{{ secret \"Token\" }}/{{ input \"Pod\" | upper }}";
    let vals = [
        ("Pod", Val::plain("api")),
        ("Env", Val::plain("dev")),
        ("Token", Val::secret("s3cr3t")),
    ];
    let mut c = Mock::new();
    let e = full(src, &mut c, &vals, Mode::Insert).unwrap();
    assert_eq!(e.text, "api/dev/s3cr3t/API");
    assert!(e.has_secret);
    let m = full(src, &mut c, &vals, Mode::Masked).unwrap();
    assert_eq!(m.text, "api/dev/••••/API");
    assert!(!format!("{m:?}").contains("s3cr3t"));
    let p = full(src, &mut c, &vals, Mode::Placeholder).unwrap();
    assert_eq!(p.text, "‹Pod›/‹Env›/‹secret›/‹POD›");
    let none = full(src, &mut c, &[], Mode::Masked).unwrap();
    assert_eq!(none.text, "‹Pod›/‹Env›/‹secret›/‹POD›");
    let e = full(src, &mut c, &[], Mode::Insert).unwrap_err();
    assert_eq!(e.missing.as_deref(), Some("Pod"));
    let e = full(src, &mut c, &vals[..2], Mode::Insert).unwrap_err();
    assert_eq!(e.missing.as_deref(), Some("Token"));
}

#[test]
fn input_default_fills_a_missing_value() {
    let mut c = Mock::new();
    let src = "{{ input \"Db\" default=\"dev\" }}";
    assert_eq!(full(src, &mut c, &[], Mode::Insert).unwrap().text, "dev");
    assert_eq!(
        full(src, &mut c, &[("Db", Val::plain("prod"))], Mode::Insert)
            .unwrap()
            .text,
        "prod"
    );
}

#[test]
fn secrets_never_format_their_value() {
    let s = Secret::new("hunter2");
    assert_eq!(format!("{s}"), "••••");
    assert_eq!(format!("{s:?}"), "••••");
    assert_eq!(format!("{s:#?}"), "••••");
    let v = Val::secret("hunter2");
    assert_eq!(format!("{v} {v:?}"), "•••• ••••");
    let derived = v.derive("deadbeef".into());
    assert!(derived.is_secret());
    assert_eq!(format!("{derived:?}"), "••••");
    assert_eq!(v.expose(), "hunter2");
    // Taint survives filters.
    let mut c = Mock::new();
    let t = Template::compile("{{ secret \"T\" | quote | sha256 }}");
    let mut sess = Session::new(&c);
    sess.set_prompt("T", Val::secret("hunter2"));
    let out = expand(&t, &mut c, &mut sess, Mode::Masked).unwrap();
    assert!(!out.text.contains("hunter2"));
}

#[test]
fn cursor_marks_a_position() {
    let mut c = Mock::new();
    let e = full(
        "psql {{ cursor }}-d {{ input \"v\" }}!",
        &mut c,
        &[("v", Val::plain("é"))],
        Mode::Insert,
    )
    .unwrap();
    assert_eq!(e.text, "psql -d é!");
    assert_eq!(e.cursor, Some(5));
    assert_eq!(e.cursor_tail_chars(), Some(5));
    assert_eq!(
        full("no cursor", &mut c, &[], Mode::Insert).unwrap().cursor,
        None
    );
    assert_eq!(
        full("{{ cursor }}x", &mut c, &[], Mode::Insert)
            .unwrap()
            .cursor_tail_chars(),
        Some(1)
    );
}

#[test]
fn spec_examples_parse() {
    for src in [
        "echo {{uuid}}",
        "git commit -m {{ input \"Message\" | quote }}",
        "cd {{ git_root }} && git checkout -b {{ b := input \"Branch\" | kebab }} && git push -u origin {{ $b }}",
        "curl -H \"Authorization: Bearer {{ secret \"Token\" }}\" https://api.example.com/{{ choice \"Env\" \"dev\" \"prod\" }}",
        "mkdir -p backups/{{ date }} && tar czf backups/{{ date }}/{{ cwd_basename }}-{{ timestamp }}.tgz .",
        "psql \"postgres://{{ user }}@localhost/{{ input \"Database\" default=\"dev\" }}\"{{ cursor }}",
        "kubectl -n {{ choice \"Namespace\" \"dev\" \"staging\" \"prod\" }} logs -f {{ input \"Pod\" }}",
    ] {
        let t = Template::compile(src);
        assert!(t.is_valid(), "{src}: {:?}", t.diagnostics());
    }
}

#[test]
fn git_checkout_example_expands() {
    let mut c = Mock::new()
        .fact(Fact::Cwd, "/x")
        .fact(Fact::IsRemote, "false");
    // git_root fails outside a repository; with default= the example still expands.
    let src = "cd {{ git_root default=\".\" }} && git checkout -b {{ b := input \"Branch\" | kebab }} && git push -u origin {{ $b }}";
    let e = full(
        src,
        &mut c,
        &[("Branch", Val::plain("Fix The Bug"))],
        Mode::Insert,
    )
    .unwrap();
    assert_eq!(
        e.text,
        "cd . && git checkout -b fix-the-bug && git push -u origin fix-the-bug"
    );
}

#[test]
fn sanitize_strips_nul_esc_and_normalises_newlines() {
    assert_eq!(sanitize("a\r\nb\u{1b}[31m\0c"), "a\nb[31mc");
    assert_eq!(sanitize("plain"), "plain");
}

#[test]
fn every_catalogue_example_compiles_and_every_name_is_unique() {
    use mt_snippets::funcs::CATALOGUE;
    let mut seen = std::collections::HashSet::new();
    for d in CATALOGUE {
        assert!(seen.insert(d.name), "duplicate {}", d.name);
        assert!(
            !d.desc.is_empty() && !d.example.is_empty() && !d.sig.is_empty(),
            "{}",
            d.name
        );
        let src = if d.kind == mt_snippets::funcs::Kind::Filter && !d.example.contains('{') {
            continue;
        } else {
            d.example
        };
        let t = Template::compile(src);
        assert!(t.is_valid(), "{}: {src}: {:?}", d.name, t.diagnostics());
    }
}

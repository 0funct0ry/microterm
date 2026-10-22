use mt_snippets::analyze::analyze;
use mt_snippets::template::{parse, DiagKind, Node, Term};
use mt_snippets::Template;

fn kinds(src: &str) -> Vec<DiagKind> {
    Template::compile(src)
        .diagnostics()
        .iter()
        .map(|d| d.kind)
        .collect()
}

#[test]
fn text_is_copied_verbatim_and_escapes_unescape() {
    let src = "echo }} hi \\{{ not a tag }} \\x";
    let p = parse(src);
    assert!(p.diags.is_empty(), "{:?}", p.diags);
    let [Node::Text(t, sp)] = &p.ast.nodes[..] else {
        panic!("{:?}", p.ast)
    };
    assert_eq!(t, "echo }} hi {{ not a tag }} \\x");
    assert_eq!((sp.start, sp.end), (0, src.len()));
}

#[test]
fn tags_with_and_without_whitespace_are_the_same() {
    let a = parse("{{uuid}}").ast;
    let b = parse("{{   uuid\t}}").ast;
    let (Node::Tag(x), Node::Tag(y)) = (&a.nodes[0], &b.nodes[0]) else {
        panic!()
    };
    assert_eq!(
        x.pipeline.first,
        y.pipeline
            .first
            .clone()
            .into_same_call_for(&x.pipeline.first)
    );
}

trait SameCall {
    fn into_same_call_for(self, other: &Term) -> Term;
}
impl SameCall for Term {
    // Spans differ with the whitespace; compare structure only.
    fn into_same_call_for(self, other: &Term) -> Term {
        match (self, other) {
            (Term::Call(mut a), Term::Call(b)) => {
                a.span = b.span;
                a.name_span = b.name_span;
                Term::Call(a)
            }
            (a, _) => a,
        }
    }
}

#[test]
fn spans_point_at_the_tag_and_its_parts() {
    let src = "ab {{ upper_x }} cd";
    let t = Template::compile(src);
    let d = &t.diagnostics()[0];
    assert_eq!(d.kind, DiagKind::UnknownFunction);
    assert_eq!(d.span.slice(src), "upper_x");
    assert_eq!(d.line_col(src), (1, 7));
}

#[test]
fn grammar_accepts_every_form() {
    for ok in [
        "{{ uuid }}",
        "{{ uuid v7 }}",
        "{{ input \"Message\" | quote }}",
        "{{ input 'Raw \\ text' }}",
        "{{ id := uuid }} {{ $id }}",
        "{{ id:=uuid | silent }}{{ $id | upper }}",
        "{{ random 1 6 }}",
        "{{ random -5 5 }}",
        "{{ date_offset \"-7d\" \"%d/%m\" }}",
        "{{ git_branch default=\"main\" }}",
        "{{ input \"Db\" default=\"dev\" }}",
        "{{ date_offset (input \"Offset\") }}",
        "{{ ($x) }}{{ x := uuid }}".trim_start_matches("{{ ($x) }}"),
        "{{ pick \"a\" b 3 true }}",
        "{{ now \"%H:%M\" utc=true }}",
        "{{ input \"a}}b\" }}",
        "{{ input \"Say \\\"hi\\\"\\n\\t\\u{1F600}\" }}",
        "\\{{ literal }} {{ cursor }}",
    ] {
        assert_eq!(kinds(ok), vec![], "{ok}");
    }
}

#[test]
fn every_diagnostic_kind_with_its_span() {
    let cases: &[(&str, DiagKind, &str)] = &[
        ("a {{}} b", DiagKind::EmptyTag, "{{}}"),
        ("a {{   }} b", DiagKind::EmptyTag, "{{   }}"),
        ("a {{ uuid", DiagKind::UnterminatedTag, "{{ uuid"),
        ("a {{ uuid\n}}", DiagKind::NewlineInTag, "\n"),
        (
            "{{ input \"x }}",
            DiagKind::UnterminatedTag,
            "{{ input \"x }}",
        ),
        ("{{ input \"a\\qb\" }}", DiagKind::BadEscape, "\\q"),
        (
            "{{ input \"a\\u{110000}\" }}",
            DiagKind::BadEscape,
            "\\u{110000}",
        ),
        ("{{ | upper }}", DiagKind::Syntax, "|"),
        ("{{ uuid | }}", DiagKind::Syntax, ""),
        ("{{ uuid ) }}", DiagKind::Syntax, ")"),
        ("{{ date_offset (uuid }}", DiagKind::Syntax, ""),
        ("{{ uuid @ }}", DiagKind::Syntax, "@"),
        ("{{ nope }}", DiagKind::UnknownFunction, "nope"),
        ("{{ upper }}", DiagKind::UnknownFunction, "upper"),
        ("{{ uuid | nope }}", DiagKind::UnknownFilter, "nope"),
        ("{{ uuid | uuid }}", DiagKind::UnknownFilter, "uuid"),
        ("{{ uuid colour=1 }}", DiagKind::UnknownArg, "colour"),
        ("{{ nanoid len=1 len=2 }}", DiagKind::UnknownArg, "len"),
        ("{{ uuid v4 v7 }}", DiagKind::Arity, "uuid v4 v7"),
        ("{{ random 1 }}", DiagKind::Arity, "random 1"),
        ("{{ pick }}", DiagKind::Arity, "pick"),
        ("{{ random 1 x }}", DiagKind::ArgType, "x"),
        ("{{ uuid v5 }}", DiagKind::ArgType, "v5"),
        ("{{ nanoid len=big }}", DiagKind::ArgType, "big"),
        ("{{ password symbols=3 }}", DiagKind::ArgType, "3"),
        ("{{ cursor }}{{ cursor }}", DiagKind::SecondCursor, "cursor"),
        ("{{ $b }}", DiagKind::UnboundVar, "$b"),
        ("{{ b := $b }}", DiagKind::UnboundVar, "$b"),
        ("{{ b := uuid }}{{ b := uuid }}", DiagKind::Rebound, "b"),
        (
            "{{ input \"A\" }}{{ secret \"A\" }}",
            DiagKind::PromptConflict,
            "\"A\"",
        ),
        ("{{ input (uuid) }}", DiagKind::ArgType, "(uuid)"),
        ("{{ choice \"A\" (uuid) }}", DiagKind::ArgType, "(uuid)"),
    ];
    for (src, kind, text) in cases {
        let t = Template::compile(src);
        let d = t
            .diagnostics()
            .into_iter()
            .find(|d| d.kind == *kind)
            .unwrap_or_else(|| panic!("{src}: want {kind:?}, got {:?}", t.diagnostics()));
        if !text.is_empty() {
            assert_eq!(d.span.slice(src), *text, "{src}");
        }
        assert!(!t.is_valid());
    }
}

#[test]
fn unknown_names_get_suggestions() {
    let d = Template::compile("{{ git_brach }}").diagnostics();
    assert!(d[0].message.contains("did you mean `git_branch`"), "{d:?}");
    let d = Template::compile("{{ uuid | uper }}").diagnostics();
    assert!(d[0].message.contains("did you mean `upper`"), "{d:?}");
}

#[test]
fn prompts_are_ordered_deduplicated_and_typed() {
    let t = Template::compile(
        "{{ input \"B\" default=\"x\" }} {{ choice \"A\" \"1\" \"2\" }} {{ input \"B\" default=\"y\" }} \
         {{ secret \"S\" }} {{ date_offset (input \"Off\") }}",
    );
    assert!(t.is_valid(), "{:?}", t.diagnostics());
    let p = &t.analysis.prompts;
    let labels: Vec<_> = p.iter().map(|p| p.label.as_str()).collect();
    assert_eq!(labels, ["B", "A", "S", "Off"]);
    assert_eq!(
        p[0].default.as_deref(),
        Some("x"),
        "default from the first occurrence"
    );
    assert_eq!(p[1].options, ["1", "2"]);
    assert!(t.has_secret_prompt());
    assert!(!Template::compile("{{ input \"x\" }}").has_secret_prompt());
}

#[test]
fn bindings_flow_forward_only() {
    assert!(Template::compile("{{ a := uuid }}{{ $a }}{{ $a | upper }}").is_valid());
    assert_eq!(kinds("{{ $a }}{{ a := uuid }}"), vec![DiagKind::UnboundVar]);
}

#[test]
fn analysis_is_skipped_when_parsing_failed() {
    let t = Template::compile("{{ nope }} {{");
    assert!(t.parse_diags.len() == 1);
    assert!(t.analysis.diags.is_empty());
    let _ = analyze(&t.ast);
}

#[test]
fn cache_returns_the_same_template() {
    let a = Template::cached("echo {{ uuid }}");
    let b = Template::cached("echo {{ uuid }}");
    assert!(std::sync::Arc::ptr_eq(&a, &b));
    assert!(!std::sync::Arc::ptr_eq(
        &a,
        &Template::cached("echo {{ ulid }}")
    ));
}

mod props {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(512))]

        #[test]
        fn parsing_never_panics(s in "\\PC{0,200}") {
            let t = Template::compile(&s);
            let _ = t.diagnostics();
        }

        #[test]
        fn parsing_never_panics_on_template_shaped_input(
            s in "([a-z|:=$() \"'\\\\{}\\n_0-9.-]|\\{\\{|\\}\\}|\\\\\\{\\{){0,60}"
        ) {
            let t = Template::compile(&s);
            for d in t.diagnostics() {
                let _ = d.line_col(&s);
                let _ = d.span.slice(&s);
            }
        }

        /// Text without `{{` is its own expansion.
        #[test]
        fn literal_text_round_trips(s in "[^{]{0,200}|([^{]|\\}\\}|\\{[^{]){0,50}") {
            prop_assume!(!s.contains("{{") && !s.contains("\\{{"));
            let t = Template::compile(&s);
            prop_assert!(t.is_valid());
            let mut ctx = super::super_mock();
            let mut sess = mt_snippets::Session::new(&ctx);
            let out = mt_snippets::expand(&t, &mut ctx, &mut sess, mt_snippets::Mode::Insert).unwrap();
            prop_assert_eq!(out.text, s);
        }

        /// An escaped `\{{` expands to a literal `{{`.
        #[test]
        fn escapes_round_trip(a in "[a-z ]{0,20}", b in "[a-z ]{0,20}") {
            let src = format!("{a}\\{{{{{b}");
            let t = Template::compile(&src);
            let mut ctx = super::super_mock();
            let mut sess = mt_snippets::Session::new(&ctx);
            let out = mt_snippets::expand(&t, &mut ctx, &mut sess, mt_snippets::Mode::Insert).unwrap();
            prop_assert_eq!(out.text, format!("{a}{{{{{b}"));
        }
    }
}

fn super_mock() -> impl mt_snippets::Context {
    struct M;
    impl mt_snippets::Context for M {
        fn now(&self) -> chrono::DateTime<chrono::FixedOffset> {
            chrono::DateTime::from_timestamp(0, 0)
                .unwrap()
                .fixed_offset()
        }
        fn fill_random(&mut self, buf: &mut [u8]) {
            buf.fill(7);
        }
        fn env(&self, _: &str) -> Option<String> {
            None
        }
        fn fact(&mut self, _: &mt_snippets::Fact) -> Result<String, String> {
            Err("none".into())
        }
        fn counter_get(&self, _: &str) -> Option<i64> {
            None
        }
    }
    M
}

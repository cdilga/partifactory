use pf_build::{BuildInput, SourceFile, kinds::Kind, render};
use std::collections::BTreeMap;

fn build(kind: &str, path: &str, source: &[u8], mime: &str) -> pf_build::BuiltFile {
    render::render(BuildInput {
        entry: path.into(),
        kind: kind.into(),
        request_id: None,
        mirrored: BTreeMap::new(),
        files: BTreeMap::from([(
            path.into(),
            SourceFile {
                bytes: source.to_vec(),
                content_type: mime.into(),
            },
        )]),
    })
    .unwrap()
    .files
    .remove(path)
    .unwrap()
}

#[test]
fn svg_stays_exact_and_outside_the_shared_html_shell() {
    let source = include_bytes!("../../../tests/golden/render/svg/drawing.svg");
    let output = build("svg", "drawing.svg", source, "image/svg+xml");
    assert_eq!(output.bytes, source);
    assert_eq!(output.content_type, "image/svg+xml");
    assert!(!String::from_utf8(output.bytes).unwrap().contains("pf-boot"));
    assert_eq!(pf_build::kinds::detect("drawing.svg").unwrap(), Kind::Svg);
}

#[test]
fn code_is_highlighted_but_markup_and_control_characters_stay_literal() {
    let source = include_str!("../../../tests/golden/render/code/hostile.rs");
    let output = build("code", "hostile.rs", source.as_bytes(), "text/plain");
    let html = String::from_utf8(output.bytes).unwrap();
    assert_eq!(output.content_type, "text/html; charset=utf-8");
    assert!(html.contains("id=\"pf-code\""));
    assert!(html.contains("light-dark("));
    assert!(html.contains("id=\"L1\""));
    assert!(html.contains(&format!("id=\"L{}\"", source.lines().count() + 1)));
    assert!(!html.contains("<script>"));
    assert!(html.contains("&lt;script&gt;"));
    assert!(html.contains("λ"));
    let exact = render::code_shell("\u{feff}<tag>\r\n&two\n", "plain.unknown").unwrap();
    assert!(exact.contains('\u{feff}'));
    assert!(exact.contains("&lt;tag&gt;&#13;\n"));
    assert!(exact.contains("&amp;two\n"));
    assert!(exact.contains("id=\"L3\""));
}

#[test]
fn empty_code_and_unknown_syntax_keep_a_readable_shell() {
    assert!(
        render::code_shell("", "note.unknown")
            .unwrap()
            .contains("id=\"L1\"")
    );
    let source = "one /* comment\n  still comment */ λ\n";
    let html = render::code_shell(source, "fixture.rs").unwrap();
    assert!(html.contains("still comment"));
    assert!(html.contains("id=\"L3\""));
}

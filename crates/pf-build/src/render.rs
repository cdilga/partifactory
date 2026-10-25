//! Deterministic Slice 1 rendering. Source bytes are never changed in storage.
use crate::{BuildInput, BuildOutput, BuiltFile, Diagnostic, kinds::Kind, scan};
use lol_html::{RewriteStrSettings, element, html_content::ContentType, rewrite_str, text};
use std::{cell::RefCell, collections::BTreeMap, rc::Rc};

pub const SHELL_CSS: &str = "html{color-scheme:light dark}body{max-width:72rem;margin:2rem auto;padding:0 1rem;font:17px/1.6 system-ui}pre,code{font-family:ui-monospace,monospace}pre{white-space:pre-wrap;overflow-wrap:anywhere}.pf-line{display:block;min-height:1.6em}.pf-line:before{content:attr(data-line);display:inline-block;width:3em;margin-right:1em;color:GrayText;user-select:none}.pf-line:target{background:Highlight;color:HighlightText}img{max-width:100%}table{border-collapse:collapse}td,th{padding:.3em .7em;border:1px solid GrayText}.pf-proportional{font-family:system-ui}";

pub fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn shell(body: &str) -> String {
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>partifactory</title></head><body>{body}</body></html>"
    )
}

pub fn text_shell(source: &str) -> String {
    let mut body = String::from(
        "<button type=\"button\" onclick=\"document.getElementById('pf-text').classList.toggle('pf-proportional')\">Toggle monospace</button><pre id=\"pf-text\">",
    );
    for (i, line) in source.split_inclusive('\n').enumerate() {
        body.push_str(&format!(
            "<span class=\"pf-line\" id=\"L{}\" data-line=\"{}\">{}</span>",
            i + 1,
            i + 1,
            escape(line).replace('\r', "&#13;")
        ));
    }
    if source.is_empty() || source.ends_with('\n') {
        let n = source.bytes().filter(|b| *b == b'\n').count() + 1;
        body.push_str(&format!(
            "<span class=\"pf-line\" id=\"L{n}\" data-line=\"{n}\"></span>"
        ));
    }
    body.push_str("</pre>");
    shell(&body)
}

fn url(u: &str, path: &str, mirrored: &BTreeMap<String, String>) -> Option<String> {
    if let Some(value) = mirrored.get(u) {
        return Some(value.clone());
    }
    if u.starts_with('/') && !u.starts_with("//") {
        let depth = path.split('/').count() - 1;
        return Some(format!("{}{}", "../".repeat(depth), &u[1..]));
    }
    None
}

fn css(source: &str, path: &str, mirrored: &BTreeMap<String, String>) -> String {
    scan::rewrite_css(source, &mut |u, _| url(u, path, mirrored))
}

fn js(
    source: &str,
    path: &str,
    mirrored: &BTreeMap<String, String>,
    warnings: &mut Vec<Diagnostic>,
) -> String {
    let (out, roots) = scan::rewrite_js(source, &mut |u| url(u, path, mirrored));
    for root in roots {
        warnings.push(Diagnostic {
            code: "root_relative_url".into(),
            file: Some(path.into()),
            message: format!("Runtime URL {root} needs the prelude's URL shim"),
        });
    }
    out
}

pub fn html(
    source: &str,
    path: &str,
    mirrored: &BTreeMap<String, String>,
    warnings: &mut Vec<Diagnostic>,
) -> Result<String, String> {
    let buffered_css = Rc::new(RefCell::new(String::new()));
    let buffered_js = Rc::new(RefCell::new(String::new()));
    let wc = Rc::new(RefCell::new(Vec::new()));
    let ws = wc.clone();
    let output = rewrite_str(
        source,
        RewriteStrSettings::new()
            .append_element_content_handler(element!(
                "[src], [href], [srcset], [style], [poster], [data]",
                |el| {
                    for attr in ["src", "href", "poster", "data"] {
                        if let Some(value) = el.get_attribute(attr)
                            && let Some(replacement) = url(&value, path, mirrored)
                        {
                            el.set_attribute(attr, &replacement)?;
                            // A rewritten dependency has different bytes after transitive rewriting.
                            // Its original integrity is verified by the parent before rewriting.
                            if mirrored.contains_key(&value) {
                                el.remove_attribute("integrity");
                            }
                        }
                    }
                    if let Some(value) = el.get_attribute("srcset")
                        && let Some(value) =
                            scan::rewrite_srcset(&value, &mut |u| url(u, path, mirrored))
                    {
                        el.set_attribute("srcset", &value)?;
                    }
                    if let Some(value) = el.get_attribute("style") {
                        el.set_attribute("style", &css(&value, path, mirrored))?;
                    }
                    Ok(())
                }
            ))
            .append_element_content_handler(element!("img", |el| {
                el.set_attribute("crossorigin", "anonymous")?;
                Ok(())
            }))
            .append_element_content_handler(text!("style", move |chunk| {
                buffered_css.borrow_mut().push_str(chunk.as_str());
                if chunk.last_in_text_node() {
                    let value = css(&buffered_css.borrow(), path, mirrored);
                    chunk.replace(&value, ContentType::Html);
                    buffered_css.borrow_mut().clear();
                } else {
                    chunk.remove();
                }
                Ok(())
            }))
            .append_element_content_handler(text!("script:not([src])", move |chunk| {
                buffered_js.borrow_mut().push_str(chunk.as_str());
                if chunk.last_in_text_node() {
                    let value = js(&buffered_js.borrow(), path, mirrored, &mut ws.borrow_mut());
                    chunk.replace(&value, ContentType::Html);
                    buffered_js.borrow_mut().clear();
                } else {
                    chunk.remove();
                }
                Ok(())
            })),
    )
    .map_err(|e| e.to_string())?;
    warnings.extend(wc.borrow_mut().drain(..));
    Ok(output)
}

/// The same Markdown rendering feeds dependency discovery and final output.
pub fn markdown(text: &str) -> String {
    let mut options = comrak::Options::default();
    options.extension.table = true;
    options.extension.strikethrough = true;
    options.extension.autolink = true;
    options.extension.tasklist = true;
    options.render.r#unsafe = true;
    comrak::markdown_to_html(text, &options)
}
pub fn render(input: BuildInput) -> Result<BuildOutput, String> {
    let kind = Kind::parse(&input.kind).map_err(|e| e.to_string())?;
    let mut output = BuildOutput {
        files: BTreeMap::new(),
        warnings: vec![],
    };
    for (path, source) in input.files {
        let mut typ = source.content_type;
        let text = std::str::from_utf8(&source.bytes).ok();
        let bytes = if path == input.entry && matches!(kind, Kind::Text | Kind::Markdown) {
            typ = "text/html; charset=utf-8".into();
            let text = text.ok_or("entry is not UTF-8")?;
            let source = if kind == Kind::Text {
                text_shell(text)
            } else {
                shell(&markdown(text))
            };
            html(&source, &path, &input.mirrored, &mut output.warnings)?.into_bytes()
        } else if let Some(text) = text {
            if typ.starts_with("text/html") {
                html(text, &path, &input.mirrored, &mut output.warnings)?.into_bytes()
            } else if typ.starts_with("text/css") {
                css(text, &path, &input.mirrored).into_bytes()
            } else if typ.contains("javascript") {
                js(text, &path, &input.mirrored, &mut output.warnings).into_bytes()
            } else {
                source.bytes
            }
        } else {
            source.bytes
        };
        let compressed = if path != input.entry && compressible(&typ) {
            use std::io::Write;
            let mut compressed = Vec::new();
            let quality = if bytes.len() <= 1 << 20 { 11 } else { 5 };
            {
                let mut writer = brotli::CompressorWriter::new(&mut compressed, 4096, quality, 22);
                writer.write_all(&bytes).map_err(|e| e.to_string())?;
            }
            Some(compressed)
        } else {
            None
        };
        output.files.insert(
            path,
            BuiltFile {
                bytes,
                compressed,
                content_type: typ,
            },
        );
    }
    Ok(output)
}

pub fn compressible(typ: &str) -> bool {
    typ.starts_with("text/")
        || typ.contains("javascript")
        || typ.contains("json")
        || typ.contains("svg")
        || typ.contains("xml")
        || typ.contains("wasm")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rewrites_nested_paths_and_keeps_ordinary_strings() {
        let source = r#"<img src="/a.png" srcset="/a.png 1x, /b.png 2x"><style>.a{background:url('/b.png')}</style><script>fetch('/data.json'); import('/module.js'); //# sourceMappingURL=x
        </script>"#;
        let mut warnings = vec![];
        let output = html(source, "dir/index.html", &BTreeMap::new(), &mut warnings).unwrap();
        assert!(output.contains("../a.png"));
        assert!(output.contains("crossorigin=\"anonymous\""));
        assert!(output.contains("url('../b.png')"));
        assert!(output.contains("import('../module.js')"));
        assert!(output.contains("fetch('/data.json')"));
        assert!(!output.contains("sourceMappingURL"));
        assert_eq!(warnings[0].code, "root_relative_url");
    }
    #[test]
    fn text_keeps_bom_crlf_trailing_newline_and_escapes_markup() {
        let output = text_shell("\u{feff}<one>\r\n&two\n");
        assert!(output.contains("\u{feff}&lt;one&gt;&#13;\n</span>"));
        assert!(output.contains("&amp;two\n</span>"));
        assert!(output.contains("id=\"L3\""));
    }
}

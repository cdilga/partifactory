//! WCAG contrast for the actual shipped Stockroom palette, not a duplicated fixture.
use std::collections::HashMap;
fn palette(css: &str) -> HashMap<&str, [f64; 3]> {
    css.split(';')
        .filter_map(|line| {
            let (key, value) = line.rsplit_once(':')?;
            let value = value.trim();
            let hex = value.strip_prefix('#')?.get(..6)?;
            let n = u32::from_str_radix(hex, 16).ok()?;
            Some((
                key.trim().rsplit('\n').next()?.trim(),
                [
                    ((n >> 16) & 255) as f64 / 255.,
                    ((n >> 8) & 255) as f64 / 255.,
                    (n & 255) as f64 / 255.,
                ],
            ))
        })
        .collect()
}
fn luminance(c: [f64; 3]) -> f64 {
    c.into_iter()
        .zip([0.2126, 0.7152, 0.0722])
        .map(|(v, w)| {
            w * if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        })
        .sum()
}
#[test]
fn stockroom_light_and_dark_contrast() {
    let (light, dark) = pf_ui::TOKENS_CSS
        .split_once(":root[data-theme=\"dark\"]")
        .unwrap();
    let dark = dark.split("@media").next().unwrap();
    for (theme, css) in [("light", light), ("dark", dark)] {
        let p = palette(css);
        for (ink, paper, min) in [
            ("--pf-ink", "--pf-paper", 4.5),
            ("--pf-ink-2", "--pf-paper", 4.5),
            ("--pf-ink-3", "--pf-paper", 4.5),
            ("--pf-ink-3", "--pf-raised", 4.5),
            ("--pf-on-tag", "--pf-tag", 4.5),
            ("--pf-on-signal", "--pf-signal", 4.5),
            ("--pf-on-wash", "--pf-signal-wash", 4.5),
            ("--pf-public-text", "--pf-public-wash", 4.5),
            ("--pf-agent", "--pf-agent-wash", 4.5),
            ("--pf-danger", "--pf-paper", 4.5),
            ("--pf-rule-strong", "--pf-paper", 3.0),
        ] {
            let a = luminance(p[ink]);
            let b = luminance(p[paper]);
            let ratio = (a.max(b) + 0.05) / (a.min(b) + 0.05);
            assert!(
                ratio >= min,
                "{theme}: {ink} on {paper} = {ratio:.2}, needs {min}"
            );
        }
    }
}

#[test]
fn hydrated_shell_fits_budget_and_precompressed_assets_match() {
    use std::io::Read;
    let assets: [(&[u8], &[u8]); 2] = [
        (
            include_bytes!("../../pf-ui-shell/pkg/pf_ui_shell.js"),
            include_bytes!("../../pf-ui-shell/pkg/pf_ui_shell.js.br"),
        ),
        (
            include_bytes!("../../pf-ui-shell/pkg/pf_ui_shell_bg.wasm"),
            include_bytes!("../../pf-ui-shell/pkg/pf_ui_shell_bg.wasm.br"),
        ),
    ];
    assert!(assets.iter().map(|(_, br)| br.len()).sum::<usize>() <= 250 * 1024);
    for (raw, br) in assets {
        let mut decoded = Vec::new();
        brotli::Decompressor::new(br, 4096)
            .read_to_end(&mut decoded)
            .unwrap();
        assert_eq!(decoded, raw);
    }
}

use std::path::Path;

use anyhow::Result;

use crate::kicad::Page;

const TEMPLATE_HTML: &str = include_str!("assets/template.html");
const STYLE_CSS: &str = include_str!("assets/style.css");
const SCRIPT_JS: &str = include_str!("assets/script.js");

pub fn generate_site(out_dir: &Path, base_ref: &str, head_ref: &str, pages: &[Page]) -> Result<()> {
    let mut entries_js = String::from("const entries = [\n");
    for page in pages {
        entries_js.push_str("  { project: ");
        entries_js.push_str(&json_escape(&page.project));
        entries_js.push_str(", kind: ");
        entries_js.push_str(&json_escape(page.kind.as_str()));
        entries_js.push_str(", name: ");
        entries_js.push_str(&json_escape(&page.name));
        entries_js.push_str(", path: ");
        entries_js.push_str(&json_escape(&page.rel));
        entries_js.push_str(", status: ");
        entries_js.push_str(&json_escape(page.status.as_str()));
        if let Some(edge) = &page.edge {
            entries_js.push_str(", edge: ");
            entries_js.push_str(&json_escape(edge));
        }
        entries_js.push_str(" },\n");
    }
    entries_js.push_str("];\n");

    let html = TEMPLATE_HTML
        .replace("__BASE__", &html_escape(base_ref))
        .replace("__HEAD__", &html_escape(head_ref));

    let assets_dir = out_dir.join("assets");
    std::fs::create_dir_all(&assets_dir)?;
    std::fs::write(out_dir.join("index.html"), html)?;
    std::fs::write(assets_dir.join("style.css"), STYLE_CSS)?;
    std::fs::write(assets_dir.join("script.js"), SCRIPT_JS)?;
    std::fs::write(assets_dir.join("entries.js"), entries_js)?;
    Ok(())
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

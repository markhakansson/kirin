use std::path::Path;

use anyhow::Result;

use crate::kicad::Page;
use crate::semantic::Change;

const TEMPLATE_HTML: &str = include_str!("assets/template.html");
const STYLE_CSS: &str = include_str!("assets/style.css");
const SCRIPT_JS: &str = include_str!("assets/script.js");

pub fn generate_site(
    out_dir: &Path,
    base_ref: &str,
    head_ref: &str,
    pages: &[Page],
    changes: &[Change],
) -> Result<()> {
    let entries_js = render_entries(pages, changes);
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

/// The data file consumed by script.js: an `entries` array of pages and a
/// `changes` array of semantic rows. The key names are the contract with
/// the viewer.
fn render_entries(pages: &[Page], changes: &[Change]) -> String {
    let mut entries_js = String::from("const entries = [\n");
    for page in pages {
        entries_js.push_str("  { project: ");
        entries_js.push_str(&json_escape(page.project.as_ref()));
        entries_js.push_str(", kind: ");
        entries_js.push_str(&json_escape(page.kind.as_str()));
        entries_js.push_str(", name: ");
        entries_js.push_str(&json_escape(page.name.as_ref()));
        entries_js.push_str(", path: ");
        entries_js.push_str(&json_escape(&page.rel));
        entries_js.push_str(", status: ");
        entries_js.push_str(&json_escape(page.status.as_str()));
        if let Some(edge) = &page.edge_base {
            entries_js.push_str(", edgeBase: ");
            entries_js.push_str(&json_escape(edge));
        }
        if let Some(edge) = &page.edge_head {
            entries_js.push_str(", edgeHead: ");
            entries_js.push_str(&json_escape(edge));
        }
        if let Some(parts) = page.parts {
            entries_js.push_str(&format!(", parts: {parts}"));
        }
        entries_js.push_str(" },\n");
    }
    entries_js.push_str("];\n");

    entries_js.push_str("const changes = [\n");
    for change in changes {
        entries_js.push_str("  { project: ");
        entries_js.push_str(&json_escape(change.project.as_ref()));
        entries_js.push_str(", scope: ");
        entries_js.push_str(&json_escape(change.scope.as_str()));
        if let Some(sheet) = &change.sheet {
            entries_js.push_str(", sheet: ");
            entries_js.push_str(&json_escape(sheet.as_ref()));
        }
        if let Some(layer) = &change.layer {
            entries_js.push_str(", layer: ");
            entries_js.push_str(&json_escape(layer.as_ref()));
        }
        entries_js.push_str(", kind: ");
        entries_js.push_str(&json_escape(change.kind.as_str()));
        entries_js.push_str(", ref: ");
        entries_js.push_str(&json_escape(change.reference.as_ref()));
        entries_js.push_str(", detail: ");
        entries_js.push_str(&json_escape(&change.detail));
        if let Some([x, y]) = change.frac_base {
            entries_js.push_str(&format!(", fb: [{x:.4}, {y:.4}]"));
        }
        if let Some([x, y]) = change.frac_head {
            entries_js.push_str(&format!(", fh: [{x:.4}, {y:.4}]"));
        }
        entries_js.push_str(" },\n");
    }
    entries_js.push_str("];\n");
    entries_js
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kicad::{Kind, Status};
    use crate::semantic::ChangeKind;

    #[test]
    fn renders_the_viewer_data_contract() {
        let pages = vec![Page {
            project: "proj".into(),
            kind: Kind::Sch,
            name: "Root sheet".into(),
            rel: "proj/sch/root.svg".to_string(),
            status: Status::Modified,
            edge_base: Some("a/svg/edge.svg".to_string()),
            edge_head: None,
            parts: Some(12),
        }];
        let changes = vec![Change {
            project: "proj".into(),
            scope: Kind::Sch,
            sheet: Some("Root sheet".into()),
            kind: ChangeKind::ValueChanged,
            reference: "R1".into(),
            detail: "say \"10k\" -> back\\slash\nline".to_string(),
            layer: None,
            at_base: None,
            at_head: None,
            frac_base: Some([0.25, 0.5]),
            frac_head: None,
        }];
        let js = render_entries(&pages, &changes);
        // The keys script.js reads; renaming one here must fail a test.
        for key in [
            "const entries = [",
            "const changes = [",
            "project: \"proj\"",
            "kind: \"sch\"",
            "name: \"Root sheet\"",
            "path: \"proj/sch/root.svg\"",
            "status: \"modified\"",
            "edgeBase: \"a/svg/edge.svg\"",
            "parts: 12",
            "scope: \"sch\"",
            "sheet: \"Root sheet\"",
            "kind: \"value\"",
            "ref: \"R1\"",
            "fb: [0.2500, 0.5000]",
        ] {
            assert!(js.contains(key), "missing `{key}` in:\n{js}");
        }
        // Quotes, backslashes and newlines survive as valid JS.
        assert!(js.contains(r#"detail: "say \"10k\" -> back\\slash\nline""#));
        // Absent optionals stay absent.
        assert!(!js.contains("edgeHead"));
        assert!(!js.contains("fh:"));
        assert!(!js.contains("layer:"));
    }
}

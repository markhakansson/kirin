use std::path::Path;

use anyhow::Result;

use crate::kicad::Page;
use crate::semantic::Change;
use crate::sidebar::Sidebar;

const TEMPLATE_HTML: &str = include_str!("assets/template.html");
const STYLE_CSS: &str = include_str!("assets/style.css");
const SCRIPT_JS: &str = include_str!("assets/script.js");

/// Generates the static site at `out_dir` for the changes.
pub fn generate_site(
    out_dir: &Path,
    base_ref: &str,
    head_ref: &str,
    pages: &[Page],
    changes: &[Change],
) -> Result<()> {
    let sidebar = Sidebar::new(pages, changes);
    let entries_js = render_entries(pages, changes, &sidebar);
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

/// `{ key: value, ... }`, dropping the fields that are absent.
fn obj(fields: &[(&str, Option<String>)]) -> String {
    let body: Vec<_> = fields
        .iter()
        .filter_map(|(key, value)| value.as_ref().map(|v| format!("{key}: {v}")))
        .collect();
    format!("{{ {} }}", body.join(", "))
}

/// `[a , b, ...]` as one line.
fn arr(items: impl IntoIterator<Item = String>) -> String {
    format!("[{}]", items.into_iter().collect::<Vec<_>>().join(", "))
}

/// A top-level array declaration, one item per line.
fn decl(name: &str, items: impl IntoIterator<Item = String>) -> String {
    let mut out = format!("const {name} = [\n");
    for item in items {
        out.push_str(&format!("  {item},\n"));
    }
    out.push_str("];\n");
    out
}

/// Creates the content for `entries.js`, which contains all necessary metadata
/// about the changes for the project. Used by `script.js`.
fn render_entries(pages: &[Page], changes: &[Change], sidebar: &Sidebar) -> String {
    let entries = pages.iter().map(|page| {
        obj(&[
            ("project", Some(json_escape(page.project.as_ref()))),
            ("kind", Some(json_escape(page.kind.as_str()))),
            ("name", Some(json_escape(page.name.as_ref()))),
            ("path", Some(json_escape(&page.rel))),
            ("status", Some(json_escape(page.status.as_str()))),
            ("edgeBase", page.edge_base.as_deref().map(json_escape)),
            ("edgeHead", page.edge_head.as_deref().map(json_escape)),
            ("parts", page.parts.map(|n| n.to_string())),
        ])
    });

    let frac = |at: Option<[f64; 2]>| at.map(|[x, y]| format!("[{x:.4}, {y:.4}]"));
    let changes_js = changes.iter().map(|change| {
        obj(&[
            ("project", Some(json_escape(change.project.as_ref()))),
            ("scope", Some(json_escape(change.scope.as_str()))),
            (
                "sheet",
                change.sheet.as_ref().map(|s| json_escape(s.as_ref())),
            ),
            (
                "layer",
                change.layer.as_ref().map(|l| json_escape(l.as_ref())),
            ),
            ("kind", Some(json_escape(change.kind.as_str()))),
            ("ref", Some(json_escape(change.reference.as_ref()))),
            ("detail", Some(json_escape(&change.detail))),
            ("fb", frac(change.frac_base)),
            ("fh", frac(change.frac_head)),
        ])
    });

    let page_groups = sidebar.projects.iter().map(|project| {
        let kinds = project.kinds.iter().map(|kind| {
            obj(&[
                ("kind", Some(json_escape(kind.kind.as_str()))),
                ("label", Some(json_escape(kind.kind.label()))),
                ("pages", Some(arr(kind.pages.iter().map(usize::to_string)))),
            ])
        });
        obj(&[
            ("project", Some(json_escape(project.project.as_ref()))),
            ("kinds", Some(arr(kinds))),
        ])
    });

    // Semantic changes metadata.
    let change_groups = sidebar.groups.iter().map(|group| {
        let parts = group.parts.iter().map(|part| {
            let rows = part.rows.iter().map(|row| {
                obj(&[
                    ("c", Some(row.change.to_string())),
                    ("badge", row.badge.map(json_escape)),
                ])
            });
            obj(&[
                ("ref", Some(json_escape(part.reference.as_ref()))),
                ("status", Some(json_escape(part.status.as_str()))),
                ("rows", Some(arr(rows))),
            ])
        });
        obj(&[
            ("page", Some(group.page.to_string())),
            ("count", Some(group.count.to_string())),
            ("summary", Some(json_escape(&group.summary))),
            ("parts", Some(arr(parts))),
        ])
    });

    [
        decl("entries", entries),
        decl("changes", changes_js),
        decl("pageGroups", page_groups),
        decl("changeGroups", change_groups),
        decl("changeOrder", sidebar.order.iter().map(usize::to_string)),
    ]
    .concat()
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
        let sidebar = Sidebar::new(&pages, &changes);
        let js = render_entries(&pages, &changes, &sidebar);
        // The keys script.js reads; renaming one here must fail a test.
        for key in [
            "const entries = [",
            "const changes = [",
            "const pageGroups = [",
            "const changeGroups = [",
            "const changeOrder = [",
            r#"{ kind: "sch", label: "Schematics", pages: [0] }"#,
            "page: 0, count: 1",
            r#"summary: "1 change · 8% of parts changed""#,
            r#"rows: [{ c: 0, badge: "modified" }]"#,
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

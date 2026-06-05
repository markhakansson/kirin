use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use anyhow::Result;

const TEMPLATE_HTML: &str = include_str!("assets/template.html");
const STYLE_CSS: &str = include_str!("assets/style.css");
const SCRIPT_JS: &str = include_str!("assets/script.js");

#[derive(PartialEq, Clone, Copy)]
enum Status {
    Unchanged,
    Modified,
    Added,
    Removed,
}

impl Status {
    fn as_str(self) -> &'static str {
        match self {
            Status::Unchanged => "unchanged",
            Status::Modified => "modified",
            Status::Added => "added",
            Status::Removed => "removed",
        }
    }
}

pub fn generate_site(
    out_dir: &Path,
    base_ref: &str,
    head_ref: &str,
    base_sch_svg_dir: &Path,
    head_sch_svg_dir: &Path,
    base_pcb_svg_dir: &Path,
    head_pcb_svg_dir: &Path,
) -> Result<()> {
    let mut entries_js = String::from("const entries = [\n");
    append_entries(
        &mut entries_js,
        "sch",
        base_sch_svg_dir,
        head_sch_svg_dir,
    )?;
    append_entries(
        &mut entries_js,
        "pcb",
        base_pcb_svg_dir,
        head_pcb_svg_dir,
    )?;
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

fn append_entries(
    out: &mut String,
    kind: &str,
    base_svg_dir: &Path,
    head_svg_dir: &Path,
) -> Result<()> {
    let base_svgs = collect_svgs(base_svg_dir)?;
    let head_svgs = collect_svgs(head_svg_dir)?;
    let all: BTreeSet<&PathBuf> = base_svgs.iter().chain(head_svgs.iter()).collect();

    for rel in &all {
        let status = classify(rel, base_svg_dir, head_svg_dir)?;
        if status == Status::Unchanged {
            continue;
        }

        let rel_str = rel.to_string_lossy();
        out.push_str("  { kind: ");
        out.push_str(&json_escape(kind));
        out.push_str(", path: ");
        out.push_str(&json_escape(&rel_str));
        out.push_str(", status: ");
        out.push_str(&json_escape(status.as_str()));
        out.push_str(" },\n");
    }
    Ok(())
}

fn classify(rel: &Path, base_svg_dir: &Path, head_svg_dir: &Path) -> Result<Status> {
    let base_path = base_svg_dir.join(rel);
    let head_path = head_svg_dir.join(rel);
    match (base_path.exists(), head_path.exists()) {
        (true, true) => {
            let a = normalize_svg(&std::fs::read(&base_path)?);
            let b = normalize_svg(&std::fs::read(&head_path)?);
            Ok(if a == b {
                Status::Unchanged
            } else {
                Status::Modified
            })
        }
        (true, false) => Ok(Status::Removed),
        (false, true) => Ok(Status::Added),
        (false, false) => unreachable!(),
    }
}

// kicad-cli embeds a generation timestamp in the <title> element. Strip it so
// runs of the same source compare as equal.
fn normalize_svg(content: &[u8]) -> String {
    String::from_utf8_lossy(content)
        .lines()
        .filter(|line| !line.trim_start().starts_with("<title>SVG Image created"))
        .collect::<Vec<_>>()
        .join("\n")
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

fn collect_svgs(dir: &Path) -> Result<BTreeSet<PathBuf>> {
    let mut out = BTreeSet::new();
    if dir.exists() {
        walk_svgs(dir, dir, &mut out)?;
    }
    Ok(out)
}

fn walk_svgs(root: &Path, dir: &Path, out: &mut BTreeSet<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            walk_svgs(root, &path, out)?;
        } else if path.extension().and_then(|e| e.to_str()) == Some("svg") {
            out.insert(path.strip_prefix(root)?.to_path_buf());
        }
    }
    Ok(())
}

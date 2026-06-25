use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use oxvg_ast::{
    parse::roxmltree::{ParsingOptions, parse_with_options},
    serialize::Node as _,
    visitor::Info,
};
use oxvg_optimiser::Jobs;

use crate::kicad::{Page, Status};

/// Minify every SVG the report links to, in place. KiCAD's exports carry a lot
/// of redundant precision and metadata; shrinking them cuts both transfer and
/// in-browser decode cost. A failure on one file is logged and left as-is rather
/// than aborting the report.
pub fn optimize_pages(out: &Path, pages: &[Page]) {
    let mut files: BTreeSet<PathBuf> = BTreeSet::new();
    for page in pages {
        if page.status != Status::Added {
            files.insert(out.join("a").join("svg").join(&page.rel));
        }
        if page.status != Status::Removed {
            files.insert(out.join("b").join("svg").join(&page.rel));
        }
        if let Some(edge) = &page.edge {
            files.insert(out.join(edge));
        }
    }
    let (mut before, mut after, mut count) = (0, 0, 0);
    for path in &files {
        match optimize_file(path) {
            Ok((b, a)) => {
                before += b;
                after += a;
                count += 1;
            }
            Err(e) => eprintln!("warning: SVG optimization skipped for '{}': {e}", path.display()),
        }
    }
    if before > 0 {
        let saved = (1.0 - after as f64 / before as f64) * 100.0;
        eprintln!(
            "Minified {count} SVG(s): {} -> {} ({saved:.1}% smaller)",
            human(before),
            human(after),
        );
    }
}

fn optimize_file(path: &Path) -> Result<(u64, u64)> {
    let source = fs::read_to_string(path)?;
    let before = source.len() as u64;
    let optimized = optimize(&source)?;
    let after = optimized.len() as u64;
    fs::write(path, optimized)?;
    Ok((before, after))
}

fn human(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

/// Run the default optimiser jobs over one SVG document.
fn optimize(source: &str) -> Result<String> {
    // KiCAD prepends an SVG DOCTYPE; roxmltree rejects a DTD unless allowed.
    let options = ParsingOptions { allow_dtd: true, ..Default::default() };
    parse_with_options(source, options, |dom, allocator| -> Result<String, String> {
        Jobs::default()
            .run(dom, &Info::new(allocator))
            .map_err(|e| e.to_string())?;
        dom.serialize().map_err(|e| e.to_string())
    })
    .map_err(|e| anyhow::anyhow!("parse: {e}"))?
    .map_err(|e| anyhow::anyhow!("optimise: {e}"))
}

#[cfg(test)]
mod tests {
    use super::optimize;

    #[test]
    fn shrinks_and_preserves_svg() {
        let input = r##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="100">
            <!-- editor cruft -->
            <title>SVG Image created as anchor.svg date 2024</title>
            <rect x="0.000000" y="0.000000" width="100.000000" height="100.000000" fill="#ff0000"/>
        </svg>"##;
        let out = optimize(input).unwrap();
        assert!(out.len() < input.len(), "expected smaller output, got: {out}");
        assert!(out.contains("<svg"));
        assert!(!out.contains("editor cruft"));
    }
}

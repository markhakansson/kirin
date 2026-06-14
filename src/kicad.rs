use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result};
use gix::ObjectId;

/// Non-copper layers diffed by default (copper layers are always included).
/// Canonical (file-format) names, as accepted by `kicad-cli -l`.
const DEFAULT_EXTRA_LAYERS: &[&str] = &[
    "F.SilkS",
    "B.SilkS",
    "F.Mask",
    "B.Mask",
    "F.Paste",
    "B.Paste",
    "F.Fab",
    "B.Fab",
    "Edge.Cuts",
];

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Sch,
    Pcb,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Sch => "sch",
            Kind::Pcb => "pcb",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Modified,
    Added,
    Removed,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Modified => "modified",
            Status::Added => "added",
            Status::Removed => "removed",
        }
    }
}

/// A single diffable page in the report (one schematic sheet or one PCB layer).
pub struct Page {
    /// Sidebar group label (the project's repo-relative dir, or its name at the root).
    pub project: String,
    pub kind: Kind,
    /// Human-facing name ("Root sheet", "Power Switch", a layer name, ...).
    pub name: String,
    /// SVG path relative to a side's `svg/` root, used to build `a/svg/<rel>` and `b/svg/<rel>`.
    pub rel: String,
    pub status: Status,
    /// For PCB layers: an index-relative URL to the Edge.Cuts SVG, overlaid as
    /// board-outline context (e.g. `b/svg/anchor/pcb/anchor-Edge_Cuts.svg`).
    pub edge: Option<String>,
}

/// A KiCAD project, identified by its `.kicad_pro` file.
pub struct Project {
    /// Repo-relative directory containing the project.
    dir: PathBuf,
    /// Project name (the `.kicad_pro` stem); also the root schematic/PCB stem.
    name: String,
}

impl Project {
    /// Sidebar label: the dir, or the bare name when the project sits at the repo root.
    fn label(&self) -> String {
        if self.dir.as_os_str().is_empty() {
            self.name.clone()
        } else {
            self.dir.to_string_lossy().into_owned()
        }
    }
}

/// All blob paths (repo-relative) and their object ids at `commit_ref`.
pub fn tree_blobs(repo: &gix::Repository, commit_ref: &str) -> Result<Vec<(PathBuf, ObjectId)>> {
    let commit = repo
        .rev_parse_single(commit_ref)
        .with_context(|| format!("failed to resolve ref '{commit_ref}'"))?
        .object()?
        .peel_to_commit()?;

    let tree = commit.tree()?;
    let mut recorder = gix::traverse::tree::Recorder::default();
    tree.traverse().breadthfirst(&mut recorder)?;

    Ok(recorder
        .records
        .into_iter()
        .map(|e| (PathBuf::from(e.filepath.to_string()), e.oid))
        .collect())
}

/// Discover projects present on either side, optionally restricted to `filter`
/// (a repo-relative path prefix). Sorted by dir then name, deduplicated.
pub fn discover_projects(
    base: &[(PathBuf, ObjectId)],
    head: &[(PathBuf, ObjectId)],
    filter: Option<&Path>,
) -> Vec<Project> {
    let mut seen = BTreeSet::new();
    for (path, _) in base.iter().chain(head.iter()) {
        if path.extension().and_then(|e| e.to_str()) != Some("kicad_pro") {
            continue;
        }
        if let Some(prefix) = filter
            && !path.starts_with(prefix)
        {
            continue;
        }
        let dir = path.parent().unwrap_or(Path::new("")).to_path_buf();
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        seen.insert((dir, name));
    }
    seen.into_iter()
        .map(|(dir, name)| Project { dir, name })
        .collect()
}

/// Render and classify every changed page of one project. Returns only pages
/// that actually differ visually (added / removed / modified).
pub fn process_project(
    repo: &gix::Repository,
    base: &[(PathBuf, ObjectId)],
    head: &[(PathBuf, ObjectId)],
    project: &Project,
    out: &Path,
) -> Result<Vec<Page>> {
    let sch_changed = sch_oids(base, &project.dir) != sch_oids(head, &project.dir);
    let pcb_rel = project.dir.join(format!("{}.kicad_pcb", project.name));
    let pcb_changed = oid_of(base, &pcb_rel) != oid_of(head, &pcb_rel);

    if !sch_changed && !pcb_changed {
        return Ok(Vec::new());
    }

    let work = out.join(".work");
    let work_a = work.join("a");
    let work_b = work.join("b");
    materialize(repo, base, &project.dir, &work_a)?;
    materialize(repo, head, &project.dir, &work_b)?;

    let mut pages = Vec::new();
    if sch_changed {
        pages.extend(render_schematics(project, &work_a, &work_b, out)?);
    }
    if pcb_changed {
        pages.extend(render_pcb(project, &work_a, &work_b, out)?);
    }
    Ok(pages)
}

/// Map of `.kicad_sch` path -> oid under `dir` (used to detect schematic changes).
fn sch_oids(blobs: &[(PathBuf, ObjectId)], dir: &Path) -> BTreeMap<PathBuf, ObjectId> {
    blobs
        .iter()
        .filter(|(p, _)| {
            p.starts_with(dir) && p.extension().and_then(|e| e.to_str()) == Some("kicad_sch")
        })
        .map(|(p, o)| (p.clone(), *o))
        .collect()
}

fn oid_of(blobs: &[(PathBuf, ObjectId)], path: &Path) -> Option<ObjectId> {
    blobs.iter().find(|(p, _)| p == path).map(|(_, o)| *o)
}

/// Write every `.kicad_sch`/`.kicad_pcb` blob under `dir` to `dst`, preserving
/// repo-relative paths so hierarchical sheet references resolve.
fn materialize(
    repo: &gix::Repository,
    blobs: &[(PathBuf, ObjectId)],
    dir: &Path,
    dst: &Path,
) -> Result<()> {
    for (path, oid) in blobs {
        if !path.starts_with(dir) {
            continue;
        }
        match path.extension().and_then(|e| e.to_str()) {
            Some("kicad_sch") | Some("kicad_pcb") => {}
            _ => continue,
        }
        let obj = repo.find_object(*oid)?;
        let out = dst.join(path);
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&out, &obj.data)?;
    }
    Ok(())
}

/// Export the root schematic hierarchy on each side that has it, pair sheets by
/// file name, and keep only those that changed.
fn render_schematics(
    project: &Project,
    work_a: &Path,
    work_b: &Path,
    out: &Path,
) -> Result<Vec<Page>> {
    let folder = sanitize(&project.label());
    let rel_dir = format!("{folder}/sch");

    let base = export_sch_side(
        project,
        work_a,
        &out.join("a").join("svg").join(&folder).join("sch"),
    )?;
    let head = export_sch_side(
        project,
        work_b,
        &out.join("b").join("svg").join(&folder).join("sch"),
    )?;

    // Order: root sheet first, then the rest alphabetically (case-insensitive).
    let order = |name: &str| {
        if name == "Root sheet" {
            (0u8, String::new())
        } else {
            (1, name.to_lowercase())
        }
    };

    let files: BTreeSet<&String> = base.keys().chain(head.keys()).collect();
    let mut named: Vec<(String, &String)> = files
        .into_iter()
        .map(|file| (sheet_name(file, &project.name), file))
        .collect();
    named.sort_by_key(|a| order(&a.0));

    let mut pages = Vec::new();
    for (name, file) in named {
        let bp = base.get(file).map(|p| p.as_path());
        let hp = head.get(file).map(|p| p.as_path());
        if let Some(status) = classify(bp, hp)? {
            pages.push(Page {
                project: project.label(),
                kind: Kind::Sch,
                name,
                rel: format!("{rel_dir}/{file}"),
                status,
                edge: None,
            });
        }
    }
    Ok(pages)
}

/// Run `kicad-cli sch export svg` on the root schematic if present; return a map
/// of svg file name -> full path. Empty when the project has no root sheet here.
fn export_sch_side(
    project: &Project,
    work: &Path,
    svg_dir: &Path,
) -> Result<BTreeMap<String, PathBuf>> {
    let root = work
        .join(&project.dir)
        .join(format!("{}.kicad_sch", project.name));
    if !root.is_file() {
        return Ok(BTreeMap::new());
    }
    fs::create_dir_all(svg_dir)?;

    let status = Command::new("kicad-cli")
        .args(["sch", "export", "svg", "--no-background-color"])
        .arg("-o")
        .arg(svg_dir)
        .arg(&root)
        .status()
        .context("failed to invoke 'kicad-cli' - is KiCAD installed and on PATH?")?;
    if !status.success() {
        anyhow::bail!("kicad-cli failed for '{}'", root.display());
    }

    list_svgs(svg_dir)
}

/// Export each side's board in one pass, pair layers by name, and keep only
/// those that changed. Edge.Cuts is rendered too and overlaid in the viewer as
/// board-outline context (rather than baked into every layer, which would make
/// any outline change flip every layer).
fn render_pcb(project: &Project, work_a: &Path, work_b: &Path, out: &Path) -> Result<Vec<Page>> {
    let folder = sanitize(&project.label());
    let rel_dir = format!("{folder}/pcb");
    let pcb_rel = project.dir.join(format!("{}.kicad_pcb", project.name));

    let pcb_a = work_a.join(&pcb_rel);
    let pcb_b = work_b.join(&pcb_rel);
    let layers_a = side_layers(&pcb_a)?;
    let layers_b = side_layers(&pcb_b)?;

    let dir_a = out.join("a").join("svg").join(&folder).join("pcb");
    let dir_b = out.join("b").join("svg").join(&folder).join("pcb");
    if !layers_a.is_empty() {
        export_pcb_side(&pcb_a, &layers_a, &dir_a)?;
    }
    if !layers_b.is_empty() {
        export_pcb_side(&pcb_b, &layers_b, &dir_b)?;
    }

    // kicad-cli mode-multi names files by the board name and the layer's GUI
    // name ("anchor-F_Silkscreen.svg"), which can differ from the canonical
    // name passed to `-l`. Pair against the files actually produced rather than
    // guessing names, then recover a canonical label.
    let base_files = svg_files(&dir_a)?;
    let head_files = svg_files(&dir_b)?;
    let prefix = format!("{}-", project.name);
    let label_of = |file: &str| -> String {
        let stem = file
            .strip_prefix(&prefix)
            .unwrap_or(file)
            .strip_suffix(".svg")
            .unwrap_or(file);
        // "F_Silkscreen" -> "F.Silkscreen" -> "F.SilkS"
        stem.replace('_', ".").replace("Silkscreen", "SilkS")
    };

    // Edge.Cuts overlay context, preferring the head side.
    let edge_file = format!("{prefix}Edge_Cuts.svg");
    let edge = if head_files.contains(&edge_file) {
        Some(format!("b/svg/{folder}/pcb/{edge_file}"))
    } else if base_files.contains(&edge_file) {
        Some(format!("a/svg/{folder}/pcb/{edge_file}"))
    } else {
        None
    };

    // Union of produced files, in physical stackup order.
    let mut files: Vec<String> = base_files.union(&head_files).cloned().collect();
    files.sort_by_key(|f| layer_sort_key(&label_of(f)));

    let mut pages = Vec::new();
    for file in files {
        let base = base_files.contains(&file).then(|| dir_a.join(&file));
        let head = head_files.contains(&file).then(|| dir_b.join(&file));
        if let Some(status) = classify(base.as_deref(), head.as_deref())? {
            pages.push(Page {
                project: project.label(),
                kind: Kind::Pcb,
                rel: format!("{rel_dir}/{file}"),
                // The outline page shows its own diff; it needs no extra context.
                edge: (file != edge_file).then(|| edge.clone()).flatten(),
                name: label_of(&file),
                status,
            });
        }
    }
    Ok(pages)
}

/// Curated, stackup-relevant layer names of a board, or empty if it is absent.
fn side_layers(pcb: &Path) -> Result<Vec<String>> {
    if !pcb.is_file() {
        return Ok(Vec::new());
    }
    Ok(wanted_layers(&get_layers(pcb)?))
}

/// Export the given layers of one board in a single `mode-multi` pass, fit to
/// the board outline so the board fills the SVG (full-page would make it a tiny
/// object on an A4 sheet). All layers of a revision share the board's bounding
/// box, and two revisions align as long as the outline's extents are unchanged
/// (an actual resize is shown on the Edge.Cuts page). The drawing sheet is
/// excluded because its page-number field renders non-deterministically.
fn export_pcb_side(pcb: &Path, layers: &[String], svg_dir: &Path) -> Result<()> {
    fs::create_dir_all(svg_dir)?;
    let status = Command::new("kicad-cli")
        .args([
            "pcb",
            "export",
            "svg",
            "--mode-multi",
            "--fit-page-to-board",
            "--exclude-drawing-sheet",
        ])
        .arg("-l")
        .arg(layers.join(","))
        .arg("-o")
        .arg(svg_dir)
        .arg(pcb)
        .status()
        .context("failed to invoke 'kicad-cli' - is KiCAD installed and on PATH?")?;
    if !status.success() {
        anyhow::bail!("kicad-cli failed for '{}'", pcb.display());
    }
    Ok(())
}

/// Compare two rendered pages. `None` means visually identical (dropped from the report).
fn classify(base: Option<&Path>, head: Option<&Path>) -> Result<Option<Status>> {
    match (base, head) {
        (Some(b), Some(h)) => {
            let a = normalize_svg(&fs::read(b)?);
            let c = normalize_svg(&fs::read(h)?);
            Ok((a != c).then_some(Status::Modified))
        }
        (Some(_), None) => Ok(Some(Status::Removed)),
        (None, Some(_)) => Ok(Some(Status::Added)),
        (None, None) => Ok(None),
    }
}

/// kicad-cli embeds a generation timestamp in the `<title>`; strip it so runs of
/// the same source compare as equal.
fn normalize_svg(content: &[u8]) -> String {
    String::from_utf8_lossy(content)
        .lines()
        .filter(|line| !line.trim_start().starts_with("<title>SVG Image created"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Set of `.svg` file names in `dir` (empty if it does not exist).
fn svg_files(dir: &Path) -> Result<BTreeSet<String>> {
    let mut out = BTreeSet::new();
    if dir.is_dir() {
        for entry in fs::read_dir(dir)? {
            let path = entry?.path();
            if path.extension().and_then(|e| e.to_str()) == Some("svg")
                && let Some(name) = path.file_name().and_then(|n| n.to_str())
            {
                out.insert(name.to_string());
            }
        }
    }
    Ok(out)
}

fn list_svgs(dir: &Path) -> Result<BTreeMap<String, PathBuf>> {
    let mut out = BTreeMap::new();
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) == Some("svg")
            && let Some(name) = path.file_name().and_then(|n| n.to_str())
        {
            out.insert(name.to_string(), path.clone());
        }
    }
    Ok(out)
}

/// Friendly sheet name from an exported svg file name, given the project stem.
fn sheet_name(file: &str, project_name: &str) -> String {
    let stem = file.strip_suffix(".svg").unwrap_or(file);
    if stem == project_name {
        "Root sheet".to_string()
    } else if let Some(rest) = stem.strip_prefix(&format!("{project_name}-")) {
        rest.to_string()
    } else {
        stem.to_string()
    }
}

/// Keep copper layers plus the default extras, preserving board order.
fn wanted_layers(all: &[String]) -> Vec<String> {
    all.iter()
        .filter(|l| l.ends_with(".Cu") || DEFAULT_EXTRA_LAYERS.contains(&l.as_str()))
        .cloned()
        .collect()
}

/// Sort key giving physical stackup order: F.Cu, In1.Cu.., B.Cu, then the
/// extras in their canonical order, then anything else.
fn layer_sort_key(name: &str) -> (u8, i32, String) {
    if name == "F.Cu" {
        return (0, 0, name.to_string());
    }
    if let Some(n) = name.strip_prefix("In").and_then(|s| s.strip_suffix(".Cu"))
        && let Ok(n) = n.parse::<i32>()
    {
        return (0, n, name.to_string());
    }
    if name == "B.Cu" {
        return (0, 9999, name.to_string());
    }
    if let Some(i) = DEFAULT_EXTRA_LAYERS.iter().position(|l| *l == name) {
        return (1, i as i32, name.to_string());
    }
    (2, 0, name.to_string())
}

/// Parses the layer names of a `kicad_pcb` file at `pcb_path`.
fn get_layers(pcb_path: &Path) -> Result<Vec<String>> {
    let content = fs::read_to_string(pcb_path)
        .with_context(|| format!("failed to read '{}'", pcb_path.display()))?;
    let mut layers = Vec::new();
    let mut in_layers_section = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed == "(layers" {
            in_layers_section = true;
            continue;
        }
        if !in_layers_section {
            continue;
        }
        if trimmed == ")" {
            break;
        }
        if let Some(name) = extract_quoted(trimmed) {
            layers.push(name);
        }
    }
    Ok(layers)
}

fn extract_quoted(s: &str) -> Option<String> {
    let start = s.find('"')? + 1;
    let end = s[start..].find('"')? + start;
    Some(s[start..end].to_string())
}

/// Make a string safe as a path component.
fn sanitize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
            out.push(c);
        } else {
            out.push('_');
        }
    }
    let trimmed = out.trim_matches('_');
    if trimmed.is_empty() {
        "page".to_string()
    } else {
        trimmed.to_string()
    }
}

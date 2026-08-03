use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result};
use gix::ObjectId;

use crate::ids::{PageName, ProjectLabel};
use crate::netlist;
use crate::semantic::{self, Change};

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
    Fp,
    Sym,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Sch => "sch",
            Kind::Pcb => "pcb",
            Kind::Fp => "fp",
            Kind::Sym => "sym",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Modified,
    Added,
    Removed,
    /// Visually identical, kept only because a semantic change points at it
    /// (a pin's net rewired from another sheet leaves no pixel behind).
    Unchanged,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Modified => "modified",
            Status::Added => "added",
            Status::Removed => "removed",
            Status::Unchanged => "unchanged",
        }
    }
}

/// A single diffable page in the report (one schematic sheet or one PCB layer).
pub struct Page {
    /// Sidebar group label (the project's repo-relative dir, or its name at the root).
    pub project: ProjectLabel,
    pub kind: Kind,
    /// Human-facing name ("Root sheet", "Power Switch", a layer name, ...).
    pub name: PageName,
    /// SVG path relative to a side's `svg/` root, used to build `a/svg/<rel>` and `b/svg/<rel>`.
    pub rel: String,
    pub status: Status,
    /// For PCB layers: index-relative URLs to each side's Edge.Cuts SVG,
    /// overlaid as board-outline context (e.g.
    /// `a/svg/anchor/pcb/anchor-Edge_Cuts.svg`). Kept per side so the viewer
    /// shows each revision with its own outline.
    pub edge_base: Option<String>,
    pub edge_head: Option<String>,
    /// Parts on this page (unique references, union of both sides), for
    /// schematic sheets and copper layers; the denominator behind the
    /// viewer's "% of parts changed" summaries.
    pub parts: Option<usize>,
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
    fn label(&self) -> ProjectLabel {
        if self.dir.as_os_str().is_empty() {
            self.name.clone().into()
        } else {
            self.dir.to_string_lossy().into_owned().into()
        }
    }
}

/// One `kicad-cli` invocation: the closure adds the subcommand arguments,
/// `subject` is the file a failure should point at.
pub fn run_kicad_cli(subject: &Path, args: impl FnOnce(&mut Command)) -> Result<()> {
    let mut cmd = Command::new("kicad-cli");
    args(&mut cmd);
    let status = cmd
        .status()
        .context("failed to invoke 'kicad-cli' - is KiCAD installed and on PATH?")?;
    if !status.success() {
        anyhow::bail!("kicad-cli failed for '{}'", subject.display());
    }
    Ok(())
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

/// Render and classify every changed page of one project, plus the semantic
/// (part-level) changes. Returns only pages that actually differ visually
/// (added / removed / modified).
pub fn process_project(
    repo: &gix::Repository,
    base: &[(PathBuf, ObjectId)],
    head: &[(PathBuf, ObjectId)],
    project: &Project,
    out: &Path,
) -> Result<(Vec<Page>, Vec<Change>)> {
    let sch_changed = sch_oids(base, &project.dir) != sch_oids(head, &project.dir);
    let pcb_rel = project.dir.join(format!("{}.kicad_pcb", project.name));
    let pcb_changed = oid_of(base, &pcb_rel) != oid_of(head, &pcb_rel);

    if !sch_changed && !pcb_changed {
        return Ok((Vec::new(), Vec::new()));
    }

    let work = out.join(".work");
    let work_a = work.join("a");
    let work_b = work.join("b");
    materialize(repo, base, &project.dir, &work_a)?;
    materialize(repo, head, &project.dir, &work_b)?;

    let mut pages = Vec::new();
    let mut changes = Vec::new();
    if sch_changed {
        let sch_rel = project.dir.join(format!("{}.kicad_sch", project.name));
        let root_a = work_a.join(&sch_rel);
        let root_b = work_b.join(&sch_rel);
        // A renamed sheet (paired by instance UUID) stays one page under
        // its head name instead of splitting into an added/removed couple.
        let renames = semantic::sheet_renames(&root_a, &root_b)?;
        let mut sch_pages = render_schematics(project, &work_a, &work_b, out, &renames)?;
        // Connectivity can only be compared when the project exists on both
        // revisions; otherwise the part diff already says everything.
        let nets = if root_a.is_file() && root_b.is_file() {
            let name = sanitize(project.label().as_ref());
            Some((
                netlist::export_netlist(&root_a, &work.join(format!("{name}-a.net")))?,
                netlist::export_netlist(&root_b, &work.join(format!("{name}-b.net")))?,
            ))
        } else {
            None
        };
        let (mut sch_changes, sheet_parts) = semantic::diff_schematics(
            &project.label(),
            &root_a,
            &root_b,
            nets.as_ref().map(|(a, b)| (a.as_slice(), b.as_slice())),
            &renames,
        )?;
        localize_sch_changes(&mut sch_changes, project, out);
        // Visually identical sheets earn a page only when a change (a pin
        // rewired from another sheet) needs somewhere to navigate to.
        let referenced: BTreeSet<_> = sch_changes
            .iter()
            .filter_map(|c| c.sheet.as_ref())
            .collect();
        sch_pages.retain(|p| p.status != Status::Unchanged || referenced.contains(&p.name));
        for page in &mut sch_pages {
            page.parts = sheet_parts.get(&page.name).copied();
        }
        pages.extend(sch_pages);
        changes.extend(sch_changes);
    }
    if pcb_changed {
        let mut pcb_pages = render_pcb(project, &work_a, &work_b, out)?;
        let (pcb_changes, layer_parts) = semantic::diff_pcb(
            &project.label(),
            &work_a.join(&pcb_rel),
            &work_b.join(&pcb_rel),
        )?;
        for page in &mut pcb_pages {
            page.parts = layer_parts.get(&page.name).copied();
        }
        pages.extend(pcb_pages);
        changes.extend(pcb_changes);
    }
    Ok((pages, changes))
}

/// Convert schematic change locations (sheet millimeters) into fractions of
/// the sheet's exported SVG extent, so the viewer can place markers. Sheet
/// SVGs cover the full page from the origin, so the fraction is just mm over
/// the viewBox size, read from each side's exported file (falling back to
/// the other side when a page only exists on one).
fn localize_sch_changes(changes: &mut [Change], project: &Project, out: &Path) {
    let folder = sanitize(project.label().as_ref());
    let mut sizes = BTreeMap::new();
    for change in changes {
        let Some(sheet) = change.sheet.clone() else {
            continue;
        };
        let file = if sheet.as_ref() == "Root sheet" {
            format!("{}.svg", project.name)
        } else {
            format!("{}-{}.svg", project.name, sheet)
        };
        let mut frac = |at: Option<[f64; 2]>, sides: [&'static str; 2]| {
            let [x, y] = at?;
            let size = sizes.entry((sheet.clone(), sides[0])).or_insert_with(|| {
                sides.iter().find_map(|side| {
                    svg_view_size(
                        &out.join(side)
                            .join("svg")
                            .join(&folder)
                            .join("sch")
                            .join(&file),
                    )
                })
            });
            let [w, h] = (*size)?;
            (w > 0.0 && h > 0.0).then(|| [x / w, y / h])
        };
        change.frac_base = frac(change.at_base, ["a", "b"]);
        change.frac_head = frac(change.at_head, ["b", "a"]);
    }
}

/// The width/height of an SVG's viewBox, from the file's leading bytes.
fn svg_view_size(path: &Path) -> Option<[f64; 2]> {
    let mut head = vec![0; 2048];
    let mut file = fs::File::open(path).ok()?;
    let n = std::io::Read::read(&mut file, &mut head).ok()?;
    let head = String::from_utf8_lossy(&head[..n]).into_owned();
    let rest = head.split("viewBox=\"").nth(1)?;
    let mut nums = rest
        .split('"')
        .next()?
        .split_ascii_whitespace()
        .filter_map(|v| v.parse::<f64>().ok());
    let (_, _, w, h) = (nums.next()?, nums.next()?, nums.next()?, nums.next()?);
    Some([w, h])
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
        // The project file must ride along: without it kicad-cli resolves
        // hierarchical connectivity into per-sheet net fragments, breaking
        // the netlist comparison.
        match path.extension().and_then(|e| e.to_str()) {
            Some("kicad_sch") | Some("kicad_pcb") | Some("kicad_pro") => {}
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

/// Diff every footprint library (a `*.pretty` directory of `*.kicad_mod`
/// files) touched between the two revisions. Only changed footprints are
/// materialized and exported; each becomes one page.
pub fn process_footprint_libs(
    repo: &gix::Repository,
    base: &[(PathBuf, ObjectId)],
    head: &[(PathBuf, ObjectId)],
    filter: Option<&Path>,
    out: &Path,
) -> Result<Vec<Page>> {
    // Library dir -> footprint file -> oid, for one side.
    let collect = |blobs: &[(PathBuf, ObjectId)]| {
        let mut libs: BTreeMap<_, BTreeMap<_, _>> = BTreeMap::new();
        for (path, oid) in blobs {
            if path.extension().and_then(|e| e.to_str()) != Some("kicad_mod") {
                continue;
            }
            if let Some(prefix) = filter
                && !path.starts_with(prefix)
            {
                continue;
            }
            let Some(dir) = path.parent() else { continue };
            if dir.extension().and_then(|e| e.to_str()) != Some("pretty") {
                continue;
            }
            libs.entry(dir.to_path_buf())
                .or_default()
                .insert(path.clone(), *oid);
        }
        libs
    };
    let libs_a = collect(base);
    let libs_b = collect(head);

    let mut pages = Vec::new();
    let lib_dirs: BTreeSet<_> = libs_a.keys().chain(libs_b.keys()).collect();
    for lib in lib_dirs {
        let empty = BTreeMap::new();
        let side_a = libs_a.get(lib).unwrap_or(&empty);
        let side_b = libs_b.get(lib).unwrap_or(&empty);
        // Footprints whose blob differs between the sides (or exists on one only).
        let changed = |ours: &BTreeMap<PathBuf, ObjectId>, theirs: &BTreeMap<PathBuf, ObjectId>| {
            ours.iter()
                .filter(|(p, o)| theirs.get(*p) != Some(o))
                .map(|(p, o)| (p.clone(), *o))
                .collect::<Vec<_>>()
        };
        let changed_a = changed(side_a, side_b);
        let changed_b = changed(side_b, side_a);
        if changed_a.is_empty() && changed_b.is_empty() {
            continue;
        }

        let label = lib.to_string_lossy().into_owned();
        let folder = sanitize(&label);
        let work = out.join(".work");
        let render = |side: &str, files: &[(PathBuf, ObjectId)]| -> Result<BTreeSet<String>> {
            let svg_dir = out.join(side).join("svg").join(&folder).join("fp");
            if files.is_empty() {
                return Ok(BTreeSet::new());
            }
            let lib_dir = work.join(side).join("fplib").join(lib);
            write_blobs(repo, files, &work.join(side).join("fplib"))?;
            export_library_svgs("fp", &lib_dir, &svg_dir)?;
            svg_files(&svg_dir)
        };
        let files_a = render("a", &changed_a)?;
        let files_b = render("b", &changed_b)?;
        pages.extend(library_pages(
            &label,
            Kind::Fp,
            &folder,
            out,
            &files_a,
            &files_b,
            |f| f.strip_suffix(".svg").unwrap_or(f).to_string(),
        )?);
    }
    Ok(pages)
}

/// Pages for the union of two sides' exported library SVGs, keeping only
/// files that differ visually; `name` derives the page title from a file.
fn library_pages(
    project: &str,
    kind: Kind,
    folder: &str,
    out: &Path,
    files_a: &BTreeSet<String>,
    files_b: &BTreeSet<String>,
    name: impl Fn(&str) -> String,
) -> Result<Vec<Page>> {
    let sub = kind.as_str();
    let mut pages = Vec::new();
    for file in files_a.union(files_b) {
        let side = |s: &str, present: bool| {
            present.then(|| out.join(s).join("svg").join(folder).join(sub).join(file))
        };
        let bp = side("a", files_a.contains(file));
        let hp = side("b", files_b.contains(file));
        if let Some(status) = classify(bp.as_deref(), hp.as_deref())? {
            pages.push(Page {
                project: project.into(),
                kind,
                name: name(file).into(),
                rel: format!("{folder}/{sub}/{file}"),
                status,
                edge_base: None,
                edge_head: None,
                parts: None,
            });
        }
    }
    Ok(pages)
}

/// Diff every symbol library (`*.kicad_sym`) touched between the two
/// revisions. The whole library is exported on both sides (one SVG per symbol
/// unit) and only visually changed units are kept.
pub fn process_symbol_libs(
    repo: &gix::Repository,
    base: &[(PathBuf, ObjectId)],
    head: &[(PathBuf, ObjectId)],
    filter: Option<&Path>,
    out: &Path,
) -> Result<Vec<Page>> {
    let collect = |blobs: &[(PathBuf, ObjectId)]| {
        blobs
            .iter()
            .filter(|(p, _)| {
                p.extension().and_then(|e| e.to_str()) == Some("kicad_sym")
                    && filter.is_none_or(|prefix| p.starts_with(prefix))
            })
            .map(|(p, o)| (p.clone(), *o))
            .collect::<BTreeMap<_, _>>()
    };
    let libs_a = collect(base);
    let libs_b = collect(head);

    let mut pages = Vec::new();
    let libs: BTreeSet<_> = libs_a.keys().chain(libs_b.keys()).collect();
    for lib in libs {
        let oid_a = libs_a.get(lib);
        let oid_b = libs_b.get(lib);
        if oid_a == oid_b {
            continue;
        }

        let label = lib.to_string_lossy().into_owned();
        let folder = sanitize(&label);
        let work = out.join(".work");
        let render = |side: &str, oid: Option<&ObjectId>| -> Result<BTreeSet<String>> {
            let Some(oid) = oid else {
                return Ok(BTreeSet::new());
            };
            let svg_dir = out.join(side).join("svg").join(&folder).join("sym");
            write_blobs(
                repo,
                &[(lib.clone(), *oid)],
                &work.join(side).join("symlib"),
            )?;
            export_library_svgs("sym", &work.join(side).join("symlib").join(lib), &svg_dir)?;
            svg_files(&svg_dir)
        };
        let files_a = render("a", oid_a)?;
        let files_b = render("b", oid_b)?;

        let union: BTreeSet<_> = files_a.union(&files_b).collect();
        pages.extend(library_pages(
            &label,
            Kind::Sym,
            &folder,
            out,
            &files_a,
            &files_b,
            |f| symbol_name(f, &union),
        )?);
    }
    Ok(pages)
}

/// Run `kicad-cli <fp|sym> export svg` on one library, into `svg_dir`.
fn export_library_svgs(kind: &str, library: &Path, svg_dir: &Path) -> Result<()> {
    fs::create_dir_all(svg_dir)?;
    run_kicad_cli(library, |c| {
        c.arg(kind)
            .args(["export", "svg", "-o"])
            .arg(svg_dir)
            .arg(library);
    })
}

/// Friendly symbol page name: `Foo_unit2.svg` -> `Foo unit 2`, with the unit
/// suffix dropped entirely for single-unit symbols (the common case).
fn symbol_name(file: &str, all: &BTreeSet<&String>) -> String {
    let stem = file.strip_suffix(".svg").unwrap_or(file);
    let Some((base, unit)) = stem.rsplit_once("_unit") else {
        return stem.to_string();
    };
    if !unit.chars().all(|c| c.is_ascii_digit()) {
        return stem.to_string();
    }
    let siblings = all
        .iter()
        .filter(|f| {
            f.strip_suffix(".svg")
                .and_then(|s| s.rsplit_once("_unit"))
                .is_some_and(|(b, u)| b == base && u.chars().all(|c| c.is_ascii_digit()))
        })
        .count();
    if unit == "1" && siblings == 1 {
        base.to_string()
    } else {
        format!("{base} unit {unit}")
    }
}

/// Write the given blobs to `dst`, preserving their repo-relative paths.
fn write_blobs(repo: &gix::Repository, blobs: &[(PathBuf, ObjectId)], dst: &Path) -> Result<()> {
    for (path, oid) in blobs {
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
/// file name (with `renames` translating base names to head ones), and keep
/// only those that changed.
fn render_schematics(
    project: &Project,
    work_a: &Path,
    work_b: &Path,
    out: &Path,
    renames: &BTreeMap<PageName, PageName>,
) -> Result<Vec<Page>> {
    let folder = sanitize(project.label().as_ref());
    let rel_dir = format!("{folder}/sch");

    let mut base = export_sch_side(
        project,
        work_a,
        &out.join("a").join("svg").join(&folder).join("sch"),
    )?;
    let head = export_sch_side(
        project,
        work_b,
        &out.join("b").join("svg").join(&folder).join("sch"),
    )?;

    // A renamed sheet exports under each side's own name; move the base
    // file to the head name so one rel path serves both revisions. Via a
    // temp name first, so sheets trading names don't collide mid-move.
    if !renames.is_empty() {
        let head_files: BTreeMap<PageName, &String> = head
            .keys()
            .map(|f| (sheet_name(f, &project.name).into(), f))
            .collect();
        let mut pending = Vec::new();
        for (file, path) in std::mem::take(&mut base) {
            let target = renames
                .get(&PageName::from(sheet_name(&file, &project.name)))
                .and_then(|n| head_files.get(n));
            match target {
                Some(new_file) => {
                    let tmp = path.with_extension("svg.tmp");
                    fs::rename(&path, &tmp)?;
                    pending.push((tmp, (*new_file).clone()));
                }
                None => {
                    base.insert(file, path);
                }
            }
        }
        for (tmp, file) in pending {
            let dest = tmp.with_file_name(&file);
            fs::rename(&tmp, &dest)?;
            base.insert(file, dest);
        }
    }

    // Order: root sheet first, then the rest alphabetically (case-insensitive).
    let order = |name: &str| {
        if name == "Root sheet" {
            (0u8, String::new())
        } else {
            (1, name.to_lowercase())
        }
    };

    let files: BTreeSet<_> = base.keys().chain(head.keys()).collect();
    let mut named: Vec<_> = files
        .into_iter()
        .map(|file| (sheet_name(file, &project.name), file))
        .collect();
    named.sort_by_key(|a| order(&a.0));

    let mut pages = Vec::new();
    for (name, file) in named {
        let bp = base.get(file).map(|p| p.as_path());
        let hp = head.get(file).map(|p| p.as_path());
        // Unchanged sheets stay too: the caller keeps the ones semantic
        // changes point at, so the viewer has a page to navigate to.
        let status = classify(bp, hp)?.unwrap_or(Status::Unchanged);
        pages.push(Page {
            project: project.label(),
            kind: Kind::Sch,
            name: name.into(),
            rel: format!("{rel_dir}/{file}"),
            status,
            edge_base: None,
            edge_head: None,
            parts: None,
        });
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
    run_kicad_cli(&root, |c| {
        c.args(["sch", "export", "svg", "--no-background-color", "-o"])
            .arg(svg_dir)
            .arg(&root);
    })?;
    list_svgs(svg_dir)
}

/// Export each side's board in one pass, pair layers by name, and keep only
/// those that changed. Edge.Cuts is rendered too and overlaid in the viewer as
/// board-outline context (rather than baked into every layer, which would make
/// any outline change flip every layer).
fn render_pcb(project: &Project, work_a: &Path, work_b: &Path, out: &Path) -> Result<Vec<Page>> {
    let folder = sanitize(project.label().as_ref());
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

    // Edge.Cuts overlay context, one per side so each revision is shown with
    // its own outline.
    let edge_file = format!("{prefix}Edge_Cuts.svg");
    let edge_base = base_files
        .contains(&edge_file)
        .then(|| format!("a/svg/{folder}/pcb/{edge_file}"));
    let edge_head = head_files
        .contains(&edge_file)
        .then(|| format!("b/svg/{folder}/pcb/{edge_file}"));

    // Union of produced files, in physical stackup order.
    let mut files: Vec<_> = base_files.union(&head_files).cloned().collect();
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
                edge_base: (file != edge_file).then(|| edge_base.clone()).flatten(),
                edge_head: (file != edge_file).then(|| edge_head.clone()).flatten(),
                name: label_of(&file).into(),
                status,
                parts: None,
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
    run_kicad_cli(pcb, |c| {
        c.args([
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
        .arg(pcb);
    })
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

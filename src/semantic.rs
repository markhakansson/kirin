//! Semantic diffing of KiCAD projects: parts, properties and connectivity.

mod connectivity;
mod extract;
#[cfg(test)]
mod fixtures;
mod geometry;
mod part;

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::Result;

use crate::ids::{EntityId, PageName, ProjectLabel, Reference};
use crate::kicad::Kind;
use crate::netlist::Net;

use connectivity::diff_connectivity;
use extract::{Entity, pcb_data, sch_entities, sheet_names};
use geometry::Bbox;
use part::diff_entities;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ChangeKind {
    Added,
    Removed,
    Renamed,
    Moved,
    Flipped,
    ValueChanged,
    FootprintChanged,
    PropertyChanged,
    NetChanged,
}

impl ChangeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ChangeKind::Added => "added",
            ChangeKind::Removed => "removed",
            ChangeKind::Renamed => "renamed",
            ChangeKind::Moved => "moved",
            ChangeKind::Flipped => "flipped",
            ChangeKind::ValueChanged => "value",
            ChangeKind::FootprintChanged => "footprint",
            ChangeKind::PropertyChanged => "property",
            ChangeKind::NetChanged => "net",
        }
    }
}

/// One semantic difference between the two revisions of a project.
pub struct Change {
    /// Sidebar group this belongs to.
    pub project: ProjectLabel,
    /// `Kind::Pcb` or `Kind::Sch`.
    pub scope: Kind,
    /// Page name of the schematic sheet the part sits on; `None` for PCB scope.
    pub sheet: Option<PageName>,
    pub kind: ChangeKind,
    /// Reference designator (head side when it exists, base side otherwise).
    pub reference: Reference,
    /// Human-facing summary ("10k -> 4k7", "moved 2.31 mm", ...).
    pub detail: String,
    /// For PCB scope: the copper layer the part sits on ("F.Cu"/"B.Cu"),
    /// which is the page the viewer navigates to.
    pub layer: Option<PageName>,
    /// Location in millimeters on each revision. A part can sit at different
    /// spots per side (or exist on only one), and the viewer shows whichever
    /// matches the revision being displayed.
    pub(crate) at_base: Option<[f64; 2]>,
    pub(crate) at_head: Option<[f64; 2]>,
    /// The locations as fractions of each side's rendered extent.
    pub frac_base: Option<[f64; 2]>,
    pub frac_head: Option<[f64; 2]>,
}

/// Diff the footprints of two board files by UUID. Change locations are
/// mapped into fractions of each side's board bounding box, matching the
/// `--fit-page-to-board` extent the layer SVGs are plotted with. Also
/// returns the part total per copper layer.
pub fn diff_pcb(
    project: &ProjectLabel,
    pcb_a: &Path,
    pcb_b: &Path,
) -> Result<(Vec<Change>, BTreeMap<PageName, usize>)> {
    let (a, bbox_a) = pcb_data(pcb_a)?;
    let (b, bbox_b) = pcb_data(pcb_b)?;
    let to_frac = |at: Option<[f64; 2]>, bbox: Option<Bbox>| {
        let ([x, y], [minx, miny, maxx, maxy]) = (at?, bbox?);
        (maxx > minx && maxy > miny)
            .then(|| [(x - minx) / (maxx - minx), (y - miny) / (maxy - miny)])
    };
    let mut changes = diff_entities(project, Kind::Pcb, &a, &b);
    for change in &mut changes {
        change.frac_base = to_frac(change.at_base, bbox_a);
        change.frac_head = to_frac(change.at_head, bbox_b);
    }
    Ok((changes, part_totals(&a, &b, |e| e.layer.as_ref())))
}

/// Returns a map with the old/new sheet names, for all sheets that have been
/// renamed between the commits. Sheets pair by their instance UUIDs, which
/// survive both a new display name and a renamed file; anything ambiguous
/// stays out and keeps the added/removed report.
pub fn sheet_renames(root_a: &Path, root_b: &Path) -> Result<BTreeMap<PageName, PageName>> {
    let a = sheet_names(root_a)?;
    let b = sheet_names(root_b)?;
    let mut candidates: BTreeMap<&PageName, BTreeSet<&PageName>> = BTreeMap::new();
    // Base names some instance of which keeps its name (or has no head
    // counterpart): their exported page still owns that name on the base
    // side, so no rename may source from or land on it.
    let mut kept = BTreeSet::new();
    for (key, old) in &a {
        match b.get(key) {
            Some(new) if old != new => {
                candidates.entry(old).or_default().insert(new);
            }
            _ => {
                kept.insert(old);
            }
        }
    }
    let unambiguous = candidates
        .into_iter()
        .filter(|(old, news)| news.len() == 1 && !kept.contains(*old))
        .map(|(old, news)| (old.clone(), (*news.first().unwrap()).clone()));
    let mut renames: BTreeMap<PageName, PageName> = unambiguous.collect();
    let mut targets: BTreeMap<PageName, usize> = BTreeMap::new();
    for new in renames.values() {
        *targets.entry(new.clone()).or_default() += 1;
    }
    renames.retain(|_, new| targets[new] == 1 && !kept.contains(new));
    Ok(renames)
}

/// Diff the symbols of two schematic hierarchies by UUID, walking sub-sheets,
/// plus electrical connectivity when both sides' netlists are given.
/// `root_a`/`root_b` are the materialized root `.kicad_sch` files. Also
/// returns the part total per sheet.
pub fn diff_schematics(
    project: &ProjectLabel,
    root_a: &Path,
    root_b: &Path,
    nets: Option<(&[Net], &[Net])>,
    renames: &BTreeMap<PageName, PageName>,
) -> Result<(Vec<Change>, BTreeMap<PageName, usize>)> {
    let mut a = sch_entities(root_a)?;
    // If pages were renamed, use the new page name for the old change. This
    // will make it easier to diff.
    for e in a.values_mut() {
        if let Some(new) = e.sheet.as_ref().and_then(|s| renames.get(s)) {
            e.sheet = Some(new.clone());
        }
    }
    let b = sch_entities(root_b)?;
    let mut changes = diff_entities(project, Kind::Sch, &a, &b);
    if let Some((nets_a, nets_b)) = nets {
        changes.extend(diff_connectivity(project, nets_a, nets_b, &a, &b));
        changes.sort_by(|l, r| (&l.sheet, &l.reference).cmp(&(&r.sheet, &r.reference)));
    }
    Ok((changes, part_totals(&a, &b, |e| e.sheet.as_ref())))
}

/// Unique references per page (sheet or copper layer), union of both sides:
/// the denominator behind the viewer's "% of parts changed" summaries.
fn part_totals(
    a: &BTreeMap<EntityId, Entity>,
    b: &BTreeMap<EntityId, Entity>,
    page: impl Fn(&Entity) -> Option<&PageName>,
) -> BTreeMap<PageName, usize> {
    let mut refs: BTreeMap<_, BTreeSet<_>> = BTreeMap::new();
    for e in a.values().chain(b.values()) {
        if let Some(p) = page(e) {
            refs.entry(p).or_default().insert(&e.reference);
        }
    }
    refs.into_iter()
        .map(|(page, refs)| (page.clone(), refs.len()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::fixtures::{entities, instance};
    use super::*;

    /// Write a root schematic holding only sheet blocks; the sub files need
    /// not exist for the hierarchy walk to see names and UUIDs.
    fn sheet_root(label: &str, sheets: &[(&str, &str, &str)]) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let body: String = sheets
            .iter()
            .map(|(uuid, name, file)| {
                format!(
                    r#"(sheet (at 0 0) (size 10 10) (uuid "{uuid}")
                      (property "Sheetname" "{name}" (at 0 0 0))
                      (property "Sheetfile" "{file}" (at 0 0 0)))"#
                )
            })
            .collect();
        let path = std::env::temp_dir().join(format!("kirin_ren_{label}_{nanos}.kicad_sch"));
        std::fs::write(
            &path,
            format!(
                r#"(kicad_sch (version 20250114) (generator "eeschema") (uuid "root") {body})"#
            ),
        )
        .unwrap();
        path
    }

    #[test]
    fn sheet_renames_pair_by_instance_uuid() {
        // s1 renames (file and all), s2 renames its display name only, s3
        // keeps its name, and s4 lands on a name s3 still owns - ambiguous,
        // so it stays out.
        let a = sheet_root(
            "a",
            &[
                ("s1", "48V to 5V", "48v_to_5v.kicad_sch"),
                ("s2", "5V to 3.3V", "dcdc_ref.kicad_sch"),
                ("s3", "Port A", "port.kicad_sch"),
                ("s4", "Port B", "port.kicad_sch"),
            ],
        );
        let b = sheet_root(
            "b",
            &[
                ("s1", "48V to 4.2V", "48v_to_4.2v.kicad_sch"),
                ("s2", "4.2V to 3.3V", "dcdc_ref.kicad_sch"),
                ("s3", "Port A", "port.kicad_sch"),
                ("s4", "Port A", "port.kicad_sch"),
            ],
        );
        let renames = sheet_renames(&a, &b).unwrap();
        let _ = std::fs::remove_file(a);
        let _ = std::fs::remove_file(b);

        let pairs: Vec<_> = renames
            .iter()
            .map(|(o, n)| (o.as_ref(), n.as_ref()))
            .collect();
        assert_eq!(
            pairs,
            [("48V to 5V", "48V to 4.2V"), ("5V to 3.3V", "4.2V to 3.3V"),]
        );
    }

    #[test]
    fn part_totals_count_unique_refs_per_page() {
        let a = entities(&[
            instance("R1", "Lib:Box", "10 10 0", 1),
            instance("R2", "Lib:Box", "30 10 0", 1),
        ]);
        let b = entities(&[
            instance("R2", "Lib:Box", "30 10 0", 1),
            instance("R3", "Lib:Box", "50 10 0", 1),
        ]);
        // Union of both sides, each reference once.
        let totals = part_totals(&a, &b, |e| e.sheet.as_ref());
        assert_eq!(totals.get(&PageName::from("Root sheet")), Some(&3));
    }
}

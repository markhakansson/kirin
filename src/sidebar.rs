//! Contains all data to be displayed in the sidebar.
//!
//! The sidebar only needs to be rendered in the frontend, the contents are
//! static. Thus the contents can be pre-generated, with ordering and all.

use std::collections::BTreeSet;

use crate::ids::{PageName, ProjectLabel, Reference};
use crate::kicad::{Kind, Page, Status};
use crate::semantic::{Change, ChangeKind};

/// Content for the sidebar in the order to be rendered.
pub struct Sidebar {
    /// List of projects.
    pub projects: Vec<ProjectGroup>,
    /// Change groups for the pages that have changes, in page order.
    pub groups: Vec<ChangeGroup>,
    /// Change indices in sidebar order: what `n`/`p` step through.
    pub order: Vec<usize>,
}

impl Sidebar {
    /// Create the data needed to render the sidebar.
    pub fn new(pages: &[Page], changes: &[Change]) -> Sidebar {
        let by_page = changes_by_page(pages, changes);

        // Group by project, then by kind, preserving the order pages arrive in.
        let mut projects: Vec<ProjectGroup> = Vec::new();
        for (i, page) in pages.iter().enumerate() {
            let project = match projects.iter().position(|p| p.project == page.project) {
                Some(at) => &mut projects[at],
                None => {
                    projects.push(ProjectGroup {
                        project: page.project.clone(),
                        kinds: Vec::new(),
                    });
                    projects.last_mut().expect("just pushed")
                }
            };
            match project.kinds.iter_mut().find(|k| k.kind == page.kind) {
                Some(kind) => kind.pages.push(i),
                None => project.kinds.push(KindGroup {
                    kind: page.kind,
                    pages: vec![i],
                }),
            }
        }

        let mut groups = Vec::new();
        let mut order = Vec::new();
        for project in &projects {
            for kind in &project.kinds {
                for &page in &kind.pages {
                    let idxs = &by_page[page];
                    if idxs.is_empty() {
                        continue;
                    }
                    let group = change_group(page, &pages[page], idxs, changes);
                    order.extend(
                        group
                            .parts
                            .iter()
                            .flat_map(|p| p.rows.iter().map(|r| r.change)),
                    );
                    groups.push(group);
                }
            }
        }

        Sidebar {
            projects,
            groups,
            order,
        }
    }
}

pub struct ProjectGroup {
    pub project: ProjectLabel,
    pub kinds: Vec<KindGroup>,
}

pub struct KindGroup {
    pub kind: Kind,
    /// Indices into the page list.
    pub pages: Vec<usize>,
}

/// The collapsible group of semantic changes under one page row.
pub struct ChangeGroup {
    pub page: usize,
    pub count: usize,
    /// The line that expands the group ("5 changes · 38% of parts changed").
    pub summary: String,
    pub parts: Vec<PartGroup>,
}

/// One part's changes: a header row carrying the reference, its changes beneath.
pub struct PartGroup {
    pub reference: Reference,
    pub status: Status,
    pub rows: Vec<Row>,
}

pub struct Row {
    /// Index into the change list.
    pub change: usize,
    /// Badge class, or `None` when the header badge already says it.
    pub badge: Option<&'static str>,
}

fn badge_for(kind: ChangeKind, status: Status) -> Option<&'static str> {
    // An added part's rows are all additions; no point repeating the header.
    if kind.as_str() == status.as_str() {
        return None;
    }
    Some(match kind {
        ChangeKind::Added => "added",
        ChangeKind::Removed => "removed",
        ChangeKind::NetChanged => "net",
        // The rest share the "modified" colors.
        _ => "modified",
    })
}

/// How early a change sorts within its page: the ones that move no pixel come
/// first, since they are the ones a visual diff cannot show you.
fn rank(kind: ChangeKind) -> u8 {
    match kind {
        ChangeKind::PropertyChanged => 0,
        ChangeKind::NetChanged => 1,
        _ => 2,
    }
}

/// The page a change lives under, which is also where clicking it navigates:
/// the sheet for schematic changes, the part's copper layer for board changes
/// (or the board's first page when that layer is not in the report).
fn home(change: &Change, pages: &[Page]) -> Option<usize> {
    let on = |page: &Page, kind: Kind, name: Option<&PageName>| {
        page.project == change.project && page.kind == kind && name.is_some_and(|n| &page.name == n)
    };
    if change.scope == Kind::Sch {
        return pages
            .iter()
            .position(|p| on(p, Kind::Sch, change.sheet.as_ref()));
    }
    pages
        .iter()
        .position(|p| on(p, Kind::Pcb, change.layer.as_ref()))
        .or_else(|| {
            pages
                .iter()
                .position(|p| p.project == change.project && p.kind == Kind::Pcb)
        })
}

/// Changes per page, sneakiest first, then by original order so ties are stable.
fn changes_by_page(pages: &[Page], changes: &[Change]) -> Vec<Vec<usize>> {
    let mut by_page = vec![Vec::new(); pages.len()];
    for (i, change) in changes.iter().enumerate() {
        if let Some(page) = home(change, pages) {
            by_page[page].push(i);
        }
    }
    for idxs in &mut by_page {
        idxs.sort_by_key(|&i| (rank(changes[i].kind), i));
    }
    by_page
}

/// The percentage counts unique references with part changes against the parts
/// on the page; net-only groups get none (nets have no such denominator).
fn change_group(page: usize, entry: &Page, idxs: &[usize], changes: &[Change]) -> ChangeGroup {
    let changed_refs: BTreeSet<_> = idxs
        .iter()
        .map(|&i| &changes[i])
        .filter(|c| c.kind != ChangeKind::NetChanged)
        .map(|c| &c.reference)
        .collect();

    let mut summary = match idxs.len() {
        1 => "1 change".to_string(),
        n => format!("{n} changes"),
    };
    if let Some(total) = entry.parts
        && total > 0
        && !changed_refs.is_empty()
    {
        // Board changes falling back to this page (their own layer is not in
        // the report) can push the count past this page's own part total.
        let pct = ((100.0 * changed_refs.len() as f64) / total as f64).round() as usize;
        summary.push_str(&format!(" · {}% of parts changed", pct.min(100)));
    }

    // `idxs` arrives sneakiest-first, so grouping in encounter order ranks each
    // part by its sneakiest change and keeps that order within the part too.
    let mut parts: Vec<PartGroup> = Vec::new();
    for &i in idxs {
        let change = &changes[i];
        let part = match parts.iter().position(|p| p.reference == change.reference) {
            Some(at) => &mut parts[at],
            None => {
                parts.push(PartGroup {
                    reference: change.reference.clone(),
                    status: Status::Modified,
                    rows: Vec::new(),
                });
                parts.last_mut().expect("just pushed")
            }
        };
        match change.kind {
            ChangeKind::Added => part.status = Status::Added,
            ChangeKind::Removed => part.status = Status::Removed,
            _ => {}
        }
        part.rows.push(Row {
            change: i,
            badge: None, // needs the final status, filled in below
        });
    }
    for part in &mut parts {
        for row in &mut part.rows {
            row.badge = badge_for(changes[row.change].kind, part.status);
        }
    }

    ChangeGroup {
        page,
        count: idxs.len(),
        summary,
        parts,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(project: &str, kind: Kind, name: &str, parts: Option<usize>) -> Page {
        Page {
            project: project.into(),
            kind,
            name: name.into(),
            rel: format!("{project}/{name}.svg"),
            status: Status::Modified,
            edge_base: None,
            edge_head: None,
            parts,
        }
    }

    fn change(project: &str, scope: Kind, on: &str, kind: ChangeKind, reference: &str) -> Change {
        let (sheet, layer) = match scope {
            Kind::Sch => (Some(on.into()), None),
            _ => (None, Some(on.into())),
        };
        Change {
            project: project.into(),
            scope,
            sheet,
            layer,
            kind,
            reference: reference.into(),
            detail: String::new(),
            at_base: None,
            at_head: None,
            frac_base: None,
            frac_head: None,
        }
    }

    #[test]
    fn groups_pages_by_project_then_kind_in_report_order() {
        let pages = vec![
            page("a", Kind::Sch, "Root", None),
            page("b", Kind::Sch, "Root", None),
            page("a", Kind::Pcb, "F.Cu", None),
            page("a", Kind::Sch, "Power", None),
        ];
        let sidebar = Sidebar::new(&pages, &[]);
        let shape: Vec<_> = sidebar
            .projects
            .iter()
            .map(|p| {
                let kinds: Vec<_> = p
                    .kinds
                    .iter()
                    .map(|k| (k.kind.as_str(), k.pages.clone()))
                    .collect();
                (p.project.as_ref().to_string(), kinds)
            })
            .collect();
        // A later page rejoins the group opened earlier rather than starting a new one.
        assert_eq!(
            shape,
            vec![
                ("a".to_string(), vec![("sch", vec![0, 3]), ("pcb", vec![2])]),
                ("b".to_string(), vec![("sch", vec![1])]),
            ]
        );
    }

    #[test]
    fn schematic_changes_home_to_their_sheet() {
        let pages = vec![
            page("a", Kind::Sch, "Root", None),
            page("a", Kind::Sch, "Power", None),
        ];
        let changes = vec![
            change("a", Kind::Sch, "Power", ChangeKind::Moved, "R1"),
            change("a", Kind::Sch, "Root", ChangeKind::Moved, "R2"),
            change("a", Kind::Sch, "Missing", ChangeKind::Moved, "R3"),
            change("b", Kind::Sch, "Root", ChangeKind::Moved, "R4"),
        ];
        let sidebar = Sidebar::new(&pages, &changes);
        let homed: Vec<_> = sidebar.groups.iter().map(|g| (g.page, g.count)).collect();
        assert_eq!(homed, vec![(0, 1), (1, 1)]);
        // Changes with nowhere to hang are dropped, not misfiled.
        assert_eq!(sidebar.order, vec![1, 0]);
    }

    #[test]
    fn board_changes_fall_back_to_the_first_pcb_page() {
        let pages = vec![
            page("a", Kind::Pcb, "F.Cu", None),
            page("a", Kind::Pcb, "F.SilkS", None),
        ];
        let changes = vec![
            change("a", Kind::Pcb, "F.Cu", ChangeKind::Moved, "R1"),
            change("a", Kind::Pcb, "B.Cu", ChangeKind::Moved, "R2"),
        ];
        let sidebar = Sidebar::new(&pages, &changes);
        assert_eq!(sidebar.groups.len(), 1);
        assert_eq!(sidebar.groups[0].page, 0);
        assert_eq!(sidebar.groups[0].count, 2);
    }

    #[test]
    fn sneaky_changes_sort_first_across_parts_and_within_one() {
        let pages = vec![page("a", Kind::Sch, "Root", None)];
        let changes = vec![
            change("a", Kind::Sch, "Root", ChangeKind::Moved, "R1"),
            change("a", Kind::Sch, "Root", ChangeKind::NetChanged, "R2"),
            change("a", Kind::Sch, "Root", ChangeKind::PropertyChanged, "R1"),
            change("a", Kind::Sch, "Root", ChangeKind::ValueChanged, "R2"),
        ];
        let sidebar = Sidebar::new(&pages, &changes);
        let parts: Vec<_> = sidebar.groups[0]
            .parts
            .iter()
            .map(|p| {
                let rows: Vec<_> = p.rows.iter().map(|r| r.change).collect();
                (p.reference.as_ref().to_string(), rows)
            })
            .collect();
        // R1 leads on its property change even though its move came first in
        // the input, and keeps that change first within the part.
        assert_eq!(
            parts,
            vec![
                ("R1".to_string(), vec![2, 0]),
                ("R2".to_string(), vec![1, 3]),
            ]
        );
        // The walk order matches what is on screen, top to bottom.
        assert_eq!(sidebar.order, vec![2, 0, 1, 3]);
    }

    #[test]
    fn part_status_comes_from_its_changes_and_suppresses_repeat_badges() {
        let pages = vec![page("a", Kind::Sch, "Root", None)];
        let changes = vec![
            change("a", Kind::Sch, "Root", ChangeKind::Added, "R1"),
            change("a", Kind::Sch, "Root", ChangeKind::ValueChanged, "R1"),
            change("a", Kind::Sch, "Root", ChangeKind::Moved, "R2"),
            change("a", Kind::Sch, "Root", ChangeKind::NetChanged, "R2"),
        ];
        let sidebar = Sidebar::new(&pages, &changes);
        let shape: Vec<_> = sidebar.groups[0]
            .parts
            .iter()
            .map(|p| {
                let badges: Vec<_> = p.rows.iter().map(|r| r.badge).collect();
                (p.status.as_str(), badges)
            })
            .collect();
        assert_eq!(
            shape,
            vec![
                // R2's net change ranks it first.
                ("modified", vec![Some("net"), Some("modified")]),
                ("added", vec![None, Some("modified")]),
            ]
        );
    }

    #[test]
    fn summary_counts_parts_not_changes_and_ignores_nets() {
        let pages = vec![page("a", Kind::Sch, "Root", Some(8))];
        let changes = vec![
            change("a", Kind::Sch, "Root", ChangeKind::Moved, "R1"),
            change("a", Kind::Sch, "Root", ChangeKind::ValueChanged, "R1"),
            change("a", Kind::Sch, "Root", ChangeKind::Moved, "R2"),
            change("a", Kind::Sch, "Root", ChangeKind::NetChanged, "R9"),
        ];
        let sidebar = Sidebar::new(&pages, &changes);
        // 4 changes over 2 changed parts of 8; R9's net does not count.
        assert_eq!(
            sidebar.groups[0].summary,
            "4 changes · 25% of parts changed"
        );
    }

    #[test]
    fn summary_without_a_part_total_is_a_bare_count() {
        let pages = vec![
            page("a", Kind::Sch, "Root", None),
            page("a", Kind::Sch, "Power", Some(0)),
        ];
        let changes = vec![
            change("a", Kind::Sch, "Root", ChangeKind::Moved, "R1"),
            change("a", Kind::Sch, "Power", ChangeKind::Moved, "R1"),
        ];
        let sidebar = Sidebar::new(&pages, &changes);
        assert_eq!(sidebar.groups[0].summary, "1 change");
        // A zero total would divide by nothing; no percentage rather than 100%.
        assert_eq!(sidebar.groups[1].summary, "1 change");
    }

    #[test]
    fn net_only_groups_get_no_percentage() {
        let pages = vec![page("a", Kind::Sch, "Root", Some(4))];
        let changes = vec![change("a", Kind::Sch, "Root", ChangeKind::NetChanged, "R1")];
        let sidebar = Sidebar::new(&pages, &changes);
        assert_eq!(sidebar.groups[0].summary, "1 change");
    }

    #[test]
    fn percentage_is_capped_when_changes_fall_back_onto_a_page() {
        let pages = vec![page("a", Kind::Pcb, "F.Cu", Some(1))];
        let changes = vec![
            change("a", Kind::Pcb, "B.Cu", ChangeKind::Moved, "R1"),
            change("a", Kind::Pcb, "B.Cu", ChangeKind::Moved, "R2"),
        ];
        let sidebar = Sidebar::new(&pages, &changes);
        assert_eq!(
            sidebar.groups[0].summary,
            "2 changes · 100% of parts changed"
        );
    }
}

//! Part-level diff: matching by UUID, re-placement pairing, per-fact rows.

use std::collections::{BTreeMap, BTreeSet};

use crate::ids::{EntityId, ProjectLabel, Reference};
use crate::kicad::Kind;

use super::extract::Entity;
use super::{Change, ChangeKind};

/// Position delta below which a footprint is not reported as moved.
const MOVE_EPSILON_MM: f64 = 0.01;

struct Diff<'a> {
    project: &'a ProjectLabel,
    scope: Kind,
    changes: Vec<Change>,
}

impl Diff<'_> {
    /// `x` is the part on the base side, `y` on the head side; either may be
    /// absent, and their positions per side are both kept for the marker.
    fn push(
        &mut self,
        kind: ChangeKind,
        x: Option<&Entity>,
        y: Option<&Entity>,
        reference: &Reference,
        detail: String,
    ) {
        self.changes.push(Change {
            project: self.project.clone(),
            scope: self.scope,
            sheet: y.or(x).and_then(|e| e.sheet.clone()),
            kind,
            reference: reference.clone(),
            detail,
            layer: y.or(x).and_then(|e| e.layer.clone()),
            at_base: x.and_then(|e| e.at),
            at_head: y.and_then(|e| e.at),
            frac_base: None,
            frac_head: None,
        });
    }

    /// Differences between two revisions of the same part. Also used for
    /// same-reference re-placements, so the ref never differs there.
    /// Each fact is reported in the domain that owns it: electrical/BOM
    /// changes (value) on the schematic, placement on the board.
    fn compare(&mut self, x: &Entity, y: &Entity) {
        if x.reference != y.reference {
            self.push(
                ChangeKind::Renamed,
                Some(x),
                Some(y),
                &y.reference,
                format!("Renamed: {} -> {}", x.reference, y.reference),
            );
        }
        if self.scope == Kind::Sch && x.value != y.value {
            self.push(
                ChangeKind::ValueChanged,
                Some(x),
                Some(y),
                &y.reference,
                format!(
                    "Value: {} -> {}",
                    x.value.as_deref().unwrap_or("-"),
                    y.value.as_deref().unwrap_or("-")
                ),
            );
        }
        if x.lib_id != y.lib_id {
            self.push(
                ChangeKind::FootprintChanged,
                Some(x),
                Some(y),
                &y.reference,
                format!(
                    "Library: {} -> {}",
                    x.lib_id.as_deref().unwrap_or("-"),
                    y.lib_id.as_deref().unwrap_or("-")
                ),
            );
        }
        if self.scope == Kind::Sch {
            let keys: BTreeSet<_> = x.properties.keys().chain(y.properties.keys()).collect();
            for key in keys {
                let from = x.properties.get(key);
                let to = y.properties.get(key);
                if from == to {
                    continue;
                }
                // A field that disappears together with a symbol swap left
                // with the old symbol; the swap row is the parent of that.
                // Deleting a field on the same symbol is its own edit.
                if to.is_none() && x.lib_id != y.lib_id {
                    continue;
                }
                self.push(
                    ChangeKind::PropertyChanged,
                    Some(x),
                    Some(y),
                    &y.reference,
                    format!(
                        "{key}: {} -> {}",
                        from.map_or("-", |v| v),
                        to.map_or("-", |v| v)
                    ),
                );
            }
        }
        if self.scope == Kind::Pcb {
            let mut motion = Vec::new();
            if let (Some(pa), Some(pb)) = (x.at, y.at) {
                let dist = ((pa[0] - pb[0]).powi(2) + (pa[1] - pb[1]).powi(2)).sqrt();
                if dist > MOVE_EPSILON_MM {
                    motion.push(format!("moved {dist:.2} mm"));
                }
                if (x.rotation - y.rotation).abs() > f64::EPSILON {
                    motion.push(format!("rotated to {}\u{b0}", y.rotation));
                }
            }
            if x.layer != y.layer
                && let (Some(from), Some(to)) = (x.layer.as_ref(), y.layer.as_ref())
            {
                // A side swap concerns both copper layers, so it gets a row
                // under each: one on the layer the part left, marking where
                // it was on the base revision, and one on the layer it landed
                // on, marking where it is on the head revision.
                let detail = |dir: String| {
                    std::iter::once(dir)
                        .chain(motion.iter().cloned())
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                self.push(
                    ChangeKind::Flipped,
                    Some(x),
                    None,
                    &y.reference,
                    detail(format!("Flipped to {to}")),
                );
                self.push(
                    ChangeKind::Flipped,
                    None,
                    Some(y),
                    &y.reference,
                    detail(format!("Flipped from {from}")),
                );
            } else if !motion.is_empty() {
                // The pieces stay lowercase for the flipped rows above, where
                // they trail the direction; leading a row they get a capital.
                let mut detail = motion.join(", ");
                if let Some(first) = detail.get_mut(0..1) {
                    first.make_ascii_uppercase();
                }
                self.push(ChangeKind::Moved, Some(x), Some(y), &y.reference, detail);
            }
        }
    }
}

/// Everything a fresh part brings, one line per fact - the added row is the
/// only place to see what actually arrived. A removed part needs no such
/// inventory (the base revision still shows it), so its detail stays empty.
fn describe(e: &Entity) -> String {
    let mut lines = Vec::new();
    if let Some(v) = e.value.as_deref().filter(|v| !v.is_empty()) {
        lines.push(format!("Value: {v}"));
    }
    if let Some(l) = e.lib_id.as_deref().filter(|l| !l.is_empty()) {
        lines.push(format!("Library: {l}"));
    }
    for (k, v) in &e.properties {
        if !v.is_empty() {
            lines.push(format!("{k}: {v}"));
        }
    }
    lines.join("\n")
}

pub(super) fn diff_entities(
    project: &ProjectLabel,
    scope: Kind,
    a: &BTreeMap<EntityId, Entity>,
    b: &BTreeMap<EntityId, Entity>,
) -> Vec<Change> {
    let mut diff = Diff {
        project,
        scope,
        changes: Vec::new(),
    };

    let mut added = Vec::new();
    let mut removed = Vec::new();
    let uuids: BTreeSet<_> = a.keys().chain(b.keys()).collect();
    for uuid in uuids {
        match (a.get(uuid), b.get(uuid)) {
            (None, Some(e)) => added.push(e),
            (Some(e), None) => removed.push(e),
            (Some(x), Some(y)) => diff.compare(x, y),
            (None, None) => unreachable!(),
        }
    }

    // Re-placing a part gives it a fresh UUID even though it is the same
    // part to the user; pair up same-reference removals and additions and
    // report their real differences instead of a remove/add couple.
    let mut removed_by_ref: BTreeMap<_, Vec<_>> = BTreeMap::new();
    for e in removed {
        removed_by_ref.entry(&e.reference).or_default().push(e);
    }
    for e in added {
        match removed_by_ref.get_mut(&e.reference) {
            Some(old) if old.len() == 1 => {
                let old = old.pop().unwrap();
                diff.compare(old, e);
            }
            _ => diff.push(ChangeKind::Added, None, Some(e), &e.reference, describe(e)),
        }
    }
    for e in removed_by_ref.into_values().flatten() {
        diff.push(
            ChangeKind::Removed,
            Some(e),
            None,
            &e.reference,
            String::new(),
        );
    }

    let mut changes = diff.changes;
    // A multi-unit symbol places one entity per unit, each repeating the
    // part-level facts (value, footprint, properties); say each once. The
    // sort must cover the whole dedup key: units interleave their rows, and
    // dedup only removes adjacent equals.
    changes.sort_by(|l, r| {
        (&l.sheet, &l.reference, l.kind.as_str(), &l.detail).cmp(&(
            &r.sheet,
            &r.reference,
            r.kind.as_str(),
            &r.detail,
        ))
    });
    changes.dedup_by(|l, r| {
        (&l.sheet, &l.reference, l.kind, &l.detail) == (&r.sheet, &r.reference, r.kind, &r.detail)
    });
    changes
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::{entities, instance};
    use super::*;

    fn footprint(reference: &str, layer: &str, at: [f64; 2], rotation: f64) -> Entity {
        Entity {
            reference: reference.into(),
            value: None,
            lib_id: None,
            at: Some(at),
            rotation,
            sheet: None,
            layer: Some(layer.into()),
            properties: BTreeMap::new(),
            pins: BTreeMap::new(),
            hidden_pins: BTreeSet::new(),
        }
    }

    /// Insert an extra property into an `instance()` symbol.
    fn with_property(symbol: &str, key: &str, value: &str) -> String {
        format!(
            "{}\n(property \"{key}\" \"{value}\" (at 0 0 0)))",
            &symbol[..symbol.len() - 1]
        )
    }

    #[test]
    fn property_changes_hint_at_a_different_part() {
        let build = |mpn: &str, datasheet: &str, extra: &[(&str, &str)]| {
            let mut sym = instance("U1", "Lib:Box", "10 10 0", 1);
            sym = with_property(&sym, "MPN", mpn);
            sym = with_property(&sym, "Datasheet", datasheet);
            for (k, v) in extra {
                sym = with_property(&sym, k, v);
            }
            entities(&[sym])
        };
        let a = build("LM339", "http://old", &[]);
        // The filled-in LCSC field reports; the documentation-only datasheet
        // edit and the empty addition stay silent.
        let b = build("LM339LV", "http://new", &[("LCSC", "C123"), ("Empty", "")]);
        let changes = diff_entities(&"p".into(), Kind::Sch, &a, &b);
        let details: Vec<&str> = changes.iter().map(|c| c.detail.as_str()).collect();
        assert_eq!(details, ["LCSC: - -> C123", "MPN: LM339 -> LM339LV"]);
        assert!(
            changes
                .iter()
                .all(|c| c.kind == ChangeKind::PropertyChanged)
        );
    }

    #[test]
    fn properties_leaving_with_the_symbol_stay_silent() {
        let with_mpn = entities(&[with_property(
            &instance("U1", "Lib:Box", "10 10 0", 1),
            "MPN",
            "X",
        )]);
        let swapped = entities(&[instance("U1", "Lib:BoxLV", "10 10 0", 1)]);
        let same_symbol = entities(&[instance("U1", "Lib:Box", "10 10 0", 1)]);
        // The swap took its field along: only the symbol change reports.
        let changes = diff_entities(&"p".into(), Kind::Sch, &with_mpn, &swapped);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].kind, ChangeKind::FootprintChanged);
        // Removing the field on the same symbol is its own edit.
        let changes = diff_entities(&"p".into(), Kind::Sch, &with_mpn, &same_symbol);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].detail, "MPN: X -> -");
    }

    #[test]
    fn multi_unit_part_facts_are_said_once() {
        let unit = |value: &str, lib: &str| Entity {
            reference: "U1".into(),
            value: Some(value.to_string()),
            lib_id: Some(lib.to_string()),
            at: Some([0.0, 0.0]),
            rotation: 0.0,
            sheet: Some("S".into()),
            layer: None,
            properties: BTreeMap::new(),
            pins: BTreeMap::new(),
            hidden_pins: BTreeSet::new(),
        };
        let a = BTreeMap::from([
            ("u1".into(), unit("LM339", "ti:LM339")),
            ("u2".into(), unit("LM339", "ti:LM339")),
        ]);
        // Two facts change at once: each unit repeats both, interleaved, so
        // the duplicates are not adjacent until sorted by the full key.
        let b = BTreeMap::from([
            ("u1".into(), unit("LM339LV", "ti:LM339LV")),
            ("u2".into(), unit("LM339LV", "ti:LM339LV")),
        ]);
        let changes = diff_entities(&"p".into(), Kind::Sch, &a, &b);
        let details: Vec<&str> = changes.iter().map(|c| c.detail.as_str()).collect();
        assert_eq!(
            details,
            ["Library: ti:LM339 -> ti:LM339LV", "Value: LM339 -> LM339LV"]
        );
    }

    #[test]
    fn side_flip_reports_a_row_on_each_layer() {
        let a = BTreeMap::from([("u1".into(), footprint("R1", "F.Cu", [10.0, 10.0], 90.0))]);
        let b = BTreeMap::from([("u1".into(), footprint("R1", "B.Cu", [10.0, 10.0], 90.0))]);
        let changes = diff_entities(&"p".into(), Kind::Pcb, &a, &b);
        assert_eq!(changes.len(), 2);
        let on = |layer: &str| changes.iter().find(|c| c.layer == Some(layer.into()));
        // The row on the layer the part left marks its base location only,
        // the one on the layer it landed on its head location only.
        let left = on("F.Cu").unwrap();
        assert_eq!(left.detail, "Flipped to B.Cu");
        assert_eq!(left.at_base, Some([10.0, 10.0]));
        assert_eq!(left.at_head, None);
        let landed = on("B.Cu").unwrap();
        assert_eq!(landed.detail, "Flipped from F.Cu");
        assert_eq!(landed.at_base, None);
        assert_eq!(landed.at_head, Some([10.0, 10.0]));
    }

    #[test]
    fn side_flip_carries_the_motion_in_its_detail() {
        let a = BTreeMap::from([("u1".into(), footprint("R1", "F.Cu", [10.0, 10.0], 90.0))]);
        let b = BTreeMap::from([("u1".into(), footprint("R1", "B.Cu", [13.0, 14.0], 180.0))]);
        let changes = diff_entities(&"p".into(), Kind::Pcb, &a, &b);
        let mut details: Vec<&str> = changes.iter().map(|c| c.detail.as_str()).collect();
        details.sort();
        assert_eq!(
            details,
            [
                "Flipped from F.Cu, moved 5.00 mm, rotated to 180\u{b0}",
                "Flipped to B.Cu, moved 5.00 mm, rotated to 180\u{b0}",
            ]
        );
    }

    #[test]
    fn move_without_flip_stays_a_single_row() {
        let a = BTreeMap::from([("u1".into(), footprint("R1", "F.Cu", [10.0, 10.0], 90.0))]);
        let b = BTreeMap::from([("u1".into(), footprint("R1", "F.Cu", [13.0, 14.0], 90.0))]);
        let changes = diff_entities(&"p".into(), Kind::Pcb, &a, &b);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].detail, "Moved 5.00 mm");
        assert_eq!(changes[0].layer, Some("F.Cu".into()));
    }
}

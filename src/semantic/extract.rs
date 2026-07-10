//! Parts (entities) pulled out of schematic hierarchies and board files.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    rc::Rc,
};

use anyhow::{Context, Result};

use crate::ids::{EntityId, PageName, Pin, Reference};
use kiutils_kicad::{PcbFile, SchematicFile};

use super::geometry::{Bbox, LibDrawing, edge_bbox, lib_drawings, sheet_pos};

pub(super) struct Entity {
    pub(super) reference: Reference,
    pub(super) value: Option<String>,
    pub(super) lib_id: Option<String>,
    pub(super) at: Option<[f64; 2]>,
    pub(super) rotation: f64,
    pub(super) sheet: Option<PageName>,
    pub(super) layer: Option<PageName>,
    /// The symbol's remaining properties (schematics only): the assigned
    /// footprint and any BOM field (MPN, LCSC, ...) - a change here hints
    /// that a different part gets mounted. Kept generic rather than a
    /// curated name list, so custom fields are covered too.
    pub(super) properties: BTreeMap<String, String>,
    /// Sheet positions of this instance's pins by pin number (schematics
    /// only), for pointing at net connection changes.
    pub(super) pins: BTreeMap<Pin, [f64; 2]>,
    /// Pins not drawn on the sheet; a hidden pin is stacked under a visible
    /// one and its connectivity is that pin's business.
    pub(super) hidden_pins: BTreeSet<Pin>,
}

pub(super) fn pcb_data(path: &Path) -> Result<(BTreeMap<EntityId, Entity>, Option<Bbox>)> {
    let mut out = BTreeMap::new();
    if !path.is_file() {
        return Ok((out, None));
    }
    let doc = PcbFile::read(path)
        .map_err(|e| anyhow::anyhow!("{e:?}"))
        .with_context(|| format!("failed to parse '{}'", path.display()))?;
    for fp in &doc.ast().footprints {
        let Some(uuid) = &fp.uuid else { continue };
        let Some(reference) = &fp.reference else {
            continue;
        };
        out.insert(
            uuid.clone().into(),
            Entity {
                reference: reference.clone().into(),
                value: fp.value.clone(),
                lib_id: fp.lib_id.clone(),
                at: fp.at,
                rotation: fp.rotation.unwrap_or(0.0),
                sheet: None,
                layer: fp.layer.clone().map(Into::into),
                properties: BTreeMap::new(),
                pins: BTreeMap::new(),
                hidden_pins: BTreeSet::new(),
            },
        );
    }
    let bbox = edge_bbox(doc.ast());
    Ok((out, bbox))
}

/// Symbol properties worth diffing. Reference and Value have their own
/// change kinds, Datasheet and Description are documentation that never
/// changes what gets mounted, and `ki_*` is KiCad's internal bookkeeping.
fn tracked_property(name: &str) -> bool {
    !matches!(name, "Reference" | "Value" | "Datasheet" | "Description") && !name.starts_with("ki_")
}

/// Collect symbols of the sheet hierarchy rooted at `root`, visiting every
/// sheet *instance* (a sheet file reused by several sheets appears once per
/// instance, with per-instance references from the symbols' `instances`
/// blocks). Entities are keyed by instance path plus symbol UUID; `sheet` is
/// the page name the viewer uses ("Root sheet", or the sheet's own name).
pub(super) fn sch_entities(root: &Path) -> Result<BTreeMap<EntityId, Entity>> {
    struct Parsed {
        ast: kiutils_kicad::SchematicAst,
        drawings: BTreeMap<String, LibDrawing>,
    }
    let mut cache: BTreeMap<PathBuf, Rc<Parsed>> = BTreeMap::new();
    let mut load = |path: &Path| -> Result<Rc<Parsed>> {
        if let Some(parsed) = cache.get(path) {
            return Ok(parsed.clone());
        }
        let doc = SchematicFile::read(path)
            .map_err(|e| anyhow::anyhow!("{e:?}"))
            .with_context(|| format!("failed to parse '{}'", path.display()))?;
        let ast = doc.ast().clone();
        let drawings = lib_drawings(&ast);
        let parsed = Rc::new(Parsed { ast, drawings });
        cache.insert(path.to_path_buf(), parsed.clone());
        Ok(parsed)
    };

    let mut out = BTreeMap::new();
    if !root.is_file() {
        return Ok(out);
    }
    let root_inst = format!("/{}", load(root)?.ast.uuid.clone().unwrap_or_default());
    // Ancestry guards against a sheet file including itself.
    let mut queue = vec![(
        root.to_path_buf(),
        "Root sheet".to_string(),
        root_inst,
        vec![],
    )];
    while let Some((path, page, inst, ancestry)) = queue.pop() {
        if !path.is_file() || ancestry.contains(&path) {
            continue;
        }
        let parsed = load(&path)?;
        for sym in &parsed.ast.symbols {
            let Some(uuid) = &sym.uuid else { continue };
            let this = sym
                .instances
                .iter()
                .find(|i| i.path.as_deref() == Some(&inst));
            let reference = this
                .and_then(|i| i.reference.clone())
                .or_else(|| sym.reference.clone());
            let Some(reference) = reference else { continue };
            // Power and flag pseudo-symbols only add netlist noise.
            if reference.starts_with('#') {
                continue;
            }
            let unit = this.and_then(|i| i.unit).or(sym.unit).unwrap_or(1);
            let angle = sym.angle.unwrap_or(0.0);
            let mirror = sym.mirror.as_deref();
            let drawing = sym.lib_id.as_ref().and_then(|id| parsed.drawings.get(id));
            let place = |p| sym.at.map(|anchor| sheet_pos(anchor, angle, mirror, p));
            let at = drawing
                .and_then(|d| d.center(unit))
                .and_then(&place)
                .or(sym.at);
            let unit_pins = || {
                drawing
                    .into_iter()
                    .flat_map(|d| &d.pins)
                    .filter(|p| p.unit == 0 || p.unit == unit)
            };
            let pins = unit_pins()
                .filter_map(|p| Some((p.number.clone().into(), place(p.at)?)))
                .collect();
            let hidden_pins = unit_pins()
                .filter(|p| p.hidden)
                .map(|p| p.number.clone().into())
                .collect();
            let properties = sym
                .properties
                .iter()
                .filter(|(k, v)| tracked_property(k) && !v.trim().is_empty() && v != "~")
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            out.insert(
                format!("{inst}/{uuid}").into(),
                Entity {
                    reference: reference.into(),
                    value: sym.value.clone(),
                    lib_id: sym.lib_id.clone(),
                    at,
                    rotation: angle,
                    sheet: Some(page.clone().into()),
                    layer: None,
                    properties,
                    pins,
                    hidden_pins,
                },
            );
        }
        let dir = path.parent().unwrap_or(Path::new(""));
        for sheet in &parsed.ast.sheets {
            let (Some(name), Some(file), Some(uuid)) = (&sheet.name, &sheet.filename, &sheet.uuid)
            else {
                continue;
            };
            let mut ancestry = ancestry.clone();
            ancestry.push(path.clone());
            queue.push((
                dir.join(file),
                name.clone(),
                format!("{inst}/{uuid}"),
                ancestry,
            ));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::{entities, instance};
    use super::*;

    fn at_of(entities: &BTreeMap<EntityId, Entity>, reference: &str) -> [f64; 2] {
        let e = entities
            .values()
            .find(|e| e.reference.as_ref() == reference)
            .unwrap();
        e.at.unwrap()
    }

    #[test]
    fn marker_centers_on_symbol_drawing() {
        let e = entities(&[
            instance("U1", "Lib:Box", "100 100 0", 1),
            instance("U2", "Lib:BoxLV", "100 100 0", 1),
        ]);
        // Drawing center (5, 10) in Y-up symbol space lands above and to the
        // right of the anchor on the Y-down sheet; extends borrows it.
        assert_eq!(at_of(&e, "U1"), [105.0, 90.0]);
        assert_eq!(at_of(&e, "U2"), [105.0, 90.0]);
    }

    #[test]
    fn marker_follows_instance_rotation_and_mirror() {
        let e = entities(&[
            instance("R90", "Lib:Box", "100 100 90", 1),
            instance("R180", "Lib:Box", "100 100 180", 1),
            instance("R270", "Lib:Box", "100 100 270", 1),
            instance("MX", "Lib:Box", "100 100 0", 1).replace("(unit", "(mirror x) (unit"),
            instance("MY", "Lib:Box", "100 100 0", 1).replace("(unit", "(mirror y) (unit"),
            instance("MXR90", "Lib:Box", "100 100 90", 1).replace("(unit", "(mirror x) (unit"),
        ]);
        // (5, 10) rotated CCW per the instance angle, then flipped to Y-down.
        assert_eq!(at_of(&e, "R90"), [90.0, 95.0]);
        assert_eq!(at_of(&e, "R180"), [95.0, 110.0]);
        assert_eq!(at_of(&e, "R270"), [110.0, 105.0]);
        // Mirror is applied to the already-rotated drawing.
        assert_eq!(at_of(&e, "MX"), [105.0, 110.0]);
        assert_eq!(at_of(&e, "MY"), [95.0, 90.0]);
        assert_eq!(at_of(&e, "MXR90"), [90.0, 105.0]);
    }

    #[test]
    fn marker_centers_on_the_placed_unit() {
        let e = entities(&[
            instance("Q1", "Lib:Quad", "100 100 0", 1),
            instance("Q2", "Lib:Quad", "100 100 0", 2),
            instance("Q3", "Lib:Quad", "100 100 0", 3),
        ]);
        // Each unit joins its own drawing with the shared unit-0 graphics;
        // the hidden pin of unit 3 does not count.
        assert_eq!(at_of(&e, "Q1"), [102.0, 98.0]);
        assert_eq!(at_of(&e, "Q2"), [112.0, 98.0]);
        assert_eq!(at_of(&e, "Q3"), [122.0, 98.0]);
    }

    /// A root sheet instantiating the same sub-sheet twice; the sub-sheet's
    /// one symbol carries per-instance references.
    #[test]
    fn reused_sheet_yields_one_entity_per_instance() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("kirin_inst_{nanos}"));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("root.kicad_sch"),
            r#"(kicad_sch (version 20250114) (generator "eeschema") (uuid "root")
              (sheet (at 0 0) (size 10 10) (uuid "s1")
                (property "Sheetname" "Port A" (at 0 0 0))
                (property "Sheetfile" "sub.kicad_sch" (at 0 0 0)))
              (sheet (at 20 0) (size 10 10) (uuid "s2")
                (property "Sheetname" "Port B" (at 0 0 0))
                (property "Sheetfile" "sub.kicad_sch" (at 0 0 0))))
"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("sub.kicad_sch"),
            r#"(kicad_sch (version 20250114) (generator "eeschema") (uuid "sub")
              (lib_symbols (symbol "Lib:Box" (symbol "Box_1_1" (rectangle (start 0 0) (end 10 20)))))
              (symbol (lib_id "Lib:Box") (at 100 100 0) (unit 1) (uuid "u")
                (property "Reference" "U?" (at 0 0 0))
                (property "Value" "v" (at 0 0 0))
                (instances (project "p"
                  (path "/root/s1" (reference "U1") (unit 1))
                  (path "/root/s2" (reference "U2") (unit 1))))))
"#,
        )
        .unwrap();
        let ents = sch_entities(&dir.join("root.kicad_sch")).unwrap();
        let _ = std::fs::remove_dir_all(dir);

        let find = |r: &str| ents.values().find(|e| e.reference.as_ref() == r).unwrap();
        assert_eq!(ents.len(), 2);
        assert_eq!(find("U1").sheet, Some("Port A".into()));
        assert_eq!(find("U2").sheet, Some("Port B".into()));
        // Both instances still get the drawing-centered position.
        assert_eq!(find("U2").at, Some([105.0, 90.0]));
    }
}

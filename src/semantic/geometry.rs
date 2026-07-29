//! Symbol and board geometry: drawing extents and the sheet-space transform.

use std::collections::BTreeMap;

pub(super) type Bbox = [f64; 4];

fn grow(bbox: &mut Option<Bbox>, x: f64, y: f64) {
    let b = bbox.get_or_insert([x, y, x, y]);
    b[0] = b[0].min(x);
    b[1] = b[1].min(y);
    b[2] = b[2].max(x);
    b[3] = b[3].max(y);
}

/// Bounding box of the board outline (Edge.Cuts centerline), which is what
/// `kicad-cli pcb export svg --fit-page-to-board` fits the page to (within a
/// stroke width). Arc bulges are approximated by their start/mid/end points.
pub(super) fn edge_bbox(ast: &kiutils_kicad::PcbAst) -> Option<Bbox> {
    let mut bbox = None;
    for g in &ast.graphics {
        if g.layer.as_deref() != Some("Edge.Cuts") {
            continue;
        }
        if g.token == "gr_circle"
            && let (Some(c), Some(e)) = (g.center, g.end)
        {
            let r = ((e[0] - c[0]).powi(2) + (e[1] - c[1]).powi(2)).sqrt();
            grow(&mut bbox, c[0] - r, c[1] - r);
            grow(&mut bbox, c[0] + r, c[1] + r);
            continue;
        }
        for p in [g.start, g.end, g.center, g.at].into_iter().flatten() {
            grow(&mut bbox, p[0], p[1]);
        }
    }
    bbox
}

/// Drawing extents of one library symbol in its own Y-up coordinate space,
/// split per unit: graphics on unit 0 are shared by every placed unit, a
/// numbered unit only draws its own shape (an op-amp in a quad package).
#[derive(Clone)]
pub(super) struct LibDrawing {
    pub(super) common: Option<Bbox>,
    pub(super) units: BTreeMap<i32, Bbox>,
    /// Pins by unit; unit 0 pins belong to every placed unit.
    pub(super) pins: Vec<LibPin>,
}

#[derive(Clone)]
pub(super) struct LibPin {
    pub(super) unit: i32,
    pub(super) number: String,
    pub(super) at: [f64; 2],
    /// Hidden pins are typically stacked under a visible pin carrying the
    /// same connection.
    pub(super) hidden: bool,
}

impl LibDrawing {
    /// Visual center of one placed unit: its own drawing joined with the
    /// shared graphics.
    pub(super) fn center(&self, unit: i32) -> Option<[f64; 2]> {
        let mut bbox = self.common;
        if let Some(&[x0, y0, x1, y1]) = self.units.get(&unit) {
            grow(&mut bbox, x0, y0);
            grow(&mut bbox, x1, y1);
        }
        bbox.map(|[x0, y0, x1, y1]| [(x0 + x1) / 2.0, (y0 + y1) / 2.0])
    }
}

/// Extents of a set of pins and body graphics. Hidden pins are not drawn and
/// would drag the box towards stacked power pins.
fn sym_bbox(
    bbox: &mut Option<Bbox>,
    pins: &[kiutils_kicad::SymPin],
    graphics: &[kiutils_kicad::SymGraphic],
) {
    for pin in pins {
        if pin.hide {
            continue;
        }
        if let Some([x, y]) = pin.at {
            grow(bbox, x, y);
        }
    }
    for g in graphics {
        if g.token == "circle"
            && let (Some(c), Some(r)) = (g.center, g.radius)
        {
            grow(bbox, c[0] - r, c[1] - r);
            grow(bbox, c[0] + r, c[1] + r);
            continue;
        }
        for p in [g.start, g.end, g.center]
            .into_iter()
            .flatten()
            .chain(g.pts.iter().copied())
        {
            grow(bbox, p[0], p[1]);
        }
    }
}

/// Drawings of the library symbols embedded in a schematic, keyed by lib id.
/// A symbol instance is anchored at the drawing origin, which for large parts
/// sits far from the visual center. Derived symbols (`extends`) borrow their
/// parent's drawing.
pub(super) fn lib_drawings(ast: &kiutils_kicad::SchematicAst) -> BTreeMap<String, LibDrawing> {
    let mut out = BTreeMap::new();
    for sym in &ast.lib_symbols {
        let Some(name) = &sym.name else { continue };
        // Sub-symbols are named "NAME_<unit>_<style>"; styles of the same
        // unit merge, and everything on unit 0 is shared.
        let mut boxes = BTreeMap::new();
        let mut pins = Vec::new();
        let mut take = |n: i32, unit_pins: &[kiutils_kicad::SymPin]| {
            for pin in unit_pins {
                if let (Some(number), Some(at)) = (&pin.number, pin.at) {
                    pins.push(LibPin {
                        unit: n,
                        number: number.clone(),
                        at,
                        hidden: pin.hide,
                    });
                }
            }
        };
        take(0, &sym.pins);
        sym_bbox(boxes.entry(0).or_default(), &sym.pins, &sym.graphics);
        for unit in &sym.units {
            let n: i32 = unit
                .name
                .as_deref()
                .and_then(|n| n.rsplit('_').nth(1))
                .and_then(|n| n.parse().ok())
                .unwrap_or(0);
            take(n, &unit.pins);
            sym_bbox(boxes.entry(n).or_default(), &unit.pins, &unit.graphics);
        }
        let common = boxes.remove(&0).flatten();
        let units = boxes
            .into_iter()
            .filter_map(|(n, b)| b.map(|b| (n, b)))
            .collect();
        out.insert(
            name.clone(),
            LibDrawing {
                common,
                units,
                pins,
            },
        );
    }
    for sym in &ast.lib_symbols {
        let (Some(name), Some(parent)) = (&sym.name, &sym.extends) else {
            continue;
        };
        let empty = out
            .get(name)
            .is_none_or(|d| d.common.is_none() && d.units.is_empty() && d.pins.is_empty());
        let lib = name.split_once(':').map(|(l, _)| l).unwrap_or("");
        let parent = out.get(&format!("{lib}:{parent}")).cloned();
        if empty && let Some(parent) = parent {
            out.insert(name.clone(), parent);
        }
    }
    out
}

/// Where a point of a symbol's drawing lands on the sheet: rotated then
/// mirrored like the instance (KiCAD composes the orientation in that
/// order), flipped into the sheet's Y-down coordinates and added to the
/// anchor.
pub(super) fn sheet_pos(
    anchor: [f64; 2],
    angle: f64,
    mirror: Option<&str>,
    point: [f64; 2],
) -> [f64; 2] {
    let (s, c) = angle.to_radians().sin_cos();
    let [mut x, mut y] = [point[0] * c - point[1] * s, point[0] * s + point[1] * c];
    match mirror {
        Some("x") => y = -y,
        Some("y") => x = -x,
        _ => {}
    }
    [anchor[0] + x, anchor[1] - y]
}

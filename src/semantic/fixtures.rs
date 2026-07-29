//! Shared test fixture: a library of symbols exercising the marker
//! centering, and builders parsing real schematic text into entities.

use std::collections::BTreeMap;

use crate::ids::EntityId;

use super::extract::{Entity, sch_entities};

/// A schematic whose library symbols exercise the marker centering:
/// - `Lib:Box` draws a rectangle whose center sits at (5, 10), up and to
///   the right of the anchor.
/// - `Lib:Quad` is multi-unit: shared unit-0 graphics around x 0..2, unit
///   1 around (2, 2), unit 2 around (22, 2), unit 3 with two visible pin
///   tips around (42, 2) and one hidden pin far away.
/// - `Lib:One` has a single pin "1".
/// - `Lib:BoxLV` derives from `Lib:Box` and has no drawing of its own.
const LIB: &str = r#"(lib_symbols
        (symbol "Lib:Box"
          (symbol "Box_1_1" (rectangle (start 0 0) (end 10 20))))
        (symbol "Lib:Quad"
          (symbol "Quad_0_1" (polyline (pts (xy 0 0) (xy 2 4))))
          (symbol "Quad_1_1" (rectangle (start 0 0) (end 4 4)))
          (symbol "Quad_2_1" (rectangle (start 20 0) (end 24 4)))
          (symbol "Quad_3_1"
            (pin passive line (at 40 0 0) (length 2) (name "A") (number "1"))
            (pin passive line (at 44 4 0) (length 2) (name "B") (number "2"))
            (pin power_in line (at 70 70 0) (length 2) hide (name "V") (number "3"))))
        (symbol "Lib:One"
          (symbol "One_1_1" (pin passive line (at 0 0 0) (length 2) (name "P") (number "1"))))
        (symbol "Lib:BoxLV" (extends "Box")))"#;

pub(super) fn instance(reference: &str, lib: &str, at: &str, unit: i32) -> String {
    format!(
        r#"(symbol (lib_id "{lib}") (at {at}) (unit {unit}) (uuid "{reference}")
              (property "Reference" "{reference}" (at 0 0 0))
              (property "Value" "v" (at 0 0 0)))"#
    )
}

pub(super) fn entities(instances: &[String]) -> BTreeMap<EntityId, Entity> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("kirin_sem_{nanos}.kicad_sch"));
    let body = instances.join("\n");
    std::fs::write(
        &path,
        format!("(kicad_sch (version 20250114) (generator \"eeschema\")\n{LIB}\n{body})\n"),
    )
    .unwrap();
    let out = sch_entities(&path).unwrap();
    let _ = std::fs::remove_file(path);
    out
}

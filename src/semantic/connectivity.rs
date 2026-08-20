//! Electrical connectivity compared between two revisions' netlists.

use std::collections::{BTreeMap, BTreeSet};

use crate::ids::{EntityId, NetName, Pin, ProjectLabel, Reference};
use crate::kicad::Kind;
use crate::netlist::Net;

use super::extract::Entity;
use super::{Change, ChangeKind};

/// A side's connectivity among the parts that live on both revisions: net
/// names, and which net each (reference, pin) belongs to. Nets connecting
/// fewer than two such pins constitute no connection among them and are
/// dropped, which also absorbs KiCAD's per-pin "unconnected-" nets. A pin
/// whose net was dropped but still ties it to one-revision-only parts (a
/// diode inserted in series leaves the pin alone with the new part) keeps
/// the net's name in `lone`, so the report can say more than "unconnected".
struct SideNets<'n> {
    names: Vec<&'n NetName>,
    net_of: BTreeMap<(&'n Reference, &'n Pin), usize>,
    lone: BTreeMap<(&'n Reference, &'n Pin), &'n NetName>,
}

fn index_nets<'n>(
    nets: &'n [Net],
    common: &BTreeSet<&Reference>,
    hidden: &BTreeSet<(&Reference, &Pin)>,
) -> SideNets<'n> {
    let mut names = Vec::new();
    let mut net_of = BTreeMap::new();
    let mut lone = BTreeMap::new();
    for net in nets {
        let nodes: Vec<_> = net
            .nodes
            .iter()
            .map(|(r, p)| (r, p))
            .filter(|&(r, p)| common.contains(r) && !hidden.contains(&(r, p)))
            .collect();
        if nodes.len() < 2 {
            if net.nodes.len() >= 2 {
                for node in nodes {
                    lone.insert(node, &net.name);
                }
            }
            continue;
        }
        for node in nodes {
            net_of.insert(node, names.len());
        }
        names.push(&net.name);
    }
    SideNets {
        names,
        net_of,
        lone,
    }
}

/// KiCad's file escapes for characters that collide with name syntax, as
/// undone by its UnescapeString.
const NAME_ESCAPES: &[(&str, &str)] = &[
    ("slash", "/"),
    ("backslash", "\\"),
    ("colon", ":"),
    ("space", " "),
    ("dblquote", "\""),
    ("quote", "'"),
    ("lt", "<"),
    ("gt", ">"),
    ("bar", "|"),
    ("brace", "{"),
];

/// Nets carry their sheet path ("/Power Switch/GATE"); the last segment is
/// the label a person recognizes. The path separator is a raw "/" - one
/// inside a name is escaped ("RXC{slash}B-CAST") - so split first, then
/// undo the escapes. Unknown "{...}" runs like overbar markup ("~{RST}")
/// pass through untouched.
fn short_net(name: &NetName) -> String {
    let name: &str = name.as_ref();
    let mut rest = name.rsplit('/').next().unwrap_or(name);
    let mut out = String::with_capacity(rest.len());
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        let Some(end) = rest.find('}') else { break };
        match NAME_ESCAPES.iter().find(|(t, _)| *t == &rest[1..end]) {
            Some((_, replacement)) => out.push_str(replacement),
            None => out.push_str(&rest[..=end]),
        }
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    out
}

/// A part's instances by reference (multi-unit symbols place one entity per
/// unit), and the sheet location of one of its pins, falling back to the
/// unit's center when the pin is unknown.
fn entities_by_ref(ents: &BTreeMap<EntityId, Entity>) -> BTreeMap<&Reference, Vec<&Entity>> {
    let mut map: BTreeMap<_, Vec<_>> = BTreeMap::new();
    for e in ents.values() {
        map.entry(&e.reference).or_default().push(e);
    }
    map
}

/// Whether `pin` exists on the symbols one side places for a reference;
/// `None` when the drawings expose no pins to judge by.
fn ref_has_pin(list: Option<&Vec<&Entity>>, pin: &Pin) -> Option<bool> {
    let list = list?;
    if list.iter().all(|e| e.pins.is_empty()) {
        return None;
    }
    Some(
        list.iter()
            .any(|e| e.pins.contains_key(pin) || e.hidden_pins.contains(pin)),
    )
}

/// The set of symbols one side places for a reference; the sides disagreeing
/// means the part was swapped for another.
fn lib_ids<'e>(list: Option<&Vec<&'e Entity>>) -> BTreeSet<&'e str> {
    list.into_iter()
        .flatten()
        .filter_map(|e| e.lib_id.as_deref())
        .collect()
}

fn locate_pin<'e>(
    map: &BTreeMap<&Reference, Vec<&'e Entity>>,
    reference: &Reference,
    pin: &Pin,
) -> Option<(&'e Entity, Option<[f64; 2]>)> {
    let list = map.get(reference)?;
    let e = list
        .iter()
        .find(|e| e.pins.contains_key(pin))
        .or_else(|| list.first())?;
    Some((e, e.pins.get(pin).copied().or(e.at)))
}

/// Compare electrical connectivity between two revisions. Net names take no
/// part in the comparison (labels get renamed): nets are paired by the pins
/// they connect, and only pins whose net membership changed are reported, so
/// a net gaining a pin flags that pin and not every member. Pins of parts
/// existing on a single revision are the part diff's business already.
pub(super) fn diff_connectivity(
    project: &ProjectLabel,
    nets_a: &[Net],
    nets_b: &[Net],
    ents_a: &BTreeMap<EntityId, Entity>,
    ents_b: &BTreeMap<EntityId, Entity>,
) -> Vec<Change> {
    let refs_a: BTreeSet<_> = ents_a.values().map(|e| &e.reference).collect();
    let refs_b: BTreeSet<_> = ents_b.values().map(|e| &e.reference).collect();
    let common = refs_a.intersection(&refs_b).copied().collect();
    // A pin hidden on either revision is stacked under a visible pin that
    // carries the same connection; its own membership is package noise.
    let mut hidden = BTreeSet::new();
    for e in ents_a.values().chain(ents_b.values()) {
        for p in &e.hidden_pins {
            hidden.insert((&e.reference, p));
        }
    }
    let a = index_nets(nets_a, &common, &hidden);
    let b = index_nets(nets_b, &common, &hidden);

    let mut overlap: BTreeMap<_, usize> = BTreeMap::new();
    for (node, ai) in &a.net_of {
        if let Some(bi) = b.net_of.get(node) {
            *overlap.entry((*ai, *bi)).or_default() += 1;
        }
    }
    let mut matched = BTreeMap::new();
    let mut taken = BTreeSet::new();
    // A net that kept its name and at least one pin is the same net, even
    // when most of its pins went elsewhere (a rework that empties a net
    // into new parts, or two nets trading their pins on a part). A renamed
    // net shares no name and falls through to the structural matching, so
    // a pure rename still reports nothing.
    let b_by_name: BTreeMap<_, _> = b.names.iter().enumerate().map(|(i, n)| (*n, i)).collect();
    for (ai, name) in a.names.iter().enumerate() {
        if let Some(&bi) = b_by_name.get(name)
            && overlap.contains_key(&(ai, bi))
            && taken.insert(bi)
        {
            matched.insert(ai, bi);
        }
    }
    // Pair the rest by greatest pin overlap, greedily.
    let mut pairs: Vec<_> = overlap.into_iter().collect();
    pairs.sort_by_key(|&((ai, bi), n)| (std::cmp::Reverse(n), ai, bi));
    for ((ai, bi), _) in pairs {
        if !matched.contains_key(&ai) && taken.insert(bi) {
            matched.insert(ai, bi);
        }
    }

    let by_ref_a = entities_by_ref(ents_a);
    let by_ref_b = entities_by_ref(ents_b);
    // A part whose symbol was swapped floods the report with one
    // dangling-pin row per pin; several pins of one part sharing that
    // fate say a single thing. Collect them first, so small changes keep
    // their precise per-pin rows.
    const STORM: usize = 3;
    let mut dangling: BTreeMap<(&Reference, bool), Vec<&Pin>> = BTreeMap::new();
    let mut rows = Vec::new();
    let nodes: BTreeSet<_> = a
        .net_of
        .keys()
        .chain(b.net_of.keys())
        .chain(a.lone.keys())
        .chain(b.lone.keys())
        .copied()
        .collect();
    for (reference, pin) in nodes {
        let sa = a.net_of.get(&(reference, pin));
        let sb = b.net_of.get(&(reference, pin));
        let same = match (sa, sb) {
            (Some(ai), Some(bi)) => matched.get(ai) == Some(bi),
            // Unconnected among common parts on both sides is the same
            // silence, unless the pin's one-revision-only company differs:
            // a pin gaining or losing a net of added/removed parts is a
            // real (dis)connection even though no common part shares it.
            (None, None) => {
                let la = a.lone.get(&(reference, pin));
                let lb = b.lone.get(&(reference, pin));
                match (la, lb) {
                    // Compare display names: sheet renames move the path
                    // prefix of every net on the sheet, which is no rewire.
                    (Some(x), Some(y)) => short_net(x) == short_net(y),
                    (None, None) => true,
                    _ => false,
                }
            }
            _ => false,
        };
        if same {
            continue;
        }
        // A pin left alone with parts of one revision is still on a net
        // there; "unconnected" is reserved for a genuinely dangling pin.
        let from = match sa {
            Some(i) => short_net(a.names[*i]),
            None => match a.lone.get(&(reference, pin)) {
                Some(n) => format!("{} (removed parts)", short_net(n)),
                None => "unconnected".to_string(),
            },
        };
        let to = match sb {
            Some(i) => short_net(b.names[*i]),
            None => match b.lone.get(&(reference, pin)) {
                Some(n) => format!("{} (added parts)", short_net(n)),
                None => "unconnected".to_string(),
            },
        };
        // `true` marks a pin dangling on head (disconnected), `false` one
        // dangling on base (newly connected); a pin is never both.
        let fate = if sb.is_none() && !b.lone.contains_key(&(reference, pin)) {
            Some(true)
        } else if sa.is_none() && !a.lone.contains_key(&(reference, pin)) {
            Some(false)
        } else {
            None
        };
        // A pin dangling on a side whose symbol does not even have it went
        // away (or arrived) with a symbol swap; the change reported on the
        // part itself is the parent of that, and a row here would only echo
        // it.
        match fate {
            Some(true) if ref_has_pin(by_ref_b.get(reference), pin) == Some(false) => continue,
            Some(false) if ref_has_pin(by_ref_a.get(reference), pin) == Some(false) => continue,
            _ => {}
        }
        if let Some(fate) = fate {
            dangling.entry((reference, fate)).or_default().push(pin);
        }
        rows.push((reference, pin, from, to, fate));
    }

    // Same-fate pins collapse into one summary row only when the symbol
    // was swapped for another part: the part's own Library row is the
    // parent of every such pin. On an unchanged symbol every pin keeps
    // its row - the per-pin diff is what firmware pin re-assignment is
    // done from.
    let collapsed: BTreeSet<(&Reference, bool)> = dangling
        .iter()
        .filter(|((reference, _), pins)| {
            pins.len() >= STORM && {
                let la = lib_ids(by_ref_a.get(*reference));
                let lb = lib_ids(by_ref_b.get(*reference));
                !la.is_empty() && !lb.is_empty() && la != lb
            }
        })
        .map(|(key, _)| *key)
        .collect();

    let mut changes = Vec::new();
    for (reference, pin, from, to, fate) in rows {
        if fate.is_some_and(|f| collapsed.contains(&(reference, f))) {
            continue;
        }
        let base = locate_pin(&by_ref_a, reference, pin);
        let head = locate_pin(&by_ref_b, reference, pin);
        changes.push(Change {
            project: project.clone(),
            scope: Kind::Sch,
            sheet: head.or(base).and_then(|(e, _)| e.sheet.clone()),
            kind: ChangeKind::NetChanged,
            reference: reference.clone(),
            detail: format!("Pin {pin}: {from} -> {to}"),
            layer: None,
            at_base: base.and_then(|(_, at)| at),
            at_head: head.and_then(|(_, at)| at),
            frac_base: None,
            frac_head: None,
        });
    }
    for ((reference, disconnected), pins) in dangling {
        if !collapsed.contains(&(reference, disconnected)) {
            continue;
        }
        // The marker points at one of the affected pins; the reference's
        // first entity may be an unused unit of a multi-unit symbol.
        let base = locate_pin(&by_ref_a, reference, pins[0]);
        let head = locate_pin(&by_ref_b, reference, pins[0]);
        changes.push(Change {
            project: project.clone(),
            scope: Kind::Sch,
            sheet: head.or(base).and_then(|(e, _)| e.sheet.clone()),
            kind: ChangeKind::NetChanged,
            reference: reference.clone(),
            detail: format!(
                "{} pins {}",
                pins.len(),
                if disconnected {
                    "disconnected"
                } else {
                    "connected"
                }
            ),
            layer: None,
            at_base: base.and_then(|(_, at)| at),
            at_head: head.and_then(|(_, at)| at),
            frac_base: None,
            frac_head: None,
        });
    }
    changes
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::{entities, instance};
    use super::*;

    fn net(name: &str, nodes: &[(&str, &str)]) -> Net {
        Net {
            name: name.into(),
            nodes: nodes.iter().map(|&(r, p)| (r.into(), p.into())).collect(),
        }
    }

    #[test]
    fn flags_only_the_pin_that_moved() {
        let e = entities(&[
            instance("U1", "Lib:Box", "100 100 0", 1),
            instance("U2", "Lib:Box", "50 50 0", 1),
            instance("U3", "Lib:Box", "20 20 0", 1),
        ]);
        let base = [
            net("/sheet/A", &[("U1", "1"), ("U2", "1"), ("U3", "1")]),
            net("B", &[("U1", "2"), ("U2", "2")]),
        ];
        // U3.1 moves from A to B; A also gets renamed, which is not a change.
        let head = [
            net("/sheet/A_RENAMED", &[("U1", "1"), ("U2", "1")]),
            net("B", &[("U1", "2"), ("U2", "2"), ("U3", "1")]),
        ];
        let changes = diff_connectivity(&"p".into(), &base, &head, &e, &e);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].reference.as_ref(), "U3");
        assert_eq!(changes[0].detail, "Pin 1: A -> B");
    }

    #[test]
    fn ignores_pins_of_single_revision_parts() {
        let base_ents = entities(&[
            instance("U1", "Lib:Box", "100 100 0", 1),
            instance("U2", "Lib:Box", "50 50 0", 1),
        ]);
        let head_ents = entities(&[
            instance("U1", "Lib:Box", "100 100 0", 1),
            instance("U2", "Lib:Box", "50 50 0", 1),
            instance("U9", "Lib:Box", "10 10 0", 1),
        ]);
        let base = [net("A", &[("U1", "1"), ("U2", "1")])];
        let head = [net("A", &[("U1", "1"), ("U2", "1"), ("U9", "1")])];
        let changes = diff_connectivity(&"p".into(), &base, &head, &base_ents, &head_ents);
        assert!(changes.is_empty());
    }

    #[test]
    fn reports_disconnections_on_both_ends() {
        let e = entities(&[
            instance("U1", "Lib:Box", "100 100 0", 1),
            instance("U2", "Lib:Box", "50 50 0", 1),
        ]);
        let base = [net("A", &[("U1", "1"), ("U2", "1")])];
        let changes = diff_connectivity(&"p".into(), &base, &[], &e, &e);
        assert_eq!(changes.len(), 2);
        assert!(
            changes
                .iter()
                .all(|c| c.detail == "Pin 1: A -> unconnected")
        );
    }

    #[test]
    fn pins_vanishing_with_a_symbol_swap_stay_silent() {
        // J1's new symbol has no pin 2: that pin left with the swap, which
        // is already reported on J1 itself. U1's pin genuinely dangles.
        let base_e = entities(&[
            instance("J1", "Lib:Quad", "100 100 0", 3),
            instance("U1", "Lib:Quad", "50 50 0", 3),
        ]);
        let head_e = entities(&[
            instance("J1", "Lib:One", "100 100 0", 1),
            instance("U1", "Lib:Quad", "50 50 0", 3),
        ]);
        let base = [net("A", &[("J1", "2"), ("U1", "1")])];
        let changes = diff_connectivity(&"p".into(), &base, &[], &base_e, &head_e);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].reference.as_ref(), "U1");
        assert_eq!(changes[0].detail, "Pin 1: A -> unconnected");
    }

    #[test]
    fn net_names_are_unescaped_for_display() {
        assert_eq!(
            short_net(&"/Ethernet bridge/RXC{slash}B-CAST".into()),
            "RXC/B-CAST"
        );
        assert_eq!(short_net(&"A{colon}B{space}C".into()), "A:B C");
        // Overbar markup is display syntax, not an escape.
        assert_eq!(
            short_net(&"POWER.~{OUT_DISABLE}".into()),
            "POWER.~{OUT_DISABLE}"
        );
        assert_eq!(short_net(&"{unbalanced".into()), "{unbalanced");
    }

    #[test]
    fn gutted_wiring_on_same_symbol_keeps_per_pin_rows() {
        // Every connection of J1 dies but its symbol stays: firmware pin
        // re-assignment reads the per-pin rows, so nothing folds into an
        // "N pins" summary.
        let e = entities(&[
            instance("J1", "Lib:Box", "100 100 0", 1),
            instance("U1", "Lib:Box", "50 50 0", 1),
            instance("U2", "Lib:Box", "60 60 0", 1),
            instance("U3", "Lib:Box", "70 70 0", 1),
        ]);
        let base = [
            net("A", &[("J1", "1"), ("U1", "1")]),
            net("B", &[("J1", "2"), ("U2", "1")]),
            net("C", &[("J1", "3"), ("U3", "1")]),
        ];
        let changes = diff_connectivity(&"p".into(), &base, &[], &e, &e);
        let details: Vec<String> = changes
            .iter()
            .map(|c| format!("{} {}", c.reference, c.detail))
            .collect();
        assert!(details.contains(&"J1 Pin 1: A -> unconnected".to_string()));
        assert!(details.contains(&"J1 Pin 2: B -> unconnected".to_string()));
        assert!(details.contains(&"J1 Pin 3: C -> unconnected".to_string()));
        assert!(details.contains(&"U1 Pin 1: A -> unconnected".to_string()));
        assert_eq!(changes.len(), 6);

        // The reverse direction stays per-pin too.
        let reversed = diff_connectivity(&"p".into(), &[], &base, &e, &e);
        assert!(
            reversed
                .iter()
                .any(|c| c.reference.as_ref() == "J1" && c.detail == "Pin 1: unconnected -> A")
        );
    }

    #[test]
    fn pin_remap_on_a_large_part_keeps_per_pin_rows() {
        // Three of M's eight connections dangle on head - far from gutted
        // wiring, so each pin keeps its own informative row instead of
        // folding into a "3 pins disconnected" summary.
        let mut insts = vec![instance("M", "Lib:Box", "100 100 0", 1)];
        for i in 1..=8 {
            insts.push(instance(&format!("U{i}"), "Lib:Box", "10 10 0", 1));
        }
        let e = entities(&insts);
        let nets = |n: usize| -> Vec<Net> {
            (1..=n)
                .map(|i| Net {
                    name: format!("N{i}").into(),
                    nodes: vec![
                        ("M".into(), format!("{i}").into()),
                        (format!("U{i}").into(), "1".into()),
                    ],
                })
                .collect()
        };
        let changes = diff_connectivity(&"p".into(), &nets(8), &nets(5), &e, &e);
        let mut m_rows: Vec<_> = changes
            .iter()
            .filter(|c| c.reference.as_ref() == "M")
            .map(|c| c.detail.as_str())
            .collect();
        m_rows.sort();
        assert_eq!(
            m_rows,
            [
                "Pin 6: N6 -> unconnected",
                "Pin 7: N7 -> unconnected",
                "Pin 8: N8 -> unconnected",
            ]
        );
    }

    #[test]
    fn symbol_swap_collapses_dangling_pins() {
        // The same three dangling pins, but M's symbol changed: the
        // Library row on M is the parent change and the pins fold into
        // one summary row.
        let mut base_insts = vec![instance("M", "Lib:Box", "100 100 0", 1)];
        let mut head_insts = vec![instance("M", "Lib:BoxLV", "100 100 0", 1)];
        for i in 1..=8 {
            let u = instance(&format!("U{i}"), "Lib:Box", "10 10 0", 1);
            base_insts.push(u.clone());
            head_insts.push(u);
        }
        let e_base = entities(&base_insts);
        let e_head = entities(&head_insts);
        let nets = |n: usize| -> Vec<Net> {
            (1..=n)
                .map(|i| Net {
                    name: format!("N{i}").into(),
                    nodes: vec![
                        ("M".into(), format!("{i}").into()),
                        (format!("U{i}").into(), "1".into()),
                    ],
                })
                .collect()
        };
        let changes = diff_connectivity(&"p".into(), &nets(8), &nets(5), &e_base, &e_head);
        let m_rows: Vec<_> = changes
            .iter()
            .filter(|c| c.reference.as_ref() == "M")
            .map(|c| c.detail.as_str())
            .collect();
        assert_eq!(m_rows, ["3 pins disconnected"]);
    }

    #[test]
    fn storm_marker_points_at_an_affected_pin() {
        // T1's wiring dies with a symbol swap; the summary row's marker
        // sits on one of the dangling pins (pin "1" at sheet (100, 100)),
        // not on the symbol drawing's center.
        let e_base = entities(&[
            instance("T1", "Lib:Tri", "100 100 0", 1),
            instance("U1", "Lib:Box", "50 50 0", 1),
            instance("U2", "Lib:Box", "60 60 0", 1),
            instance("U3", "Lib:Box", "70 70 0", 1),
        ]);
        let e_head = entities(&[
            instance("T1", "Lib:Box", "100 100 0", 1),
            instance("U1", "Lib:Box", "50 50 0", 1),
            instance("U2", "Lib:Box", "60 60 0", 1),
            instance("U3", "Lib:Box", "70 70 0", 1),
        ]);
        let base = [
            net("A", &[("T1", "1"), ("U1", "1")]),
            net("B", &[("T1", "2"), ("U2", "1")]),
            net("C", &[("T1", "3"), ("U3", "1")]),
        ];
        let changes = diff_connectivity(&"p".into(), &base, &[], &e_base, &e_head);
        let t = changes
            .iter()
            .find(|c| c.reference.as_ref() == "T1")
            .unwrap();
        assert_eq!(t.detail, "3 pins disconnected");
        assert_eq!(t.at_base, Some([100.0, 100.0]));
    }

    #[test]
    fn connection_to_added_parts_only_gets_a_row() {
        // M's pin 5 was truly unconnected and now reaches only newly
        // added parts: a real connection, reported with the net's name
        // even though no common part shares the net.
        let e_base = entities(&[instance("M", "Lib:Box", "100 100 0", 1)]);
        let e_head = entities(&[
            instance("M", "Lib:Box", "100 100 0", 1),
            instance("D", "Lib:Box", "50 50 0", 1),
        ]);
        let base = [net("unconnected-(M-Pad5)", &[("M", "5")])];
        let head = [net("LED", &[("M", "5"), ("D", "1")])];
        let changes = diff_connectivity(&"p".into(), &base, &head, &e_base, &e_head);
        let details: Vec<_> = changes.iter().map(|c| c.detail.as_str()).collect();
        assert_eq!(details, ["Pin 5: unconnected -> LED (added parts)"]);
    }

    #[test]
    fn neighbor_swap_keeps_the_pin_silent() {
        // A pull-up replaced under a new reference (on a renamed sheet)
        // leaves M's pin lone on both sides with the same net label: no
        // rewiring happened, so nothing is reported.
        let e_base = entities(&[
            instance("M", "Lib:Box", "100 100 0", 1),
            instance("R1", "Lib:Box", "50 50 0", 1),
        ]);
        let e_head = entities(&[
            instance("M", "Lib:Box", "100 100 0", 1),
            instance("R2", "Lib:Box", "50 50 0", 1),
        ]);
        let base = [net("/sheet/PULL", &[("M", "5"), ("R1", "1")])];
        let head = [net("/renamed/PULL", &[("M", "5"), ("R2", "1")])];
        let changes = diff_connectivity(&"p".into(), &base, &head, &e_base, &e_head);
        assert!(changes.is_empty());
    }

    #[test]
    fn pin_swap_flags_the_swapped_pins() {
        // Two nets trade their U1/U2 pins (the MDC/MDIO-crossed case). The
        // structural match ties, so the kept names decide which net is
        // which, and only the swapped pins are flagged - not the pins that
        // stayed put on M and R.
        let e = entities(&[
            instance("U1", "Lib:Box", "100 100 0", 1),
            instance("U2", "Lib:Box", "80 80 0", 1),
            instance("M", "Lib:Box", "50 50 0", 1),
            instance("R", "Lib:Box", "20 20 0", 1),
        ]);
        let base = [
            net("MDC", &[("U1", "4"), ("U2", "4"), ("M", "1")]),
            net("MDIO", &[("U1", "5"), ("U2", "5"), ("M", "2"), ("R", "1")]),
        ];
        let head = [
            net("MDC", &[("U1", "5"), ("U2", "5"), ("M", "1")]),
            net("MDIO", &[("U1", "4"), ("U2", "4"), ("M", "2"), ("R", "1")]),
        ];
        let changes = diff_connectivity(&"p".into(), &base, &head, &e, &e);
        let mut rows: Vec<_> = changes
            .iter()
            .map(|c| format!("{} {}", c.reference, c.detail))
            .collect();
        rows.sort();
        assert_eq!(
            rows,
            [
                "U1 Pin 4: MDC -> MDIO",
                "U1 Pin 5: MDIO -> MDC",
                "U2 Pin 4: MDC -> MDIO",
                "U2 Pin 5: MDIO -> MDC",
            ]
        );
    }

    #[test]
    fn series_insertion_names_the_net_through_the_new_part() {
        // A new part D is inserted between M's pin and the rest of the net:
        // the pin keeps its net name but now reaches only the added part,
        // and the report says so instead of calling the pin unconnected.
        let e_base = entities(&[
            instance("M", "Lib:Box", "100 100 0", 1),
            instance("U1", "Lib:Box", "50 50 0", 1),
            instance("U2", "Lib:Box", "20 20 0", 1),
        ]);
        let e_head = entities(&[
            instance("M", "Lib:Box", "100 100 0", 1),
            instance("U1", "Lib:Box", "50 50 0", 1),
            instance("U2", "Lib:Box", "20 20 0", 1),
            instance("D", "Lib:Box", "70 70 0", 1),
        ]);
        let base = [net("EN", &[("M", "5"), ("U1", "1"), ("U2", "1")])];
        let head = [
            net("EN", &[("M", "5"), ("D", "1")]),
            net("EN_SW", &[("D", "2"), ("U1", "1"), ("U2", "1")]),
        ];
        let changes = diff_connectivity(&"p".into(), &base, &head, &e_base, &e_head);
        let m = changes
            .iter()
            .find(|c| c.reference.as_ref() == "M")
            .unwrap();
        assert_eq!(m.detail, "Pin 5: EN -> EN (added parts)");
    }

    #[test]
    fn rework_keeps_the_named_net_and_flags_the_leavers() {
        // Most of ENABLE's pins move to a fresh net, but it keeps its name
        // and its MCU pin. The net that kept its name is the same net: the
        // pins that left are flagged, not the pin that stayed.
        let e = entities(&[
            instance("M", "Lib:Box", "100 100 0", 1),
            instance("R", "Lib:Box", "80 80 0", 1),
            instance("D", "Lib:Box", "50 50 0", 1),
            instance("T", "Lib:Box", "20 20 0", 1),
            instance("G", "Lib:Box", "10 10 0", 1),
        ]);
        let base = [
            net("ENABLE", &[("M", "5"), ("R", "1"), ("D", "2"), ("T", "1")]),
            net("GND", &[("R", "2"), ("G", "1")]),
        ];
        let head = [
            net("ENABLE", &[("M", "5"), ("R", "2")]),
            net("GND", &[("R", "1"), ("G", "1")]),
            net("Net-(D-K)", &[("D", "2"), ("T", "1")]),
        ];
        let changes = diff_connectivity(&"p".into(), &base, &head, &e, &e);
        let mut rows: Vec<_> = changes
            .iter()
            .map(|c| format!("{} {}", c.reference, c.detail))
            .collect();
        rows.sort();
        assert_eq!(
            rows,
            [
                "D Pin 2: ENABLE -> Net-(D-K)",
                "R Pin 1: ENABLE -> GND",
                "R Pin 2: GND -> ENABLE",
                "T Pin 1: ENABLE -> Net-(D-K)",
            ]
        );
    }

    #[test]
    fn ignores_hidden_pins() {
        // Quad unit 3's pin "3" is hidden (a stacked pin); its membership
        // moving between nets is not a reportable connection change.
        let e = entities(&[
            instance("Q3", "Lib:Quad", "100 100 0", 3),
            instance("U1", "Lib:Box", "50 50 0", 1),
            instance("U2", "Lib:Box", "20 20 0", 1),
        ]);
        let base = [
            net("A", &[("Q3", "3"), ("U1", "1")]),
            net("B", &[("U2", "1"), ("U1", "2")]),
        ];
        let head = [
            net("A", &[("U1", "1"), ("U2", "1")]),
            net("B", &[("Q3", "3"), ("U1", "2")]),
        ];
        let changes = diff_connectivity(&"p".into(), &base, &head, &e, &e);
        assert!(changes.iter().all(|c| c.reference.as_ref() != "Q3"));
    }

    #[test]
    fn marker_points_at_the_pin() {
        // Quad unit 3 has visible pins "1" at (40, 0) and "2" at (44, 4).
        let e = entities(&[
            instance("Q3", "Lib:Quad", "100 100 0", 3),
            instance("U1", "Lib:Box", "50 50 0", 1),
            instance("U2", "Lib:Box", "20 20 0", 1),
        ]);
        let base = [
            net("A", &[("Q3", "2"), ("U1", "1")]),
            net("B", &[("U2", "1"), ("U1", "2")]),
        ];
        let head = [
            net("A", &[("U1", "1"), ("U2", "1")]),
            net("B", &[("Q3", "2"), ("U1", "2")]),
        ];
        let changes = diff_connectivity(&"p".into(), &base, &head, &e, &e);
        let q = changes
            .iter()
            .find(|c| c.reference.as_ref() == "Q3")
            .unwrap();
        assert_eq!(q.at_head, Some([144.0, 96.0]));
    }
}

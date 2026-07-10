use std::path::Path;

use anyhow::{Context, Result};
use kiutils_sexpr::{Atom, Node};

use crate::ids::{NetName, Pin, Reference};
use crate::kicad::run_kicad_cli;

/// One electrical net from an exported netlist.
pub struct Net {
    pub name: NetName,
    /// Connected pins as (reference, pin number).
    pub nodes: Vec<(Reference, Pin)>,
}

/// Run `kicad-cli sch export netlist` on a root schematic and parse the
/// resulting s-expression netlist.
pub fn export_netlist(root: &Path, out: &Path) -> Result<Vec<Net>> {
    run_kicad_cli(root, |c| {
        c.args(["sch", "export", "netlist", "--format", "kicadsexpr", "-o"])
            .arg(out)
            .arg(root);
    })?;
    parse_netlist(out)
}

fn parse_netlist(path: &Path) -> Result<Vec<Net>> {
    let src = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read '{}'", path.display()))?;
    let doc = kiutils_sexpr::parse_one(&src)
        .map_err(|e| anyhow::anyhow!("{e:?}"))
        .with_context(|| format!("failed to parse '{}'", path.display()))?;
    let mut nets = Vec::new();
    let Some(Node::List { items, .. }) = doc.nodes.first() else {
        return Ok(nets);
    };
    for section in items {
        if head_of(section) != Some("nets") {
            continue;
        }
        let Node::List { items, .. } = section else {
            continue;
        };
        for net in items.iter().skip(1) {
            if head_of(net) == Some("net") {
                nets.push(parse_net(net));
            }
        }
    }
    Ok(nets)
}

fn parse_net(node: &Node) -> Net {
    let mut name = String::new();
    let mut nodes = Vec::new();
    let Node::List { items, .. } = node else {
        return Net {
            name: name.into(),
            nodes,
        };
    };
    for child in items.iter().skip(1) {
        match head_of(child) {
            Some("name") => name = second_string(child).unwrap_or_default(),
            Some("node") => {
                let Node::List { items, .. } = child else {
                    continue;
                };
                let mut reference = None;
                let mut pin = None;
                for field in items.iter().skip(1) {
                    match head_of(field) {
                        Some("ref") => reference = second_string(field),
                        Some("pin") => pin = second_string(field),
                        _ => {}
                    }
                }
                if let (Some(reference), Some(pin)) = (reference, pin) {
                    nodes.push((reference.into(), pin.into()));
                }
            }
            _ => {}
        }
    }
    Net {
        name: name.into(),
        nodes,
    }
}

fn head_of(node: &Node) -> Option<&str> {
    let Node::List { items, .. } = node else {
        return None;
    };
    match items.first() {
        Some(Node::Atom {
            atom: Atom::Symbol(s),
            ..
        }) => Some(s),
        _ => None,
    }
}

fn second_string(node: &Node) -> Option<String> {
    let Node::List { items, .. } = node else {
        return None;
    };
    match items.get(1) {
        Some(Node::Atom { atom, .. }) => match atom {
            Atom::Symbol(s) | Atom::Quoted(s) => Some(s.clone()),
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nets_and_nodes() {
        let src = r#"(export (version "E")
          (components (comp (ref "R1")))
          (nets
            (net (code "1") (name "/sheet/GATE")
              (node (ref "R1") (pin "1") (pintype "passive"))
              (node (ref "U1") (pin "14")))
            (net (code "2") (name "unconnected-(U1-PA7-Pad13)")
              (node (ref "U1") (pin "13")))))
"#;
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("kirin_netlist_{nanos}.net"));
        std::fs::write(&path, src).unwrap();
        let nets = parse_netlist(&path).unwrap();
        let _ = std::fs::remove_file(path);

        assert_eq!(nets.len(), 2);
        assert_eq!(nets[0].name.as_ref(), "/sheet/GATE");
        assert_eq!(
            nets[0].nodes,
            vec![("R1".into(), "1".into()), ("U1".into(), "14".into())]
        );
        assert_eq!(nets[1].nodes.len(), 1);
    }
}

use std::{
    collections::HashMap,
    fs::read_to_string,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result};

/// Searches for KiCAD schematics in `repo` at commit `commit_ref`;
/// and extracts them to `output_dir`. Returns a map with paths to
/// each schematic and the commit hash it was last updated at.
pub fn extract_schematics(
    repo: &gix::Repository,
    commit_ref: &str,
    output_dir: &Path,
) -> Result<HashMap<PathBuf, gix::ObjectId>> {
    let commit = repo
        .rev_parse_single(commit_ref)
        .with_context(|| format!("failed to resolve ref '{commit_ref}'"))?
        .object()?
        .peel_to_commit()?;

    let tree = commit.tree()?;
    let mut recorder = gix::traverse::tree::Recorder::default();
    tree.traverse().breadthfirst(&mut recorder)?;

    let mut found = HashMap::new();
    for entry in recorder.records {
        if entry.filepath.ends_with(b".kicad_sch") {
            let obj = repo.find_object(entry.oid)?;
            let rel_path = PathBuf::from(entry.filepath.to_string());
            let dest = output_dir.join(&rel_path);
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&dest, &obj.data)?;
            found.insert(rel_path, entry.oid);
        }
    }

    Ok(found)
}

/// Searches for KiCAD layouts in `repo` at commit `commit_ref`;
/// and extracts them to `output_dir`. Returns a map with paths to
/// each layout and the commit hash it was last updated at.
pub fn extract_pcb(
    repo: &gix::Repository,
    commit_ref: &str,
    output_dir: &Path,
) -> Result<HashMap<PathBuf, gix::ObjectId>> {
    let commit = repo
        .rev_parse_single(commit_ref)
        .with_context(|| format!("failed to resolve ref '{commit_ref}'"))?
        .object()?
        .peel_to_commit()?;

    let tree = commit.tree()?;
    let mut recorder = gix::traverse::tree::Recorder::default();
    tree.traverse().breadthfirst(&mut recorder)?;

    let mut found = HashMap::new();
    for entry in recorder.records {
        if entry.filepath.ends_with(b".kicad_pcb") {
            let obj = repo.find_object(entry.oid)?;
            let rel_path = PathBuf::from(entry.filepath.to_string());
            let dest = output_dir.join(&rel_path);
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&dest, &obj.data)?;
            found.insert(rel_path, entry.oid);
        }
    }

    Ok(found)
}

// Returns (paths_to_render_in_base, paths_to_render_in_head):
// any file present on one side only, or present on both with different OIDs.
pub fn changed_paths(
    base: &HashMap<PathBuf, gix::ObjectId>,
    head: &HashMap<PathBuf, gix::ObjectId>,
) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut from_base = Vec::new();
    let mut from_head = Vec::new();
    for (path, base_oid) in base {
        match head.get(path) {
            None => from_base.push(path.clone()),
            Some(head_oid) if head_oid != base_oid => {
                from_base.push(path.clone());
                from_head.push(path.clone());
            }
            _ => {}
        }
    }
    for path in head.keys() {
        if !base.contains_key(path) {
            from_head.push(path.clone());
        }
    }
    from_base.sort();
    from_head.sort();
    (from_base, from_head)
}

pub fn render_schematics(
    schematic_dir: &Path,
    svg_dir: &Path,
    schematics: &[PathBuf],
) -> Result<()> {
    for sch in schematics {
        let input = schematic_dir.join(sch);
        let output_parent = svg_dir.join(sch.parent().unwrap_or(Path::new("")));
        std::fs::create_dir_all(&output_parent)?;

        let status = Command::new("kicad-cli")
            .args(["sch", "export", "svg"])
            .arg("-o")
            .arg(&output_parent)
            .arg(&input)
            .status()
            .context("failed to invoke 'kicad-cli' — is KiCAD installed and on PATH?")?;

        if !status.success() {
            anyhow::bail!("kicad-cli failed for '{}'", input.display());
        }
    }

    Ok(())
}

pub fn render_pcbs(pcb_dir: &Path, svg_dir: &Path, pcbs: &[PathBuf]) -> Result<()> {
    for pcb in pcbs {
        let input = pcb_dir.join(pcb);
        let output_parent = svg_dir.join(pcb.parent().unwrap_or(Path::new("")));
        std::fs::create_dir_all(&output_parent)?;

        let layers = get_layers(&input);
        let layer_arg = layers.join(",");

        let status = Command::new("kicad-cli")
            .args(["pcb", "export", "svg"])
            .arg("-o")
            .arg(&output_parent)
            .arg(&input)
            .arg("-l")
            .arg(&layer_arg)
            .arg("--mode-multi")
            .arg("--fit-page-to-board")
            .status()
            .context("failed to invoke 'kicad-cli' — is KiCAD installed and on PATH?")?;

        if !status.success() {
            anyhow::bail!("kicad-cli failed for '{}'", input.display());
        }
    }

    Ok(())
}

/// Parses the layers of a `kicad_pcb` file at `pcb_path`.
fn get_layers(pcb_path: &Path) -> Vec<String> {
    let content = read_to_string(pcb_path).expect("Failed to read file");
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
    layers
}

fn extract_quoted(s: &str) -> Option<String> {
    let start = s.find('"')? + 1;
    let end = s[start..].find('"')? + start;
    Some(s[start..end].to_string())
}

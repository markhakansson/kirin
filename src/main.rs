use std::path::PathBuf;
use std::process::Command;
use std::process::Stdio;

use anyhow::{Context, Result};
use clap::Parser;

mod kicad;
mod svg;
mod template;

#[derive(Parser)]
#[command(about = "Compare KiCAD projects between commits")]
struct Args {
    #[arg(short, long, default_value = ".")]
    /// Path to the git repository
    repo: PathBuf,
    #[arg(short, long, default_value = "HEAD~1")]
    /// Base commit reference (e.g. main, HEAD~1, a commit SHA)
    base: String,
    // No `short`: it would derive `-h`, which collides with clap's `--help`.
    #[arg(long, default_value = "HEAD")]
    /// Head commit reference
    head: String,
    /// Restrict to projects under this repo-relative path (e.g. anchor)
    #[arg(short, long)]
    project_dir: Option<PathBuf>,
    /// Output directory
    #[arg(short, long, default_value = "kirin-out")]
    out: PathBuf,
    /// Skip SVG minification
    #[arg(long)]
    no_svg_compression: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();

    // Quick-check to verify external dependencies exist
    let _ = Command::new("kicad-cli")
        .arg("--help")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("'kicad-cli' not in PATH?")?;

    let repo = gix::open(&args.repo)
        .with_context(|| format!("failed to open repo at '{}'", args.repo.display()))?;

    let base_blobs = kicad::tree_blobs(&repo, &args.base)?;
    let head_blobs = kicad::tree_blobs(&repo, &args.head)?;

    let projects = kicad::discover_projects(&base_blobs, &head_blobs, args.project_dir.as_deref());
    if projects.is_empty() {
        eprintln!("warning: no KiCAD projects (*.kicad_pro) found in the selected range");
    }

    let mut pages = Vec::new();
    for project in &projects {
        pages.extend(kicad::process_project(
            &repo,
            &base_blobs,
            &head_blobs,
            project,
            &args.out,
        )?);
    }

    if !args.no_svg_compression {
        svg::optimize_pages(&args.out, &pages);
    }

    template::generate_site(&args.out, &args.base, &args.head, &pages)?;

    // Sources are only needed for the render step; keep the artifact small.
    let _ = std::fs::remove_dir_all(args.out.join(".work"));

    eprintln!(
        "Done ({} changed page(s)). Open '{}/index.html'",
        pages.len(),
        args.out.display()
    );
    Ok(())
}

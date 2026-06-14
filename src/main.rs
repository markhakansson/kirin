use std::path::PathBuf;
use std::process::Command;
use std::process::Stdio;

use anyhow::{Context, Result};
use clap::Parser;
use log;

mod kicad;
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
    /// Output directory
    #[arg(short, long, default_value = "kirin-out")]
    out: PathBuf,
}

fn main() -> Result<()> {
    env_logger::init();

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

    let sch_dir_a = args.out.join("a").join("sch");
    let sch_dir_b = args.out.join("b").join("sch");

    let pcb_dir_a = args.out.join("a").join("pcb");
    let pcb_dir_b = args.out.join("b").join("pcb");

    let svg_sch_dir_a = args.out.join("a").join("svg").join("sch");
    let svg_sch_dir_b = args.out.join("b").join("svg").join("sch");
    let svg_pcb_dir_a = args.out.join("a").join("svg").join("pcb");
    let svg_pcb_dir_b = args.out.join("b").join("svg").join("pcb");

    std::fs::create_dir_all(&sch_dir_a)?;
    std::fs::create_dir_all(&sch_dir_b)?;

    std::fs::create_dir_all(&svg_sch_dir_a)?;
    std::fs::create_dir_all(&svg_sch_dir_b)?;
    std::fs::create_dir_all(&svg_pcb_dir_a)?;
    std::fs::create_dir_all(&svg_pcb_dir_b)?;

    let schematics_a = kicad::extract_schematics(&repo, &args.base, &sch_dir_a)?;
    let pcb_a = kicad::extract_pcb(&repo, &args.base, &pcb_dir_a)?;

    let schematics_b = kicad::extract_schematics(&repo, &args.head, &sch_dir_b)?;
    let pcb_b = kicad::extract_pcb(&repo, &args.head, &pcb_dir_b)?;

    let (sch_render_a, sch_render_b) = kicad::changed_paths(&schematics_a, &schematics_b);
    let (pcb_render_a, pcb_render_b) = kicad::changed_paths(&pcb_a, &pcb_b);

    kicad::render_schematics(&sch_dir_a, &svg_sch_dir_a, &sch_render_a)?;
    kicad::render_schematics(&sch_dir_b, &svg_sch_dir_b, &sch_render_b)?;

    kicad::render_pcbs(&pcb_dir_a, &svg_pcb_dir_a, &pcb_render_a)?;
    kicad::render_pcbs(&pcb_dir_b, &svg_pcb_dir_b, &pcb_render_b)?;

    template::generate_site(
        &args.out,
        &args.base,
        &args.head,
        &svg_sch_dir_a,
        &svg_sch_dir_b,
        &svg_pcb_dir_a,
        &svg_pcb_dir_b,
    )?;

    // Sources are only needed for the render step; keep the artifact small.
    let _ = std::fs::remove_dir_all(&sch_dir_a);
    let _ = std::fs::remove_dir_all(&sch_dir_b);
    let _ = std::fs::remove_dir_all(&pcb_dir_a);
    let _ = std::fs::remove_dir_all(&pcb_dir_b);

    log::info!("Done. Open '{}/index.html'", args.out.display());
    Ok(())
}

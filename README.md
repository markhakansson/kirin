# kirin

Generate visual diff reports for KiCAD projects - outputs a self-contained
static page, no server required. Built for CI/CD review workflows.

Inspired by the work of [leoheck/kiri](https://github.com/leoheck/kiri).
If you're looking for a tool with more interactivity and features, go there.

## Limitations

- Only looks for changes between two commits, not a range of commits (i.e., only `b c` not `a..c`)

## Requirements

- KiCAD 10+
  - May work on older versions, but there is no intention to support any other versions than the latest major release

## Usage

```sh
kirin [--repo <dir>] [--base <ref>] [--head <ref>] [--project-dir <path>] [--out <dir>]
```

All options have defaults (`--repo .`, `--base HEAD~1`, `--head HEAD`,
`--out kirin-out`), so a bare `kirin` diffs the last commit of the current
repository.

Projects are discovered from `.kicad_pro` files. For each project, kirin
exports the root schematic hierarchy (one page per sheet) and the curated
PCB layers (copper plus silk/mask/paste/fab and `Edge.Cuts`). Only pages
that changed visually between the two revisions are included; the
`Edge.Cuts` outline is overlaid as context behind each PCB layer in the
viewer.

On a monorepo with several boards, `--project-dir <path>` restricts the
report to projects under that repo-relative path, e.g. `--project-dir anchor`.

## Viewer

The report opens to a sidebar of changed pages (grouped by project, then
schematics and PCB layers) and a pan/zoom stage. Scroll to zoom, drag to
pan, `f` or double-click to fit, `0` to reset to 1:1, and `j`/`k` to step
through pages.

Six compare modes, switchable with keys `1`-`6` or the toolbar:

1. **Base** - the old revision only.
2. **Head** - the new revision only (hold `Space` to flip between base/head).
3. **Onion** - base with head overlaid at half opacity.
4. **Swipe** - a draggable divider, old on the left, new on the right.
5. **Red/Green** - base tinted red, head tinted green: unchanged geometry
   is gray, removed content is red and added content is green.
6. **Blink** - alternates base/head on a timer; good for spotting moves.

## License

Dual-licensed under MIT ([LICENSE-MIT](LICENSE-MIT)) or Apache-2.0 ([LICENSE-APACHE](LICENSE-APACHE)) at your option.

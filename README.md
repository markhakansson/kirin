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
that changed visually between the two revisions are included; each
revision's own `Edge.Cuts` outline is overlaid as context behind that
revision's PCB layers in the viewer.

Libraries are diffed too: footprints (`*.pretty` directories of
`*.kicad_mod` files) and symbols (`*.kicad_sym`), one page per changed
footprint or symbol unit, grouped by library. Changes that do not affect
the rendered image (e.g. 3D model paths) are dropped.

Beyond the visual diff, parts are compared semantically by their KiCAD
UUIDs. Each fact is reported in the domain that owns it: added, removed
and renamed parts in both, value and property changes on the schematic
(the assigned footprint, MPN, LCSC - any field except documentation-only
ones, so custom BOM fields count too), moves and rotations on the board,
library swaps in whichever file they occur. A
part that switched board side is reported under both copper layers,
marking where it left and where it landed. The
viewer nests them in the sidebar under the page they happened on,
collapsed behind a summary line ("5 changes · 38% of parts changed")
and rendered as a tree: one header row per part, its changes indented
beneath, ordered sneakiest first (properties, then nets, then the
visible rest) across parts and within each one; clicking a change (or
stepping with `n`/`p`, which expands the group it enters) jumps to that
page, zooms in and marks the exact location, and clicking it again
releases it. Parts that were deleted and re-placed
under the same reference are matched up and reported by their actual
differences rather than as a remove/add pair.

Electrical connectivity is compared as well, from netlists exported per
revision. Net names take no part in the comparison - nets are matched by
the pins they connect, so renaming a label is not a change - and only
pins whose connections actually changed are reported ("Pin 2:
TJA_CONFIG1 -> +3V3"), with the marker pointing at the pin itself.

On a monorepo with several boards, `--project-dir <path>` restricts the
report to projects under that repo-relative path, e.g. `--project-dir anchor`.

## Viewer

The report opens to a sidebar of changed pages (grouped by project, then
schematics and PCB layers) and a pan/zoom stage. Pages fit to the
viewport when opened. Scroll to zoom, drag to pan, `f` or double-click
to fit, `0` to reset to 1:1, and `j`/`k` to step through pages.

The address bar follows the view: the current page, focused change and
compare mode live in the URL hash, so copying it gives a colleague a
link that opens on the same change, highlighted and zoomed. Changes are
addressed by reference and detail, so links survive regenerating the
report.

Five compare modes, switchable with keys `1`-`5` or the toolbar:

1. **Base** - the old revision only.
2. **Head** - the new revision only (hold `Space` to flip between base/head).
3. **Swipe** - a draggable divider, new on the left, old on the right.
4. **Red/Green** - base tinted red, head tinted green: unchanged geometry
   is gray, removed content is red and added content is green.
5. **Blink** - alternates base/head on a timer; good for spotting moves.

## License

Dual-licensed under MIT ([LICENSE-MIT](LICENSE-MIT)) or Apache-2.0 ([LICENSE-APACHE](LICENSE-APACHE)) at your option.

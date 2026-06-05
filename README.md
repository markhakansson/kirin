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
kirin <repo> <base> <head> [--out <dir>]
```

## License

Dual-licensed under MIT ([LICENSE-MIT](LICENSE-MIT)) or Apache-2.0 ([LICENSE-APACHE](LICENSE-APACHE)) at your option.

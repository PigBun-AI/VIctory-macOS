# symptom and cost columns (format)

Used by `docs/metal/translation-matrix.csv.txt` (columns `symptom`, `cost`) and, when the Mac integration lead adds them, by
`docs/mac/translation-matrix-mac.csv.txt` with the same text. `tools/metal/matrix.sh` reads them by header name, checks the
format of the rows of this file, and writes `docs/metal/translation-priority.md`.

## symptom

`geomN colorN flickerN slowN crashN` — five kinds, in this order, each 0–3, separated by one space. Then, optionally and in any
order, separated by spaces:

| tag | meaning |
|---|---|
| `估` | the five scores are an estimate (cannot be judged from evidence: score 1 and mark 估) |
| `freq=floor` | the row's `freq` is words only, but the symptom is measured and reproducible (long-run crash, white screen, slow frames): freq counts as 10,000 (every frame) |
| `closed` | fixed today, waiting for a re-test: the row is not ranked |
| `summary` | the row stands for rows of their own: not ranked |

Kinds: `geom` missing geometry · `color` wrong, white or dark color · `flicker` flicker, an unstable picture · `slow` slow frames ·
`crash` crash. Scores: 0 the node cannot cause it, 1 a rare or small effect, 2 a clear effect, 3 the picture or the run is lost.
Score by what the node most likely does when it is wrong. When it cannot be judged, take 1 and add `估`.

Example: `geom2 color0 flicker1 slow0 crash3`, `geom0 color3 flicker2 slow0 crash0 估`.

## cost

Person-hours to close the node, a number (`0.5`, `8`, `24`). Taken from `research/map-plan/plan.tsv` (1 d = 8 h; a range takes its
middle; `人时` as written; a plan with only a text scheme counts the days written). A row nobody is on and the plan has no number for
takes `4估`. `估` after the number means the number is an estimate. Proven rows have no cost.

## priority (computed, not stored)

priority = log10(freq + 1) × max(five scores) ÷ cost

* freq: the `freq` column. Words only or unknown counts as 1 (`freq=unknown`); a row tagged `freq=floor` and with no number counts
  as 10,000. No separate P0 list: everything is sorted by this one formula.
* cost: below 0.25 counts as 0.25.
* Ranked: rows with used ≠ no, status ≠ proven, not tagged `closed` or `summary`, and not marked 仅参照（D3DMetal）. The rest are listed
  at the end of the view with the reason.
* plan.tsv view (research/map-plan/plan-ranked.tsv): the same formula over the plan's nodes; a node with no scored row gets a family estimate marked 估; `待估(≥1 d)` costs 8估; `并入 X` takes X's cost; the first 20 are in `plan-ranked.md`.
* Mac rows: the Mac file's own columns when it has them; until then `docs/metal/symptom-overlay-mac.tsv.txt` (the Mac lead's scores of
  2026-11-28). On the same id the Mac row wins, and so does its score.

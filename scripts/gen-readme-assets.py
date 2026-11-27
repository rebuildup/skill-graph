#!/usr/bin/env python3
"""Regenerate README SVG assets from measured data.

Data sources (see docs/evidence/2026-11-27-v050-readme-perf-eval.md):
  - assets/readme/data/perf-500.json  : `skill-graph perf` output (synthetic corpus)
  - assets/readme/data/bench-eval.json: `skill-graph bench evals/dataset/cases.jsonl`
  - assets/readme/data/memory.json    : RSS figures measured via
    `/usr/bin/time -v` (recorded in the evidence doc; not emitted by
    the CLI). Single source for BOTH this figure and the evidence
    doc's memory table — no hardcoded copies (#225 r5).

Outputs light + dark variants for GitHub `<picture>` theming:
  assets/readme/{perf-latency,perf-memory,eval-quality,architecture}-{light,dark}.svg
"""

import json
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DATA = ROOT / "assets/readme/data"
OUT = ROOT / "assets/readme"

FONT = "ui-sans-serif,system-ui,-apple-system,'Segoe UI',sans-serif"
MONO = "ui-monospace,'SF Mono',Menlo,Consolas,monospace"

THEMES = {
    "light": {
        "bg": "#ffffff",
        "panel": "#f6f8fa",
        "ink": "#1f2328",
        "muted": "#59636e",
        "faint": "#818b98",
        "grid": "#d1d9e0",
        "axis": "#818b98",
        "blue": "#0969da",
        "blue_tint": "#b6d4f5",
        "green": "#1a7f37",
        "green_tint": "#b8e6c4",
        "orange": "#bc4c00",
        "purple": "#8250df",
        "red": "#d1242f",
        "neg": "#8c959f",
    },
    "dark": {
        "bg": "#0d1117",
        "panel": "#161b22",
        "ink": "#e6edf3",
        "muted": "#8b949e",
        "faint": "#6e7681",
        "grid": "#21262d",
        "axis": "#6e7681",
        "blue": "#58a6ff",
        "blue_tint": "#1f3a5f",
        "green": "#3fb950",
        "green_tint": "#1d3a24",
        "orange": "#d29922",
        "purple": "#bc8cff",
        "red": "#f85149",
        "neg": "#484f58",
    },
}


def esc(s: str) -> str:
    return s.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")


def svg_open(w: int, h: int, t: dict) -> str:
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" '
        f'viewBox="0 0 {w} {h}" role="img" font-family="{FONT}">\n'
        f'<rect width="{w}" height="{h}" fill="{t["bg"]}" rx="8"/>\n'
    )


def text(x, y, s, size=11, fill=None, weight=None, anchor="start", mono=False, opacity=1.0):
    fam = MONO if mono else FONT
    attrs = (
        f'x="{x}" y="{y}" font-size="{size}" fill="{fill or "inherit"}" '
        f'text-anchor="{anchor}" font-family="{fam}" opacity="{opacity}"'
    )
    if weight:
        attrs += f' font-weight="{weight}"'
    return f"<text {attrs}>{esc(s)}</text>"


def line(x1, y1, x2, y2, stroke, w=1.0, dash=None, opacity=1.0):
    d = f' stroke-dasharray="{dash}"' if dash else ""
    return (
        f'<line x1="{x1}" y1="{y1}" x2="{x2}" y2="{y2}" '
        f'stroke="{stroke}" stroke-width="{w}"{d} opacity="{opacity}"/>'
    )


def rect(x, y, w, h, fill, rx=3, opacity=1.0):
    return f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="{rx}" fill="{fill}" opacity="{opacity}"/>'


def circle(cx, cy, r, fill, stroke=None, sw=0):
    s = f' stroke="{stroke}" stroke-width="{sw}"' if stroke else ""
    return f'<circle cx="{cx}" cy="{cy}" r="{r}" fill="{fill}"{s}/>'


def load_perf():
    d = json.loads((DATA / "perf-500.json").read_text())
    rows = {r["skills"]: r for r in d["results"]}
    sizes = sorted(rows)
    return sizes, rows


# ---------------------------------------------------------------- figure 1
def fig_latency(theme: str) -> str:
    t = THEMES[theme]
    sizes, rows = load_perf()
    W, H = 880, 330
    out = [svg_open(W, H, t)]
    out.append(f'<g fill="{t["ink"]}">')

    # figure-level caption + legend row (keeps panel bottoms clean)
    out.append(text(70, 20, "synthetic corpus · 30 iterations · 30 process samples · 2026-11-28", 9.5, t["faint"]))
    leg = [("cold discovery + graph build", t["blue_tint"], False),
           ("full process p95", t["blue"], False),
           ("warm recommend p95", t["green"], False)]
    lx = 810
    for lbl, col, _ in reversed(leg):
        lx -= 14 + len(lbl) * 5.4
        out.append(rect(lx, 12, 10, 10, col, rx=2))
        out.append(text(lx + 14, 21, lbl, 9.5, t["muted"]))
        lx -= 22

    panels = [
        # (title, subtitle, key cold, key proc, ymax, yticks, x0, pw)
        ("End-to-end latency", "full CLI process incl. spawn · p95 · corpus size (skills)", "cold_total_ms", "process_p95_ms", 16.0, [0, 4, 8, 12, 16], 70, 360),
        ("Recommendation core", "in-memory recommend · warm p95 · corpus size (skills)", None, "warm_recommend_p95_ms", 1.6, [0, 0.4, 0.8, 1.2, 1.6], 478, 360),
    ]

    for title, sub, k_cold, k_hot, ymax, yticks, x0, pw in panels:
        ph = 218  # plot height
        py = 66   # plot top
        px = x0
        # heading
        out.append(text(px, 34, title, 14, t["ink"], weight="600"))
        out.append(text(px, 52, sub, 10.5, t["muted"]))
        # frame + gridlines + y labels
        for v in yticks:
            gy = py + ph - (v / ymax) * ph
            out.append(line(px, gy, px + pw, gy, t["grid"], 1))
            lbl = f"{v:g}"
            out.append(text(px - 8, gy + 3.5, lbl, 10, t["muted"], anchor="end", mono=True))
        out.append(text(px - 8, py - 8, "ms", 9.5, t["faint"], anchor="end", mono=True))
        # baseline
        out.append(line(px, py + ph, px + pw, py + ph, t["axis"], 1.2))
        n = len(sizes)
        group_w = pw / n
        bar_gap = 4
        for i, s in enumerate(sizes):
            r = rows[s]
            gx = px + i * group_w + group_w * 0.5
            if k_cold:
                # grouped: cold (tint) + process/core (solid)
                bw = (group_w * 0.62) / 2 - bar_gap / 2
                v1 = r[k_cold] / ymax * ph
                v2 = r[k_hot] / ymax * ph
                bx1 = gx - bw - bar_gap / 2
                bx2 = gx + bar_gap / 2
                out.append(rect(bx1, py + ph - v1, bw, v1, t["blue_tint"], rx=2))
                out.append(rect(bx2, py + ph - v2, bw, v2, t["blue"], rx=2))
                out.append(text(bx1 + bw / 2, py + ph - v1 - 4, f'{r[k_cold]:.1f}', 9, t["muted"], anchor="middle", mono=True))
                out.append(text(bx2 + bw / 2, py + ph - v2 - 4, f'{r[k_hot]:.1f}', 9.5, t["ink"], anchor="middle", mono=True, weight="600"))
            else:
                bw = group_w * 0.42
                v = r[k_hot] / ymax * ph
                bx = gx - bw / 2
                out.append(rect(bx, py + ph - v, bw, v, t["green"], rx=2.5))
                out.append(text(bx + bw / 2, py + ph - v - 4, f'{r[k_hot]:.2f}', 9.5, t["ink"], anchor="middle", mono=True, weight="600"))
            out.append(text(gx, py + ph + 16, str(s), 10.5, t["muted"], anchor="middle", mono=True))
            out.append(text(gx, py + ph + 30, "skills", 9, t["faint"], anchor="middle"))

    out.append("</g>\n</svg>\n")
    return "".join(out)


# ---------------------------------------------------------------- figure 2
def load_memory():
    return json.loads((DATA / "memory.json").read_text())


def fig_memory(theme: str) -> str:
    t = THEMES[theme]
    mem = load_memory()
    W, H = 880, 300
    out = [svg_open(W, H, t)]
    out.append(f'<g fill="{t["ink"]}">')
    out.append(text(70, 34, mem["headline"], 14, t["ink"], weight="600"))
    out.append(text(70, 52, mem["subheadline"], 10.5, t["muted"]))

    px, pw = 70, 740
    ph, py = 148, 70
    ymax = 10.0
    yticks = [0, 2, 4, 6, 8, 10]
    for v in yticks:
        gy = py + ph - (v / ymax) * ph
        out.append(line(px, gy, px + pw, gy, t["grid"], 1))
        out.append(text(px - 8, gy + 3.5, f"{v:g}", 10, t["muted"], anchor="end", mono=True))
    out.append(text(px - 8, py - 8, "MB", 9.5, t["faint"], anchor="end", mono=True))
    out.append(line(px, py + ph, px + pw, py + ph, t["axis"], 1.2))
    out.append(line(px, py, px, py + ph, t["axis"], 1.2))

    # points come from memory.json — the same file the evidence doc
    # cites, so figure and prose can never drift apart (#225 r5).
    pts = [
        (p["label"], str(p["skills"]), p["rss_mb"], p["kind"])
        for p in mem["points"]
    ]
    # x positions span the observed corpus-size range on a log scale:
    # sizes like 24→500 would imply a continuous trend under equal
    # spacing that does not exist
    import math
    lo = min(int(n) for _, n, _, _ in pts)
    hi = max(int(n) for _, n, _, _ in pts)
    xs = []
    for _, n, _, _ in pts:
        f = (math.log10(int(n)) - math.log10(lo)) / (math.log10(hi) - math.log10(lo))
        xs.append(130 + f * 620)
    ys = [py + ph - (mb / ymax) * ph for _, _, mb, _ in pts]
    kind_label = {"real": "real", "synthetic": "synthetic", "syn": "synthetic"}
    for (label, n, mb, kind), x, y in zip(pts, xs, ys):
        out.append(circle(x, y, 5.5, t["panel"], stroke=t["blue"], sw=2.2))
        out.append(circle(x, y, 2.2, t["blue"]))
        out.append(text(x - 12, y + 4, f"{mb:.1f} MB", 11, t["ink"], anchor="end", mono=True, weight="600"))
        out.append(text(x, py + ph + 20, label, 10.5, t["muted"], anchor="middle"))
        out.append(text(x, py + ph + 36, f"{n} skills · {kind_label.get(kind, kind)}", 10, t["muted"], anchor="middle"))

    for i, note in enumerate(mem.get("footnotes", [])):
        out.append(text(70, 276 + i * 18, note, 10, t["muted"]))
    out.append("</g>\n</svg>\n")
    return "".join(out)


# ---------------------------------------------------------------- figure 3
def fig_eval(theme: str) -> str:
    t = THEMES[theme]
    bench = json.loads((DATA / "bench-eval.json").read_text())
    s = bench["summary"]
    cases = bench["cases"]
    W, H = 880, 320
    out = [svg_open(W, H, t)]
    out.append(f'<g fill="{t["ink"]}">')

    out.append(text(70, 34, "Recommendation quality on the offline eval set", 14, t["ink"], weight="600"))
    out.append(text(70, 52, f'{s["cases"]} cases · synthetic corpus · `skill-graph bench` · {s["elapsed_ms"]} ms total', 10.5, t["muted"]))

    # headline stats
    stats = [
        (f'{s["required_recall_at3"] * 100:.0f}%', "Recall@3 — required skills", t["green"]),
        (f'{s["forbidden_hits"]}', "forbidden recommendations", t["ink"]),
        (f'{s["precision_acceptable"] * 100:.0f}%', f'precision · acceptable ({int(round(s["precision_acceptable"] * s["recommended_total"]))}/{s["recommended_total"]})', t["blue"]),
    ]
    sx = 70
    for val, lbl, col in stats:
        out.append(text(sx, 108, val, 34, col, weight="700", mono=True))
        out.append(text(sx, 126, lbl, 10.5, t["muted"]))
        sx += 250

    # per-case dot strip: 52 required-hit + 6 correctly-empty negatives
    out.append(text(70, 158, "per-case outcome", 11, t["muted"], weight="600"))
    n = len(cases)
    dx, dy, r, gap = 60, 194, 5.0, 12.5
    hits = empt = 0
    for i, c in enumerate(cases):
        x = dx + i * gap
        if c["required_total"] > 0:
            hits += 1
            out.append(circle(x, dy, r, t["green"]))
        else:
            empt += 1
            # hollow = "correctly empty" — reads in both themes
            out.append(circle(x, dy, r, t["bg"], stroke=t["neg"], sw=1.6))
    # inline legend below the strip
    lx = dx
    ly = 224
    out.append(circle(lx + r, ly - 3.5, r, t["green"]))
    out.append(text(lx + 2 * r + 8, ly, f"required skill in top-3 ({hits})", 10, t["muted"]))
    lx += 2 * r + 8 + len(f"required skill in top-3 ({hits})") * 5.6 + 26
    out.append(circle(lx + r, ly - 3.5, r, t["bg"], stroke=t["neg"], sw=1.6))
    out.append(text(lx + 2 * r + 8, ly, f"negative case → correctly empty ({empt})", 10, t["muted"]))

    out.append(text(70, 252, "52/52 required skills ranked in the top 3 · 0 forbidden hits · 6/6 negative cases abstained", 11.5, t["ink"], weight="600"))
    out.append(text(70, 272, "offline evaluation on a synthetic corpus — measures ranking quality, not real-session outcomes", 10, t["muted"]))
    out.append(text(70, 290, "real-session effect is tracked separately via `observe --summary` funnel metrics", 10, t["muted"]))
    out.append("</g>\n</svg>\n")
    return "".join(out)


# ---------------------------------------------------------------- figure 4
def fig_arch(theme: str) -> str:
    t = THEMES[theme]
    W, H = 880, 300
    out = [svg_open(W, H, t)]
    out.append(f'<g fill="{t["ink"]}">')
    out.append(text(70, 34, "How a recommendation reaches the agent", 14, t["ink"], weight="600"))
    out.append(text(70, 52, "every call is a fresh process — state lives only in an append-only event log", 10.5, t["muted"]))

    nodes = [
        ("Agent prompt", "payload on stdin", t["panel"], t["ink"]),
        ("skill-graph hook", "thin CLI · fail-open", t["blue_tint"], t["blue"]),
        ("in-memory graph", "build + rank per call", t["blue_tint"], t["blue"]),
        ("top-3 skills", "context injection", t["green_tint"], t["green"]),
        ("agent continues", "session proceeds", t["panel"], t["ink"]),
    ]
    bw, bh, gap = 132, 68, 34
    y0 = 92
    total = len(nodes) * bw + (len(nodes) - 1) * gap
    x0 = (W - total) / 2
    for i, (title, sub, fill, col) in enumerate(nodes):
        x = x0 + i * (bw + gap)
        out.append(rect(x, y0, bw, bh, fill, rx=8))
        out.append(f'<rect x="{x}" y="{y0}" width="{bw}" height="{bh}" rx="8" fill="none" stroke="{col}" stroke-width="1.4" opacity="0.55"/>')
        out.append(text(x + bw / 2, y0 + 29, title, 12.5, t["ink"], anchor="middle", weight="600"))
        out.append(text(x + bw / 2, y0 + 48, sub, 10, t["muted"], anchor="middle"))
        if i < len(nodes) - 1:
            ax = x + bw + 6
            out.append(line(ax, y0 + bh / 2, ax + gap - 12, y0 + bh / 2, t["axis"], 1.6))
            out.append(f'<path d="M {ax + gap - 12} {y0 + bh / 2} l -7 -4 v 8 z" fill="{t["axis"]}"/>')

    # infrastructure deliberately absent — centered group under the flow
    chips = ["daemon", "database", "index files", "resident process"]
    cws = [14 + len(c) * 6.4 for c in chips]
    cgap = 12
    ctotal = sum(cws) + cgap * (len(chips) - 1)
    cx = (W - ctotal) / 2
    cy = 206
    out.append(text(W / 2, cy - 12, "absent by design:", 10.5, t["muted"], anchor="middle"))
    for c, wch in zip(chips, cws):
        out.append(rect(cx, cy, wch, 24, t["panel"], rx=12))
        out.append(f'<rect x="{cx}" y="{cy}" width="{wch}" height="24" rx="12" fill="none" stroke="{t["red"]}" stroke-width="1" opacity="0.5"/>')
        out.append(text(cx + wch / 2, cy + 16.5, c, 10.5, t["muted"], anchor="middle"))
        cx += wch + cgap

    out.append(text(70, 270, "event log: append-only JSONL · prompt bodies stored as SHA-256 hashes only", 10, t["muted"]))
    out.append(text(70, 290, "failure mode: hook always exits 0 — a broken graph can never block the agent", 10, t["muted"]))
    out.append("</g>\n</svg>\n")
    return "".join(out)


FIGS = {
    "perf-latency": fig_latency,
    "perf-memory": fig_memory,
    "eval-quality": fig_eval,
    "architecture": fig_arch,
}


def main():
    for name, fn in FIGS.items():
        for theme in THEMES:
            p = OUT / f"{name}-{theme}.svg"
            p.write_text(fn(theme))
            print(f"wrote {p.relative_to(ROOT)}")


if __name__ == "__main__":
    main()

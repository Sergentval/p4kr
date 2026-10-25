"""Write the benchmark charts (SVG) from results.csv. `--check` exits 1 if a tracked chart is out of date."""
import csv
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
WIDTH, BAR_H, GAP, LEFT, TOP = 760, 18, 10, 230, 48
COLORS = {"p4kr": "#2f7d4f"}
OTHER = "#9aa0a6"
TITLES = {"open-extract": "Open and extract (warm cache, lower is better)",
          "xml": "Binary XML to text XML (lower is better)",
          "dataforge": "DataForge database to XML (lower is better)"}


def rows(chart: str) -> list[dict]:
    with open(HERE / "results.csv", newline="", encoding="utf-8") as f:
        return [r for r in csv.DictReader(f) if r["chart"] == chart]


def esc(t: str) -> str:
    return t.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")


def svg(chart: str) -> str:
    data = [r for r in rows(chart)
            if r["median_s"] and (chart != "open-extract" or r["cache"] == "warm")]
    peak = max(float(r["median_s"]) for r in data)
    scale = (WIDTH - LEFT - 120) / peak
    groups: dict[str, list[dict]] = {}
    for r in data:
        groups.setdefault(f'{r["label"]} ({r["cache"]})' if chart != "open-extract" else r["label"], []).append(r)
    y, parts = TOP, []
    for label, items in groups.items():
        parts.append(f'<text x="12" y="{y + 12}" class="g">{esc(label)}</text>')
        y += BAR_H
        for r in items:
            w = max(1.0, float(r["median_s"]) * scale)
            color = COLORS.get(r["tool"], OTHER)
            ratio = f' ×{r["ratio"]}' if r["tool"] != "p4kr" and r["ratio"] else ""
            parts.append(f'<rect x="{LEFT}" y="{y}" width="{w:.1f}" height="{BAR_H}" fill="{color}" rx="2"/>')
            parts.append(f'<text x="{LEFT - 6}" y="{y + 13}" class="t" text-anchor="end">{esc(r["tool"])}</text>')
            parts.append(f'<text x="{LEFT + w + 6:.1f}" y="{y + 13}" class="v">{float(r["median_s"]):.2f} s{ratio}</text>')
            y += BAR_H + 4
        y += GAP
    height = y + 16
    head = (f'<svg xmlns="http://www.w3.org/2000/svg" width="{WIDTH}" height="{height}" '
            f'viewBox="0 0 {WIDTH} {height}" font-family="system-ui, -apple-system, Segoe UI, sans-serif">\n'
            '<style>.g{font-size:13px;font-weight:600;fill:#202124}.t{font-size:12px;fill:#3c4043}'
            '.v{font-size:12px;fill:#202124}.h{font-size:15px;font-weight:700;fill:#202124}</style>\n'
            f'<rect width="{WIDTH}" height="{height}" fill="#ffffff" rx="6"/>\n'
            f'<text x="12" y="28" class="h">{esc(TITLES[chart])}</text>\n')
    return head + "\n".join(parts) + "\n</svg>\n"


def main(argv: list[str]) -> int:
    stale = []
    for chart in TITLES:
        path, text = HERE / f"{chart}.svg", svg(chart)
        if "--check" in argv:
            if not path.is_file() or path.read_text(encoding="utf-8") != text:
                stale.append(path.name)
        else:
            path.write_text(text, encoding="utf-8", newline="\n")
    print("\n".join(stale) or "charts up to date")
    return 1 if stale else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

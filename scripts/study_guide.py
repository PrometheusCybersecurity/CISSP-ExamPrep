"""
CISSP Coach — Study Guide PDF generator.

Reads a JSON payload on stdin describing a finished batch's missed questions
plus an LLM-synthesised "study notes" markdown blob, and writes a finished
PDF to stdout. Designed to be spawned as a subprocess by the Rust server so
the rest of the application stays in Rust.

JSON input shape (all fields required unless noted):
    {
      "batch_id":     "<string>",
      "generated_at": "<ISO 8601 UTC>",
      "total":        <int>,                  // total in batch
      "answered":     <int>,
      "correct":      <int>,
      "accuracy_pct": <float, 0..100>,
      "synthesis_md": "<markdown string from the LLM, may be empty>",
      "missed": [                              // grouped/ordered by domain
        {
          "id":          "<string>",
          "domain":      <int 1..8>,
          "domain_name": "<string>",
          "tier":        <int 1..4>,
          "tier_name":   "<string>",
          "subtopic":    "<string or null>",
          "question":    "<string>",
          "options":     {"A": "...", "B": "...", "C": "...", "D": "..."},
          "user_answer": "<A|B|C|D|null>",
          "correct":     "<A|B|C|D>",
          "explanation": "<string or null>"
        }, ...
      ]
    }

Exit codes:
    0   PDF written to stdout
    2   bad/missing JSON on stdin
    3   reportlab not installed
    4   render error
"""

from __future__ import annotations

import io
import json
import re
import sys
from collections import OrderedDict
from html import escape


def _fail(code: int, msg: str) -> None:
    sys.stderr.write(msg.rstrip() + "\n")
    sys.exit(code)


try:
    from reportlab.lib import colors
    from reportlab.lib.enums import TA_LEFT
    from reportlab.lib.pagesizes import LETTER
    from reportlab.lib.styles import ParagraphStyle, getSampleStyleSheet
    from reportlab.lib.units import inch
    from reportlab.platypus import (
        KeepTogether,
        PageBreak,
        Paragraph,
        SimpleDocTemplate,
        Spacer,
        Table,
        TableStyle,
    )
except ImportError as e:  # pragma: no cover
    _fail(
        3,
        "reportlab is not installed. Run:  python -m pip install -r requirements.txt"
        f"\n(import error: {e})",
    )


# ─── Markdown → ReportLab inline HTML ──────────────────────────────────────
# ReportLab's Paragraph supports a small subset of HTML: <b>, <i>, <u>,
# <font color="...">, <br/>. This converter handles the markdown features
# the LLM actually emits (bold, italic, inline code, headings, bullets)
# without pulling in a full markdown library.

_BOLD     = re.compile(r"\*\*(.+?)\*\*")
_ITALIC   = re.compile(r"(?<!\*)\*(?!\*)(.+?)\*(?!\*)")
_CODE     = re.compile(r"`([^`]+)`")
_HEADING  = re.compile(r"^(#{1,4})\s+(.*)$")
_BULLET   = re.compile(r"^\s*[-*•]\s+(.*)$")
_NUMBERED = re.compile(r"^\s*\d+\.\s+(.*)$")


def _md_inline(text: str) -> str:
    """Convert inline markdown to ReportLab paragraph HTML."""
    out = escape(text, quote=False)
    out = _BOLD.sub(r"<b>\1</b>", out)
    out = _ITALIC.sub(r"<i>\1</i>", out)
    out = _CODE.sub(
        r'<font face="Courier" color="#0a4a82">\1</font>', out
    )
    return out


def _md_to_flowables(md: str, styles) -> list:
    """Convert a markdown blob to a list of Platypus flowables."""
    flowables: list = []
    if not md or not md.strip():
        return flowables

    para_buf: list[str] = []

    def flush_para() -> None:
        if not para_buf:
            return
        text = " ".join(p.strip() for p in para_buf if p.strip())
        if text:
            flowables.append(Paragraph(_md_inline(text), styles["Body"]))
            flowables.append(Spacer(1, 4))
        para_buf.clear()

    for raw_line in md.splitlines():
        line = raw_line.rstrip()

        if not line.strip():
            flush_para()
            continue

        h = _HEADING.match(line)
        if h:
            flush_para()
            level = len(h.group(1))
            text = h.group(2).strip()
            style_name = "H1" if level <= 1 else ("H2" if level == 2 else "H3")
            flowables.append(Paragraph(_md_inline(text), styles[style_name]))
            flowables.append(Spacer(1, 4))
            continue

        b = _BULLET.match(line)
        if b:
            flush_para()
            flowables.append(
                Paragraph("• " + _md_inline(b.group(1)), styles["Bullet"])
            )
            continue

        n = _NUMBERED.match(line)
        if n:
            flush_para()
            flowables.append(
                Paragraph("• " + _md_inline(n.group(1)), styles["Bullet"])
            )
            continue

        para_buf.append(line)

    flush_para()
    return flowables


# ─── Style sheet ───────────────────────────────────────────────────────────


def _make_styles():
    base = getSampleStyleSheet()
    accent = colors.HexColor("#1d4ed8")
    text_dark = colors.HexColor("#0f172a")
    text_muted = colors.HexColor("#64748b")
    correct_green = colors.HexColor("#047857")
    wrong_red = colors.HexColor("#b91c1c")

    s = {
        "TitleBig": ParagraphStyle(
            "TitleBig",
            parent=base["Title"],
            fontName="Helvetica-Bold",
            fontSize=26,
            leading=30,
            textColor=accent,
            spaceAfter=8,
            alignment=TA_LEFT,
        ),
        "Subtitle": ParagraphStyle(
            "Subtitle",
            parent=base["Normal"],
            fontName="Helvetica",
            fontSize=11,
            leading=15,
            textColor=text_muted,
            spaceAfter=4,
        ),
        "H1": ParagraphStyle(
            "H1",
            parent=base["Heading1"],
            fontName="Helvetica-Bold",
            fontSize=16,
            leading=20,
            textColor=accent,
            spaceBefore=10,
            spaceAfter=6,
        ),
        "H2": ParagraphStyle(
            "H2",
            parent=base["Heading2"],
            fontName="Helvetica-Bold",
            fontSize=13,
            leading=17,
            textColor=accent,
            spaceBefore=8,
            spaceAfter=5,
        ),
        "H3": ParagraphStyle(
            "H3",
            parent=base["Heading3"],
            fontName="Helvetica-Bold",
            fontSize=11,
            leading=14,
            textColor=text_dark,
            spaceBefore=6,
            spaceAfter=3,
        ),
        "DomainHeader": ParagraphStyle(
            "DomainHeader",
            parent=base["Heading1"],
            fontName="Helvetica-Bold",
            fontSize=15,
            leading=19,
            textColor=colors.white,
            backColor=accent,
            borderPadding=(6, 8, 6, 8),
            leftIndent=0,
            spaceBefore=12,
            spaceAfter=10,
        ),
        "Body": ParagraphStyle(
            "Body",
            parent=base["Normal"],
            fontName="Helvetica",
            fontSize=10.5,
            leading=14.5,
            textColor=text_dark,
            spaceAfter=2,
        ),
        "Bullet": ParagraphStyle(
            "Bullet",
            parent=base["Normal"],
            fontName="Helvetica",
            fontSize=10.5,
            leading=14.5,
            leftIndent=14,
            textColor=text_dark,
            spaceAfter=2,
        ),
        "QStem": ParagraphStyle(
            "QStem",
            parent=base["Normal"],
            fontName="Helvetica",
            fontSize=10.5,
            leading=14.5,
            textColor=text_dark,
            spaceAfter=6,
        ),
        "QMeta": ParagraphStyle(
            "QMeta",
            parent=base["Normal"],
            fontName="Helvetica-Oblique",
            fontSize=9,
            leading=12,
            textColor=text_muted,
            spaceAfter=4,
        ),
        "OptCorrect": ParagraphStyle(
            "OptCorrect",
            parent=base["Normal"],
            fontName="Helvetica-Bold",
            fontSize=10,
            leading=13,
            textColor=correct_green,
            leftIndent=14,
            spaceAfter=2,
        ),
        "OptWrongPicked": ParagraphStyle(
            "OptWrongPicked",
            parent=base["Normal"],
            fontName="Helvetica",
            fontSize=10,
            leading=13,
            textColor=wrong_red,
            leftIndent=14,
            spaceAfter=2,
        ),
        "OptOther": ParagraphStyle(
            "OptOther",
            parent=base["Normal"],
            fontName="Helvetica",
            fontSize=10,
            leading=13,
            textColor=text_dark,
            leftIndent=14,
            spaceAfter=2,
        ),
        "Explanation": ParagraphStyle(
            "Explanation",
            parent=base["Normal"],
            fontName="Helvetica",
            fontSize=10,
            leading=14,
            textColor=text_dark,
            leftIndent=8,
            rightIndent=8,
            spaceBefore=4,
            spaceAfter=8,
            backColor=colors.HexColor("#f1f5f9"),
            borderPadding=(6, 8, 6, 8),
        ),
        "Cover": ParagraphStyle(
            "Cover",
            parent=base["Normal"],
            fontName="Helvetica",
            fontSize=11,
            leading=16,
            textColor=text_dark,
        ),
    }
    return s


# ─── Page chrome (header / footer) ─────────────────────────────────────────


def _draw_page_chrome(canvas, doc) -> None:
    canvas.saveState()
    canvas.setFont("Helvetica", 8)
    canvas.setFillColor(colors.HexColor("#94a3b8"))
    page_w, page_h = LETTER
    # Top-left brand
    canvas.drawString(0.5 * inch, page_h - 0.35 * inch, "🛡 CISSP Coach — Study Guide")
    # Bottom-right page number
    canvas.drawRightString(
        page_w - 0.5 * inch,
        0.35 * inch,
        f"Page {doc.page}",
    )
    canvas.restoreState()


# ─── Document assembly ─────────────────────────────────────────────────────


def _accuracy_color(pct: float):
    if pct >= 75:
        return colors.HexColor("#047857")  # green
    if pct >= 50:
        return colors.HexColor("#b45309")  # amber
    return colors.HexColor("#b91c1c")      # red


def _build_cover(payload: dict, styles) -> list:
    flowables: list = []
    scope = (payload.get("scope") or "batch").lower()
    if scope == "all-time":
        title = "CISSP — All-Time Misses Study Guide"
        miss_count = payload.get("miss_count") or len(payload.get("missed") or [])
        capped_note = " (most recent)" if payload.get("capped") else ""
        subtitle = (
            f"Generated {escape(payload.get('generated_at', ''))} · "
            f"<b>{miss_count}</b> missed question{'s' if miss_count != 1 else ''}{escape(capped_note)} "
            f"across all batches"
        )
    else:
        title = "CISSP Adaptive Quiz — Study Guide"
        subtitle = (
            f"Generated {escape(payload.get('generated_at', ''))} · "
            f"Batch <font face='Courier'>{escape(payload.get('batch_id', ''))}</font>"
        )
    flowables.append(Paragraph(title, styles["TitleBig"]))
    flowables.append(Paragraph(subtitle, styles["Subtitle"]))
    flowables.append(Spacer(1, 14))

    # Big accuracy number + summary table
    pct = float(payload.get("accuracy_pct", 0.0))
    pct_color = _accuracy_color(pct)
    big = ParagraphStyle(
        "Big", parent=styles["TitleBig"], fontSize=44, leading=48, textColor=pct_color
    )
    flowables.append(Paragraph(f"{pct:.0f}%", big))
    flowables.append(
        Paragraph(
            f"{payload.get('correct', 0)} correct of {payload.get('answered', 0)} answered "
            f"(batch size {payload.get('total', 0)})",
            styles["Subtitle"],
        )
    )
    flowables.append(Spacer(1, 18))

    missed = payload.get("missed") or []
    if missed:
        # Per-domain miss tally
        tally: "OrderedDict[str, int]" = OrderedDict()
        for m in missed:
            key = f"D{m.get('domain', '?')} · {m.get('domain_name', 'Unknown')}"
            tally[key] = tally.get(key, 0) + 1
        rows = [["Domain", "Missed"]]
        for k, v in tally.items():
            rows.append([k, str(v)])
        tbl = Table(rows, colWidths=[3.5 * inch, 1.0 * inch], hAlign="LEFT")
        tbl.setStyle(
            TableStyle(
                [
                    ("BACKGROUND", (0, 0), (-1, 0), colors.HexColor("#1d4ed8")),
                    ("TEXTCOLOR", (0, 0), (-1, 0), colors.white),
                    ("FONTNAME", (0, 0), (-1, 0), "Helvetica-Bold"),
                    ("FONTSIZE", (0, 0), (-1, -1), 10),
                    ("BOTTOMPADDING", (0, 0), (-1, -1), 6),
                    ("TOPPADDING", (0, 0), (-1, -1), 6),
                    ("ROWBACKGROUNDS", (0, 1), (-1, -1), [colors.white, colors.HexColor("#f8fafc")]),
                    ("GRID", (0, 0), (-1, -1), 0.4, colors.HexColor("#cbd5e1")),
                    ("ALIGN", (1, 0), (1, -1), "RIGHT"),
                ]
            )
        )
        flowables.append(Paragraph("Misses by domain", styles["H3"]))
        flowables.append(tbl)
    else:
        flowables.append(
            Paragraph("🔥 No missed questions in this batch — nice work.", styles["Body"])
        )

    return flowables


def _build_synthesis(payload: dict, styles) -> list:
    md = (payload.get("synthesis_md") or "").strip()
    if not md:
        return []
    out: list = [PageBreak(), Paragraph("Study Notes", styles["H1"])]
    out.extend(_md_to_flowables(md, styles))
    return out


def _build_missed_section(payload: dict, styles) -> list:
    missed = payload.get("missed") or []
    if not missed:
        return []
    out: list = [PageBreak(), Paragraph("Missed Questions — Detailed Review", styles["H1"])]

    # Group missed by domain (preserving first-seen order)
    grouped: "OrderedDict[int, list]" = OrderedDict()
    for q in missed:
        d = int(q.get("domain", 0))
        grouped.setdefault(d, []).append(q)

    for domain, items in grouped.items():
        domain_name = items[0].get("domain_name", f"Domain {domain}")
        out.append(
            Paragraph(f"Domain {domain} — {escape(domain_name)}", styles["DomainHeader"])
        )
        for i, q in enumerate(items, 1):
            block: list = []
            tier_name = q.get("tier_name", "")
            subtopic = q.get("subtopic") or "—"
            block.append(
                Paragraph(
                    f"<b>Q{i}.</b> "
                    f"<font color='#64748b'>{escape(tier_name)} · {escape(str(subtopic))}</font>",
                    styles["QMeta"],
                )
            )
            block.append(Paragraph(escape(q.get("question", "")), styles["QStem"]))

            opts = q.get("options") or {}
            user_letter = (q.get("user_answer") or "").upper()
            correct_letter = (q.get("correct") or "").upper()
            for letter in ("A", "B", "C", "D"):
                text = opts.get(letter, "") or ""
                line = f"<b>{letter}.</b> {escape(text)}"
                if letter == correct_letter:
                    line += "  <b>✓ correct</b>"
                    style = styles["OptCorrect"]
                elif letter == user_letter and letter != correct_letter:
                    line += "  <b>✗ your answer</b>"
                    style = styles["OptWrongPicked"]
                else:
                    style = styles["OptOther"]
                block.append(Paragraph(line, style))

            expl = q.get("explanation")
            if expl:
                block.append(
                    Paragraph(
                        f"<b>Why:</b> {escape(expl)}",
                        styles["Explanation"],
                    )
                )
            block.append(Spacer(1, 6))

            # Try to keep each question together; fall back to flowing if too long.
            try:
                out.append(KeepTogether(block))
            except Exception:
                out.extend(block)
    return out


def render(payload: dict) -> bytes:
    buf = io.BytesIO()
    doc = SimpleDocTemplate(
        buf,
        pagesize=LETTER,
        leftMargin=0.75 * inch,
        rightMargin=0.75 * inch,
        topMargin=0.7 * inch,
        bottomMargin=0.7 * inch,
        title=f"CISSP Study Guide — {payload.get('batch_id', '')}",
        author="CISSP Coach",
    )
    styles = _make_styles()

    flow: list = []
    flow.extend(_build_cover(payload, styles))
    flow.extend(_build_synthesis(payload, styles))
    flow.extend(_build_missed_section(payload, styles))

    if not flow:
        flow.append(Paragraph("Empty study guide.", styles["Body"]))

    doc.build(flow, onFirstPage=_draw_page_chrome, onLaterPages=_draw_page_chrome)
    return buf.getvalue()


def main() -> int:
    raw = sys.stdin.buffer.read()
    if not raw:
        _fail(2, "no JSON on stdin")
    try:
        payload = json.loads(raw)
    except json.JSONDecodeError as e:
        _fail(2, f"invalid JSON on stdin: {e}")

    try:
        pdf = render(payload)
    except Exception as e:  # pragma: no cover
        _fail(4, f"reportlab render failed: {e}")

    # Write binary PDF to stdout.
    sys.stdout.buffer.write(pdf)
    sys.stdout.buffer.flush()
    return 0


if __name__ == "__main__":
    sys.exit(main())

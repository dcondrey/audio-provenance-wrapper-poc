#!/usr/bin/env python3
"""Generate packaging/assets/quickstart.pdf (one page, deterministic)."""
from __future__ import annotations

import argparse
from pathlib import Path

from reportlab.lib.colors import HexColor, Color
from reportlab.lib.pagesizes import LETTER
from reportlab.pdfbase.pdfmetrics import stringWidth
from reportlab.pdfgen.canvas import Canvas

W, H = LETTER
MARGIN = 52.0

INK = HexColor("#14171C")
MUTED = HexColor("#6C7581")
FAINT = HexColor("#9AA3AE")
RULE = HexColor("#DCE0E5")
PANEL = HexColor("#F4F6F7")
ACCENT = HexColor("#1F6B58")
ACCENT_SOFT = HexColor("#E4EFEB")
AMBER = HexColor("#B0730B")
RED = HexColor("#9E3535")
WAVE = HexColor("#BFC7CF")

BOLD = "Helvetica-Bold"
REG = "Helvetica"
OBL = "Helvetica-Oblique"


def tracked(c: Canvas, x: float, y: float, text: str, font: str, size: float, spacing: float) -> None:
    c.setFont(font, size)
    for ch in text:
        c.drawString(x, y, ch)
        x += stringWidth(ch, font, size) + spacing


def wrap(text: str, font: str, size: float, width: float) -> list[str]:
    words, lines, cur = text.split(), [], ""
    for w in words:
        trial = f"{cur} {w}".strip()
        if stringWidth(trial, font, size) <= width:
            cur = trial
        else:
            lines.append(cur)
            cur = w
    if cur:
        lines.append(cur)
    return lines


def paragraph(c: Canvas, x: float, y: float, text: str, font: str, size: float,
              width: float, leading: float, color: Color) -> float:
    c.setFillColor(color)
    c.setFont(font, size)
    for line in wrap(text, font, size, width):
        c.drawString(x, y, line)
        y -= leading
    return y


def chip_lines(text: str, avail: float, font: str, size: float) -> list[str]:
    lines, rest = [], text
    while stringWidth(rest, font, size) > avail:
        cut = len(rest)
        while cut > 1 and stringWidth(rest[:cut], font, size) > avail:
            cut -= 1
        brk = max(rest.rfind("/", 1, cut), rest.rfind(" ", 1, cut))
        if brk > 0:
            cut = brk + 1
        lines.append(rest[:cut])
        rest = rest[cut:]
    lines.append(rest)
    return lines


def mono_chip(c: Canvas, x: float, y: float, text: str, width: float) -> float:
    size = 8.2
    pad = 5.0
    lead = 10.2
    font = "Courier-Bold"
    lines = chip_lines(text, width - 2 * pad, font, size)
    tw = min(max(stringWidth(line, font, size) for line in lines) + 2 * pad, width)
    bottom = y - 3.5 - (len(lines) - 1) * lead
    c.setFillColor(PANEL)
    c.setStrokeColor(RULE)
    c.setLineWidth(0.5)
    c.roundRect(x, bottom, tw, 14.5 + (len(lines) - 1) * lead, 3, stroke=1, fill=1)
    c.setFillColor(INK)
    c.setFont(font, size)
    ty = y + 0.8
    for line in lines:
        c.drawString(x + pad, ty, line)
        ty -= lead
    return bottom


def circled_number(c: Canvas, cx: float, cy: float, n: int) -> None:
    c.setFillColor(ACCENT)
    c.circle(cx, cy, 11.5, stroke=0, fill=1)
    c.setFillColor(HexColor("#FFFFFF"))
    c.setFont(BOLD, 12)
    c.drawCentredString(cx, cy - 4.2, str(n))


def file_glyph(c: Canvas, x: float, y: float, w: float, h: float, label: str,
               border: Color, fill: Color, badge: str | None = None,
               badge_color: Color = ACCENT) -> None:
    fold = 8.0
    p = c.beginPath()
    p.moveTo(x, y)
    p.lineTo(x, y + h)
    p.lineTo(x + w - fold, y + h)
    p.lineTo(x + w, y + h - fold)
    p.lineTo(x + w, y)
    p.close()
    c.setFillColor(fill)
    c.setStrokeColor(border)
    c.setLineWidth(0.8)
    c.drawPath(p, stroke=1, fill=1)
    c.setStrokeColor(border)
    c.line(x + w - fold, y + h, x + w - fold, y + h - fold)
    c.line(x + w - fold, y + h - fold, x + w, y + h - fold)
    c.setFillColor(MUTED)
    c.setFont(REG, 6.4)
    c.drawCentredString(x + w / 2, y - 8.5, label)
    if badge:
        c.setFillColor(badge_color)
        c.roundRect(x + 4, y + 4, w - 8, 8.5, 2, stroke=0, fill=1)
        c.setFillColor(HexColor("#FFFFFF"))
        c.setFont(BOLD, 5.4)
        c.drawCentredString(x + w / 2, y + 6.4, badge)


def waveform(c: Canvas, x: float, y: float, w: float, h: float, color: Color) -> None:
    import math
    c.setStrokeColor(color)
    c.setLineWidth(0.9)
    n = 26
    for i in range(n):
        t = i / (n - 1)
        a = (h / 2) * (0.30 + 0.70 * abs(math.sin(t * 6.1)) * (0.45 + 0.55 * math.sin(t * 2.6)))
        px = x + t * w
        c.line(px, y + h / 2 - a, px, y + h / 2 + a)


def arrow(c: Canvas, x1: float, y: float, x2: float, color: Color) -> None:
    c.setStrokeColor(color)
    c.setFillColor(color)
    c.setLineWidth(0.9)
    c.line(x1, y, x2 - 4, y)
    p = c.beginPath()
    p.moveTo(x2, y)
    p.lineTo(x2 - 4.6, y + 2.8)
    p.lineTo(x2 - 4.6, y - 2.8)
    p.close()
    c.drawPath(p, stroke=0, fill=1)


def key_glyph(c: Canvas, x: float, y: float, color: Color) -> None:
    c.setStrokeColor(color)
    c.setFillColor(color)
    c.setLineWidth(1.6)
    c.circle(x + 6, y, 5.4, stroke=1, fill=0)
    c.line(x + 11.4, y, x + 30, y)
    c.line(x + 24, y, x + 24, y - 5)
    c.line(x + 29, y, x + 29, y - 4)


def shield(c: Canvas, cx: float, cy: float, s: float, color: Color, fill: Color, tick: bool) -> None:
    p = c.beginPath()
    p.moveTo(cx, cy + s)
    p.lineTo(cx + s * 0.78, cy + s * 0.42)
    p.lineTo(cx + s * 0.78, cy - s * 0.30)
    p.curveTo(cx + s * 0.78, cy - s * 0.78, cx + s * 0.36, cy - s * 0.94, cx, cy - s * 1.05)
    p.curveTo(cx - s * 0.36, cy - s * 0.94, cx - s * 0.78, cy - s * 0.78, cx - s * 0.78, cy - s * 0.30)
    p.lineTo(cx - s * 0.78, cy + s * 0.42)
    p.close()
    c.setFillColor(fill)
    c.setStrokeColor(color)
    c.setLineWidth(1.1)
    c.drawPath(p, stroke=1, fill=1)
    if tick:
        c.setStrokeColor(color)
        c.setLineWidth(1.8)
        c.setLineCap(1)
        c.line(cx - s * 0.34, cy + s * 0.04, cx - s * 0.06, cy - s * 0.26)
        c.line(cx - s * 0.06, cy - s * 0.26, cx + s * 0.40, cy + s * 0.44)
        c.setLineCap(0)


def diagram_authorize(c: Canvas, x: float, y: float, w: float, h: float) -> None:
    cy = y + h / 2
    key_glyph(c, x + 4, cy + 2, ACCENT)
    arrow(c, x + 40, cy + 2, x + 62, FAINT)
    shield(c, x + 84, cy + 1, 15, ACCENT, ACCENT_SOFT, True)
    c.setFillColor(MUTED)
    c.setFont(REG, 6.4)
    c.drawCentredString(x + 20, cy - 15, "demo key")
    c.drawCentredString(x + 84, cy - 20, "trust anchor")
    c.setStrokeColor(RULE)
    c.setLineWidth(0.6)
    c.setDash(2, 2)
    c.line(x + 108, cy + 1, x + w - 2, cy + 1)
    c.setDash()
    c.setFillColor(FAINT)
    c.setFont(OBL, 6.4)
    c.drawString(x + 112, cy - 2.4, "self-issued")


def diagram_sign(c: Canvas, x: float, y: float, w: float, h: float) -> None:
    cy = y + h / 2
    c.setStrokeColor(RULE)
    c.setFillColor(HexColor("#FFFFFF"))
    c.setLineWidth(0.8)
    c.roundRect(x + 2, cy - 12, 52, 24, 3, stroke=1, fill=1)
    waveform(c, x + 8, cy - 8, 40, 16, WAVE)
    c.setFillColor(MUTED)
    c.setFont(REG, 6.4)
    c.drawCentredString(x + 28, cy - 21, "master out")
    arrow(c, x + 58, cy, x + 76, FAINT)
    c.setFillColor(ACCENT_SOFT)
    c.setStrokeColor(ACCENT)
    c.roundRect(x + 78, cy - 10, 34, 20, 3, stroke=1, fill=1)
    c.setFillColor(ACCENT)
    c.setFont(BOLD, 6.2)
    c.drawCentredString(x + 95, cy - 2.2, "CAPTURE")
    arrow(c, x + 116, cy, x + 134, FAINT)
    file_glyph(c, x + 136, cy - 14, 26, 28, "signed WAV", ACCENT, HexColor("#FFFFFF"), "C2PA")


def diagram_verify(c: Canvas, x: float, y: float, w: float, h: float) -> None:
    cy = y + h / 2
    file_glyph(c, x + 4, cy - 14, 24, 28, "export", RULE, HexColor("#FFFFFF"))
    arrow(c, x + 32, cy, x + 50, FAINT)
    # IMPORTANT: this legend mirrors PluginEditor.cpp's outcome colours. Nothing found
    # is neutral, never red: an absence of evidence is not a failed or suspect file.
    rows = [
        (ACCENT, "verified", True),
        (RED, "registered, but changed", False),
        (AMBER, "mark found, claim not trusted", False),
        (ACCENT, "nothing found", False),
    ]
    ry = cy + 17
    for color, label, emphasis in rows:
        c.setFillColor(color)
        c.circle(x + 56, ry + 2.2, 2.6, stroke=0, fill=1)
        c.setFillColor(INK if emphasis else MUTED)
        c.setFont(BOLD if emphasis else REG, 6.4)
        c.drawString(x + 63, ry, label)
        ry -= 11.5


VOLUME_DOCS = "Documentation and Demo Project"

STEPS = [
    {
        "n": 1,
        "title": "Install, then start the recorder",
        "body": "Run 'Install Plug-Ins.command': it creates both plug-in folders and copies the VST3 and "
                "the Audio Unit in. Then run 'Start Capture Daemon.command' and leave it open. The "
                "plug-in streams to that recorder; with nothing listening, nothing is recorded.",
        "chips": ["Install Plug-Ins.command", "Start Capture Daemon.command"],
        "diagram": diagram_authorize,
    },
    {
        "n": 2,
        "title": "Capture and sign an export",
        "body": "The demo set needs Live 12.0 or newer. Insert the plug-in, play, then deactivate the "
                "device before rendering: an offline render outruns the realtime hasher. Render into the "
                "exports folder the recorder opened. Export 16-bit PCM WAV; 32-bit float is refused.",
        "chips": ["Documents/Audio Provenance Capture/sessions/<id>/exports"],
        "diagram": diagram_sign,
    },
    {
        "n": 3,
        "title": "Verify a file",
        "body": "Each sealed export gets a verifier result linked from the dashboard, reporting only what "
                "was observed, hashed or declared, with its own proof level. The bundled root is the root "
                "of the store the recorder signs with, so any C2PA verifier can recheck the binding on the "
                "demo clips and on everything you sign. Only a changed file is red: nothing found is "
                "neutral, never evidence of synthetic origin.",
        "chips": ["manifests/artifacts/<export>_verification.json",
                  f"c2patool EXPORT.wav trust --trust_anchors '{VOLUME_DOCS}/demo-root-ca.pem'"],
        "diagram": diagram_verify,
    },
]


def render(path: Path) -> None:
    c = Canvas(str(path), pagesize=LETTER, invariant=1)
    c.setTitle("Audio Provenance Wrapper Quickstart")
    c.setAuthor("Audio Provenance Wrapper")
    c.setSubject("Sign and verify an export")

    c.setFillColor(ACCENT)
    c.rect(0, H - 6, W, 6, stroke=0, fill=1)

    y = H - MARGIN - 14
    c.setFillColor(ACCENT)
    tracked(c, MARGIN, y, "QUICKSTART", BOLD, 8, 1.9)

    y -= 30
    c.setFillColor(INK)
    c.setFont(BOLD, 27)
    c.drawString(MARGIN, y, "Sign and verify an export")

    y -= 20
    inner = W - 2 * MARGIN
    y = paragraph(
        c, MARGIN, y,
        "Audio Provenance Wrapper records what it actually observed on the way out of the session, "
        "binds it to the exported bytes, and hands anyone a way to check that binding.",
        REG, 10.5, inner * 0.78, 14, MUTED,
    )

    y -= 8
    c.setStrokeColor(RULE)
    c.setLineWidth(0.8)
    c.line(MARGIN, y, W - MARGIN, y)

    text_w = 306.0
    diag_x = MARGIN + 340
    diag_w = W - MARGIN - diag_x

    box_h = 66.0
    box_y = MARGIN + 34
    gap = 14.0
    floor = box_y + box_h + 10

    top = y - 28
    for i, step in enumerate(STEPS):
        circled_number(c, MARGIN + 11.5, top - 2, step["n"])
        tx = MARGIN + 34
        c.setFillColor(INK)
        c.setFont(BOLD, 14.5)
        c.drawString(tx, top - 8, step["title"])
        by = paragraph(c, tx, top - 27, step["body"], REG, 9.8, text_w, 13.6, MUTED)
        by -= 4
        for chip in step["chips"]:
            by = mono_chip(c, tx, by, chip, text_w) - 9
        step["diagram"](c, diag_x, top - 72, diag_w, 72)
        bottom = min(by, top - 82)
        if i < len(STEPS) - 1:
            c.setStrokeColor(RULE)
            c.setLineWidth(0.5)
            c.line(MARGIN + 34, bottom - gap / 2, W - MARGIN, bottom - gap / 2)
        top = bottom - gap

    # REQUIRED: a step that runs into the callout ships an unreadable quickstart in the DMG.
    if top < floor:
        raise ValueError(f"quickstart steps overrun the callout by {floor - top:.1f}pt")

    c.setFillColor(PANEL)
    c.setStrokeColor(RULE)
    c.setLineWidth(0.8)
    c.roundRect(MARGIN, box_y, inner, box_h, 4, stroke=1, fill=1)
    c.setFillColor(AMBER)
    c.rect(MARGIN, box_y, 3.2, box_h, stroke=0, fill=1)
    c.setFillColor(AMBER)
    tracked(c, MARGIN + 16, box_y + box_h - 19, "READ THIS BEFORE YOU SHOW IT TO ANYONE", BOLD, 7.4, 1.1)
    paragraph(
        c, MARGIN + 16, box_y + box_h - 34,
        "The bundled key is a self-issued demo identity. It is not a verified identity, it attests to no "
        "person or organisation, and a trusted result here means only that the bytes match a signature made "
        "by this demo key. Nothing in this bundle establishes who made the audio.",
        REG, 8.6, inner - 34, 11.2, INK,
    )

    c.setFillColor(FAINT)
    c.setFont(REG, 7.2)
    c.drawString(MARGIN, MARGIN - 6, "Audio Provenance Wrapper — proof of concept")
    c.drawRightString(W - MARGIN, MARGIN - 6, "Every field carries its own proof level")

    c.showPage()
    c.save()


def main() -> None:
    default = Path(__file__).resolve().parent.parent / "packaging" / "assets" / "quickstart.pdf"
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--out", type=Path, default=default)
    args = ap.parse_args()
    args.out.parent.mkdir(parents=True, exist_ok=True)
    render(args.out)
    print(f"{args.out} ({args.out.stat().st_size} bytes)")


if __name__ == "__main__":
    main()

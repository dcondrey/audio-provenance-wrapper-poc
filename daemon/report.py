from __future__ import annotations

import html
import json
from pathlib import Path


PROOF_LABELS = {
    "directly_observed": "Directly observed",
    "inferred": "Inferred",
    "user_declared": "User declared",
    "externally_verified": "Externally verified",
    "unknown_unobserved": "Unknown / unobserved",
}


def _escape(value: object) -> str:
    return html.escape(str(value), quote=True)


def _proof_badge(proof_level: object) -> str:
    proof = str(proof_level or "unknown_unobserved")
    label = PROOF_LABELS.get(proof, proof.replace("_", " ").title())
    return f'<span class="proof proof-{_escape(proof)}">{_escape(label)}</span>'


def _short_hash(value: object) -> str:
    digest = str(value or "not available")
    if len(digest) <= 24:
        return digest
    return f"{digest[:16]}…{digest[-8:]}"


def render_html_report(manifest: dict[str, object]) -> str:
    """Render a dependency-free human-readable fight card from a manifest."""
    export = manifest.get("export") if isinstance(manifest.get("export"), dict) else {}
    stems = manifest.get("observed_stems") if isinstance(manifest.get("observed_stems"), list) else []
    claims = manifest.get("claim_summary") if isinstance(manifest.get("claim_summary"), list) else []
    unobserved = manifest.get("apw:unobserved") if isinstance(manifest.get("apw:unobserved"), list) else []
    association = (
        manifest.get("stem_export_association")
        if isinstance(manifest.get("stem_export_association"), dict)
        else {}
    )

    claim_cards: list[str] = []
    for raw_claim in claims:
        if not isinstance(raw_claim, dict):
            continue
        value = raw_claim.get("value")
        display_value = "Yes" if value is True else "No" if value is False else value
        claim_cards.append(
            """
            <article class="claim-card">
              <div class="claim-heading">
                <span>{claim}</span>
                {badge}
              </div>
              <strong class="claim-value">{value}</strong>
              <p>{evidence}</p>
            </article>
            """.format(
                claim=_escape(str(raw_claim.get("claim", "claim")).replace("_", " ").title()),
                badge=_proof_badge(raw_claim.get("apw:proof_level")),
                value=_escape(display_value),
                evidence=_escape(raw_claim.get("evidence", "")),
            )
        )

    stem_rows: list[str] = []
    for raw_stem in stems:
        if not isinstance(raw_stem, dict):
            continue
        stem_rows.append(
            """
            <tr>
              <td>{stem_id}</td>
              <td><code title="{full_hash}">{chain_root}</code></td>
              <td>{windows}</td>
              <td>{sample_rate} Hz / {channels} ch</td>
              <td>{badge}</td>
            </tr>
            """.format(
                stem_id=_escape(raw_stem.get("stem_id", "unknown")),
                full_hash=_escape(raw_stem.get("hash_chain_root", "")),
                chain_root=_escape(_short_hash(raw_stem.get("hash_chain_root"))),
                windows=_escape(raw_stem.get("hash_chain_length", 0)),
                sample_rate=_escape(raw_stem.get("sample_rate_hz", "unknown")),
                channels=_escape(raw_stem.get("channel_count", "unknown")),
                badge=_proof_badge(raw_stem.get("apw:proof_level")),
            )
        )

    unobserved_items = "".join(
        f"<li>{_escape(str(item).replace('_', ' '))}</li>" for item in unobserved
    )
    raw_manifest = html.escape(json.dumps(manifest, indent=2, ensure_ascii=False))

    return """<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Audio Provenance Fight Card</title>
  <style>
    :root {{ color-scheme: dark; --bg: #0b0f14; --panel: #141b23; --line: #263242;
      --text: #edf4fb; --muted: #93a4b8; --cyan: #4de3ff; --green: #4fe3a3;
      --amber: #ffc45c; --violet: #b89cff; --gray: #a5b0bf; }}
    * {{ box-sizing: border-box; }}
    body {{ margin: 0; background: radial-gradient(circle at 85% 0%, #122b35 0, var(--bg) 38%);
      color: var(--text); font: 15px/1.5 -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; }}
    main {{ width: min(1120px, calc(100% - 40px)); margin: 0 auto; padding: 56px 0 80px; }}
    header {{ display: flex; justify-content: space-between; align-items: end; gap: 24px;
      border-bottom: 1px solid var(--line); padding-bottom: 28px; }}
    .eyebrow {{ color: var(--cyan); text-transform: uppercase; letter-spacing: .16em; font-weight: 700; font-size: 12px; }}
    h1 {{ margin: 8px 0 6px; font-size: clamp(34px, 6vw, 64px); line-height: .98; letter-spacing: -.045em; }}
    .subtitle, .meta, p {{ color: var(--muted); }}
    .meta {{ text-align: right; white-space: nowrap; }}
    section {{ margin-top: 36px; }}
    h2 {{ font-size: 18px; letter-spacing: -.01em; margin: 0 0 14px; }}
    .hero-grid {{ display: grid; grid-template-columns: 1fr 1fr; gap: 16px; }}
    .hero-card, .claim-card, .panel {{ background: color-mix(in srgb, var(--panel) 92%, transparent);
      border: 1px solid var(--line); border-radius: 16px; box-shadow: 0 18px 50px rgba(0,0,0,.18); }}
    .hero-card {{ padding: 24px; }}
    .hero-card span {{ display: block; color: var(--muted); font-size: 12px; text-transform: uppercase; letter-spacing: .1em; }}
    .hero-card strong {{ display: block; margin-top: 8px; font-size: 22px; }}
    code {{ color: var(--cyan); font-family: ui-monospace, SFMono-Regular, Menlo, monospace; }}
    .claims {{ display: grid; grid-template-columns: repeat(auto-fit, minmax(220px, 1fr)); gap: 12px; }}
    .claim-card {{ padding: 18px; min-height: 160px; }}
    .claim-heading {{ display: flex; flex-direction: column; align-items: flex-start; gap: 10px; }}
    .claim-heading > span:first-child {{ color: var(--muted); font-size: 12px; text-transform: uppercase; letter-spacing: .08em; }}
    .claim-value {{ display: block; font-size: 24px; margin-top: 16px; overflow-wrap: anywhere; }}
    .claim-card p {{ margin: 8px 0 0; font-size: 13px; }}
    .proof {{ display: inline-flex; align-items: center; border-radius: 999px; padding: 4px 9px;
      font-size: 11px; font-weight: 700; letter-spacing: .02em; background: #202a36; color: var(--gray); }}
    .proof-directly_observed {{ color: var(--green); background: rgba(79,227,163,.1); }}
    .proof-inferred {{ color: var(--amber); background: rgba(255,196,92,.1); }}
    .proof-user_declared {{ color: var(--violet); background: rgba(184,156,255,.1); }}
    .panel {{ overflow: hidden; }}
    table {{ width: 100%; border-collapse: collapse; }}
    th, td {{ text-align: left; padding: 15px 18px; border-bottom: 1px solid var(--line); }}
    th {{ color: var(--muted); text-transform: uppercase; letter-spacing: .08em; font-size: 11px; }}
    tr:last-child td {{ border-bottom: 0; }}
    .association {{ padding: 20px 22px; border-left: 3px solid var(--amber); }}
    .association p {{ margin: 6px 0 0; }}
    .unknowns {{ columns: 2; margin: 0; padding: 20px 40px; color: var(--muted); }}
    details {{ margin-top: 28px; }}
    summary {{ cursor: pointer; color: var(--muted); }}
    pre {{ overflow: auto; max-height: 520px; padding: 20px; background: #070a0e; border: 1px solid var(--line);
      border-radius: 12px; color: #b8c7d9; font: 12px/1.55 ui-monospace, SFMono-Regular, Menlo, monospace; }}
    footer {{ margin-top: 42px; padding-top: 20px; border-top: 1px solid var(--line); color: var(--muted); font-size: 13px; }}
    @media (max-width: 720px) {{ header {{ display: block; }} .meta {{ text-align: left; margin-top: 16px; }}
      .hero-grid {{ grid-template-columns: 1fr; }} .unknowns {{ columns: 1; }} table {{ font-size: 12px; }} }}
  </style>
</head>
<body>
<main>
  <header>
    <div>
      <div class="eyebrow">Audio Provenance Capture</div>
      <h1>Evidence fight card</h1>
      <div class="subtitle">A truthful summary of what this capture session observed—and what it could not.</div>
    </div>
    <div class="meta">Session<br><code>{session_id}</code><br>{created_at}</div>
  </header>

  <section class="hero-grid">
    <div class="hero-card"><span>Export</span><strong>{export_name}</strong><code title="{export_hash_full}">{export_hash}</code></div>
    <div class="hero-card"><span>Observed stems</span><strong>{stem_count}</strong><span>{window_count} routed-audio hash windows</span></div>
  </section>

  <section>
    <h2>Claim summary</h2>
    <div class="claims">{claim_cards}</div>
  </section>

  <section>
    <h2>Observed routed audio</h2>
    <div class="panel"><table>
      <thead><tr><th>Stem</th><th>Chain commitment</th><th>Windows</th><th>Format</th><th>Proof</th></tr></thead>
      <tbody>{stem_rows}</tbody>
    </table></div>
  </section>

  <section>
    <h2>Stem-to-export association</h2>
    <div class="panel association">{association_badge}<strong> {association_status}</strong>
      <p>{association_basis}</p></div>
  </section>

  <section>
    <h2>Explicitly not claimed</h2>
    <div class="panel"><ul class="unknowns">{unobserved_items}</ul></div>
  </section>

  <details>
    <summary>Inspect complete JSON evidence manifest</summary>
    <pre>{raw_manifest}</pre>
  </details>

  <footer>Never claim full DAW provenance. This report covers only routed, observed, hashed, declared, inferred, verified, or explicitly missed evidence.</footer>
</main>
</body>
</html>
""".format(
        session_id=_escape(manifest.get("session_id", "unknown")),
        created_at=_escape(manifest.get("created_at", "")),
        export_name=_escape(export.get("file_name", "No export detected")),
        export_hash_full=_escape(export.get("sha256", "")),
        export_hash=_escape(_short_hash(export.get("sha256"))),
        stem_count=len(stems),
        window_count=sum(
            int(stem.get("hash_chain_length", 0))
            for stem in stems
            if isinstance(stem, dict)
        ),
        claim_cards="".join(claim_cards),
        stem_rows="".join(stem_rows) or '<tr><td colspan="5">No routed-audio evidence was received.</td></tr>',
        association_badge=_proof_badge(association.get("apw:proof_level")),
        association_status=_escape(str(association.get("status", "not established")).replace("_", " ").title()),
        association_basis=_escape(association.get("basis", "")),
        unobserved_items=unobserved_items,
        raw_manifest=raw_manifest,
    )


def write_html_report(manifest: dict[str, object], path: Path) -> Path:
    path = path.expanduser()
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(render_html_report(manifest), encoding="utf-8")
    return path

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
    coverage = (
        manifest.get("observation_coverage")
        if isinstance(manifest.get("observation_coverage"), dict)
        else {}
    )
    verification = (
        manifest.get("local_verification_summary")
        if isinstance(manifest.get("local_verification_summary"), dict)
        else {}
    )
    handoff = (
        manifest.get("downstream_registration_handoff")
        if isinstance(manifest.get("downstream_registration_handoff"), dict)
        else {}
    )
    capture_session = (
        manifest.get("capture_session")
        if isinstance(manifest.get("capture_session"), dict)
        else {}
    )
    presentation = (
        manifest.get("presentation")
        if isinstance(manifest.get("presentation"), dict)
        else {}
    )
    plugin_ids = sorted({
        str(plugin_id)
        for stem in stems if isinstance(stem, dict)
        for plugin_id in stem.get("plugin_instance_ids", [])
    })
    alignment_values = association.get("alignment_similarity", [])
    alignment_bars = "".join(
        f'<i style="height:{max(3, min(100, float(value) * 100)):.1f}%" title="{float(value):.3f}"></i>'
        for value in alignment_values
        if isinstance(value, (int, float))
    )
    missing_requirements = handoff.get("missing_downstream_requirements", [])
    handoff_items = "".join(
        f"<li>{_escape(item)}</li>" for item in missing_requirements
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
    .timeline {{ display: grid; grid-template-columns: repeat(6, 1fr); gap: 8px; }}
    .step {{ min-height: 92px; padding: 14px; border: 1px solid var(--line); border-radius: 12px; background: var(--panel); }}
    .step b {{ display: block; color: var(--green); font-size: 11px; text-transform: uppercase; letter-spacing: .08em; }}
    .step span {{ display: block; margin-top: 8px; color: var(--muted); font-size: 12px; }}
    .alignment-chart {{ height: 150px; padding: 20px; display: flex; align-items: end; gap: 3px; }}
    .alignment-chart i {{ display: block; flex: 1; min-width: 2px; background: linear-gradient(var(--cyan), var(--green)); border-radius: 2px 2px 0 0; }}
    .metrics {{ display: grid; grid-template-columns: repeat(4, 1fr); border-top: 1px solid var(--line); }}
    .metric {{ padding: 16px 20px; border-right: 1px solid var(--line); }}
    .metric:last-child {{ border-right: 0; }}
    .metric span {{ display: block; color: var(--muted); font-size: 11px; text-transform: uppercase; letter-spacing: .08em; }}
    .metric strong {{ display: block; margin-top: 6px; font-size: 18px; }}
    .handoff {{ display: grid; grid-template-columns: .8fr 1.2fr; gap: 0; }}
    .handoff > div {{ padding: 22px; }}
    .handoff > div + div {{ border-left: 1px solid var(--line); }}
    .legend {{ display: flex; flex-wrap: wrap; gap: 8px; }}
    .unknowns {{ columns: 2; margin: 0; padding: 20px 40px; color: var(--muted); }}
    details {{ margin-top: 28px; }}
    summary {{ cursor: pointer; color: var(--muted); }}
    pre {{ overflow: auto; max-height: 520px; padding: 20px; background: #070a0e; border: 1px solid var(--line);
      border-radius: 12px; color: #b8c7d9; font: 12px/1.55 ui-monospace, SFMono-Regular, Menlo, monospace; }}
    footer {{ margin-top: 42px; padding-top: 20px; border-top: 1px solid var(--line); color: var(--muted); font-size: 13px; }}
    @media (max-width: 720px) {{ header {{ display: block; }} .meta {{ text-align: left; margin-top: 16px; }}
      .hero-grid {{ grid-template-columns: 1fr; }} .unknowns {{ columns: 1; }} table {{ font-size: 12px; }}
      .timeline {{ grid-template-columns: 1fr 1fr; }} .metrics {{ grid-template-columns: 1fr 1fr; }}
      .handoff {{ grid-template-columns: 1fr; }} .handoff > div + div {{ border-left: 0; border-top: 1px solid var(--line); }} }}
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
    <div class="hero-card"><span>Observed path coverage</span><strong>{coverage_status}</strong><span>{window_count} routed-audio hash windows received</span></div>
  </section>

  <section>
    <h2>Session pipeline</h2>
    <div class="timeline">
      <div class="step"><b>01 Observed</b><span>Plug-in routed audio<br>{plugin_ids}</span></div>
      <div class="step"><b>02 Received</b><span>Daemon accepted<br>{received_count} events</span></div>
      <div class="step"><b>03 Checked</b><span>Gaps {sequence_gaps}<br>Breaks {chain_breaks}</span></div>
      <div class="step"><b>04 Export</b><span>{export_name}</span></div>
      <div class="step"><b>05 Associated</b><span>{association_status}</span></div>
      <div class="step"><b>06 Verified</b><span>{verification_outcome}<br>Local POC result</span></div>
    </div>
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
    <h2>Routed / export alignment</h2>
    <div class="panel">
      <div class="alignment-chart">{alignment_bars}</div>
      <div class="metrics">
        <div class="metric"><span>Method</span><strong>{association_method}</strong></div>
        <div class="metric"><span>Confidence</span><strong>{association_confidence}</strong></div>
        <div class="metric"><span>Matched coverage</span><strong>{matched_coverage}</strong></div>
        <div class="metric"><span>Best offset</span><strong>{best_offset}</strong></div>
      </div>
    </div>
    <p>A high score remains inferred. An unavailable or failed match does not prove routed audio was absent.</p>
  </section>

  <section>
    <h2>Verification status</h2>
    <div class="panel association">{verification_badge}<strong> {verification_outcome}</strong>
      <p>Local POC integrity result only. Signer identity, authorship, ownership, consent, and registry status are not established.</p></div>
  </section>

  <section>
    <h2>Downstream registration handoff</h2>
    <div class="panel handoff">
      <div><span class="eyebrow">Prepared input</span><h2>{handoff_status}</h2>
        <p>Export hard hash, routed observation commitment, coverage, inferred association, declarations, signing key, and evidence bindings.</p>
        <a href="{handoff_href}">Open handoff record ↗</a></div>
      <div><strong>Still required downstream</strong><ul>{handoff_items}</ul>
        <p>This is a neutral provenance handoff, not a provider-specific API payload.</p></div>
    </div>
  </section>

  <section>
    <h2>Explicitly not claimed</h2>
    <div class="panel"><ul class="unknowns">{unobserved_items}</ul></div>
  </section>

  <section>
    <h2>Proof-level legend</h2>
    <div class="legend">{proof_legend}</div>
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
        coverage_status=_escape(str(coverage.get("status", "unknown_coverage")).replace("_", " ").title()),
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
        association_method=_escape(association.get("method", "unavailable")),
        association_confidence=_escape(
            f"{float(association.get('confidence')) * 100:.1f}%"
            if isinstance(association.get("confidence"), (int, float)) else "Unavailable"
        ),
        matched_coverage=_escape(
            f"{float(association.get('matched_coverage', 0)) * 100:.1f}%"
        ),
        best_offset=_escape(
            f"{association.get('best_offset_seconds')} s"
            if association.get("best_offset_seconds") is not None else "Unavailable"
        ),
        alignment_bars=alignment_bars or '<span style="color:var(--muted)">Alignment unavailable</span>',
        plugin_ids=_escape(", ".join(plugin_ids) if plugin_ids else "No instance observed"),
        received_count=_escape((coverage.get("counters") or {}).get("events_received", 0)),
        sequence_gaps=_escape((coverage.get("counters") or {}).get("sequence_gaps", 0)),
        chain_breaks=_escape((coverage.get("counters") or {}).get("hash_chain_breaks", 0)),
        verification_outcome=_escape(str(verification.get("outcome", "untrusted")).replace("_", " ").title()),
        verification_badge=_proof_badge(
            "directly_observed" if verification.get("outcome") == "verified" else "unknown_unobserved"
        ),
        handoff_status=_escape(str(handoff.get("status", "not prepared")).replace("_", " ").title()),
        handoff_items=handoff_items,
        handoff_href=_escape(presentation.get("downstream_handoff", "#")),
        proof_legend="".join(_proof_badge(proof) for proof in PROOF_LABELS),
        unobserved_items=unobserved_items,
        raw_manifest=raw_manifest,
    )


def write_html_report(manifest: dict[str, object], path: Path) -> Path:
    path = path.expanduser()
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(render_html_report(manifest), encoding="utf-8")
    return path

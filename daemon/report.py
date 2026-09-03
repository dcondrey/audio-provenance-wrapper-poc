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


# Keyed by upper case: str.title() splits on digits, so 'c2pa' becomes 'C2Pa'
# rather than 'C2pa', and a literal-case map silently misses it.
_ACRONYMS = {
    "C2PA": "C2PA", "UDP": "UDP", "MIDI": "MIDI", "DAW": "DAW", "ACK": "ACK",
    "HMAC": "HMAC", "SHA256": "SHA-256", "JSON": "JSON", "WAV": "WAV",
    "AIFF": "AIFF", "RMS": "RMS", "POC": "POC", "ID": "ID", "PCM": "PCM",
}


def _humanize(value: object, fallback: str = "") -> str:
    """Title-case an identifier without mangling the acronyms in it.

    str.title() renders 'embedded_c2pa_claim' as 'Embedded C2Pa Claim', which is
    the vendor standard misspelt in the fight card's headline summary.
    """
    words = str(value or fallback).replace("_", " ").title().split()
    return " ".join(_ACRONYMS.get(word.upper(), word) for word in words)


def _escape(value: object) -> str:
    return html.escape(str(value), quote=True)


def _percentage(value: object) -> str:
    """Unmeasured is not zero: an unavailable comparison has no coverage number."""
    if isinstance(value, (int, float)) and not isinstance(value, bool):
        return f"{float(value) * 100:.1f}%"
    return "Unavailable"


def _proof_badge(proof_level: object) -> str:
    proof = str(proof_level or "unknown_unobserved")
    label = PROOF_LABELS.get(proof, _humanize(proof))
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
    c2pa_claim = (
        manifest.get("c2pa_claim")
        if isinstance(manifest.get("c2pa_claim"), dict)
        else {}
    )
    c2pa_validation = (
        c2pa_claim.get("validation")
        if isinstance(c2pa_claim.get("validation"), dict)
        else {}
    )
    c2pa_binding = (
        c2pa_claim.get("hard_binding")
        if isinstance(c2pa_claim.get("hard_binding"), dict)
        else {}
    )
    c2pa_signed = c2pa_claim.get("status") in {"embedded", "sidecar"}
    c2pa_ingredients = (
        c2pa_claim.get("ingredients")
        if isinstance(c2pa_claim.get("ingredients"), list)
        else []
    )
    c2pa_unresolved_refs = (
        c2pa_claim.get("unresolved_ingredient_references")
        if isinstance(c2pa_claim.get("unresolved_ingredient_references"), list)
        else []
    )
    c2pa_ingredient_rows = "".join(
        """
            <tr>
              <td>{title}</td>
              <td>{relationship}</td>
              <td><code title="{full_hash}">{digest}</code></td>
              <td>{meaning}</td>
              <td>{badge}</td>
            </tr>
            """.format(
            title=_escape(node.get("title", "unknown")),
            relationship=_escape(node.get("relationship", "unknown")),
            full_hash=_escape(node.get("sha256", "")),
            digest=_escape(_short_hash(node.get("sha256"))),
            meaning=_escape(node.get("digest_meaning", "")),
            badge=_proof_badge(node.get("apw:proof_level")),
        )
        for node in c2pa_ingredients
        if isinstance(node, dict)
    )
    unresolved_refs = "".join(
        f"<li>{_escape(item.get('reference', ''))} — {_escape(item.get('reason', ''))}</li>"
        for item in c2pa_unresolved_refs
        if isinstance(item, dict)
    )

    handoff = (
        manifest.get("downstream_registration_handoff")
        if isinstance(manifest.get("downstream_registration_handoff"), dict)
        else {}
    )
    presentation = (
        manifest.get("presentation")
        if isinstance(manifest.get("presentation"), dict)
        else {}
    )
    receipt = (
        manifest.get("daemon_receipt_acknowledgement")
        if isinstance(manifest.get("daemon_receipt_acknowledgement"), dict)
        else {}
    )
    receipt_streams = receipt.get("streams") if isinstance(receipt.get("streams"), list) else []
    first_receipt_stream = receipt_streams[0] if receipt_streams and isinstance(receipt_streams[0], dict) else {}
    plugin_ids = sorted({
        str(plugin_id)
        for stem in stems if isinstance(stem, dict)
        for plugin_id in stem.get("plugin_instance_ids", [])
    })
    alignment_series = association.get("alignment_series", [])
    if not isinstance(alignment_series, list) or not alignment_series:
        alignment_series = [
            {"similarity": value, "matched": float(value) >= 0.72}
            for value in association.get("alignment_similarity", [])
            if isinstance(value, (int, float))
        ]
    alignment_bars = "".join(
        '<i class="{state}" style="height:{height:.1f}%" title="Window similarity {score:.3f}"></i>'.format(
            state="matched" if point.get("matched") else "weak",
            height=max(3, min(100, float(point.get("similarity", 0)) * 100)),
            score=float(point.get("similarity", 0)),
        )
        for point in alignment_series
        if isinstance(point, dict) and isinstance(point.get("similarity"), (int, float))
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
                claim=_escape(_humanize(raw_claim.get("claim"), "claim")),
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
    .timeline {{ display: grid; grid-template-columns: repeat(7, 1fr); gap: 8px; }}
    .step {{ min-height: 92px; padding: 14px; border: 1px solid var(--line); border-radius: 12px; background: var(--panel); }}
    .step b {{ display: block; color: var(--green); font-size: 11px; text-transform: uppercase; letter-spacing: .08em; }}
    .step span {{ display: block; margin-top: 8px; color: var(--muted); font-size: 12px; }}
    .alignment-chart {{ height: 150px; padding: 20px; display: flex; align-items: end; gap: 3px; }}
    .alignment-chart i {{ display: block; flex: 1; min-width: 2px; background: var(--amber); border-radius: 2px 2px 0 0; }}
    .alignment-chart i.matched {{ background: linear-gradient(var(--cyan), var(--green)); }}
    .chart-key {{ padding: 0 20px 14px; color: var(--muted); font-size: 12px; }}
    .chart-key b {{ color: var(--green); }} .chart-key em {{ color: var(--amber); font-style: normal; }}
    .metrics {{ display: grid; grid-template-columns: repeat(6, 1fr); border-top: 1px solid var(--line); }}
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
      <div class="step"><b>02 Emitted</b><span>Local UDP writes<br>{emitted_count} attempted</span></div>
      <div class="step"><b>03 Received</b><span>Daemon accepted<br>{received_count} events</span></div>
      <div class="step"><b>04 Acknowledged</b><span>{receipt_status}<br>contiguous {highest_contiguous}</span></div>
      <div class="step"><b>05 Checked</b><span>Gaps {sequence_gaps}<br>Breaks {chain_breaks}</span></div>
      <div class="step"><b>06 Associated</b><span>{association_status}<br>{export_name}</span></div>
      <div class="step"><b>07 Verified</b><span>{verification_outcome}<br>Local POC result</span></div>
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
      <p>{association_basis}</p>{association_reason}</div>
  </section>

  <section>
    <h2>Routed / export alignment</h2>
    <div class="panel">
      <div class="alignment-chart">{alignment_bars}</div>
      <div class="chart-key"><b>Green/cyan</b> windows met the bounded similarity threshold; <em>amber</em> windows did not. Bars show aligned routed/export feature windows, not raw audio.</div>
      <div class="metrics">
        <div class="metric"><span>Method version</span><strong>{association_method}</strong></div>
        <div class="metric"><span>Confidence</span><strong>{association_confidence}</strong></div>
        <div class="metric"><span>Matched coverage</span><strong>{matched_coverage}</strong></div>
        <div class="metric"><span>Matched windows</span><strong>{matched_windows}</strong></div>
        <div class="metric"><span>Comparable</span><strong>{comparable_windows}</strong></div>
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
    <h2>Embedded C2PA claim</h2>
    <div class="panel association">{c2pa_badge}<strong> {c2pa_status}</strong>
      <p>{c2pa_detail}</p></div>
    <div class="panel" style="margin-top:12px">
      <div class="metrics">
        <div class="metric"><span>Validation state</span><strong>{c2pa_state}</strong></div>
        <div class="metric"><span>Library state</span><strong>{c2pa_library_state}</strong></div>
        <div class="metric"><span>Hard binding</span><strong>{c2pa_binding_type}</strong></div>
        <div class="metric"><span>Trust scope</span><strong>{c2pa_trust_scope}</strong></div>
        <div class="metric"><span>Signer key</span><strong>{c2pa_key_id}</strong></div>
        <div class="metric"><span>Signer identity</span><strong>{c2pa_signer_identity}</strong></div>
      </div>
      <table>
        <thead><tr><th>Ingredient</th><th>Relationship</th><th>Digest</th><th>What the digest is</th><th>Proof</th></tr></thead>
        <tbody>{c2pa_ingredient_rows}</tbody>
      </table>
    </div>
    <p>{c2pa_caveat}</p>
    <div class="panel"><ul class="unknowns">{c2pa_unresolved}</ul></div>
  </section>

  <section>
    <h2>Downstream registration handoff</h2>
    <div class="panel handoff">
      <div><span class="eyebrow">Prepared input</span><h2>{handoff_status}</h2>
        <p>Export hard hash, routed observation commitment, coverage, inferred association, declarations, signing key, and evidence bindings.</p>
        <a href="{bundle_href}">Download evidence bundle ↓</a><br>
        <a href="{bundle_index_href}">Open signed bundle index ↗</a><br>
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
        coverage_status=_escape(_humanize(coverage.get("status"), "unknown_coverage")),
        window_count=sum(
            int(stem.get("hash_chain_length", 0))
            for stem in stems
            if isinstance(stem, dict)
        ),
        claim_cards="".join(claim_cards),
        stem_rows="".join(stem_rows) or '<tr><td colspan="5">No routed-audio evidence was received.</td></tr>',
        association_badge=_proof_badge(association.get("apw:proof_level")),
        association_status=_escape(_humanize(association.get("status"), "not established")),
        association_basis=_escape(association.get("basis", "")),
        association_reason=(
            f'<p class="reason">Cause: {_escape(association["reason"])}</p>'
            if association.get("reason") else ""
        ),
        association_method=_escape(
            f"{association.get('method', 'unavailable')} v{association.get('method_version', 'unknown')}"
        ),
        association_confidence=_escape(_percentage(association.get("confidence"))),
        matched_coverage=_escape(_percentage(association.get("matched_coverage"))),
        matched_windows=_escape(
            association.get("matched_window_count")
            if association.get("matched_window_count") is not None else "Unavailable"
        ),
        comparable_windows=_escape(
            association.get("comparable_window_count")
            if association.get("comparable_window_count") is not None else "Unavailable"
        ),
        best_offset=_escape(
            f"{association.get('best_offset_seconds')} s"
            if association.get("best_offset_seconds") is not None else "Unavailable"
        ),
        alignment_bars=alignment_bars or '<span style="color:var(--muted)">Alignment unavailable</span>',
        plugin_ids=_escape(", ".join(plugin_ids) if plugin_ids else "No instance observed"),
        received_count=_escape((coverage.get("counters") or {}).get("events_received", 0)),
        emitted_count=_escape((coverage.get("counters") or {}).get("udp_sends_attempted", 0)),
        sequence_gaps=_escape((coverage.get("counters") or {}).get("sequence_gaps", 0)),
        chain_breaks=_escape((coverage.get("counters") or {}).get("hash_chain_breaks", 0)),
        receipt_status=_escape(_humanize(receipt.get("status"), "unknown")),
        highest_contiguous=_escape(first_receipt_stream.get("highest_contiguous_sequence", 0)),
        verification_outcome=_escape(_humanize(verification.get("outcome"), "untrusted")),
        verification_badge=_proof_badge(
            "directly_observed" if verification.get("outcome") == "verified" else "unknown_unobserved"
        ),
        c2pa_badge=_proof_badge(c2pa_claim.get("apw:proof_level")),
        c2pa_status=_escape(_humanize(c2pa_claim.get("status"), "unavailable")),
        c2pa_detail=_escape(
            c2pa_validation.get("detail", "")
            if c2pa_signed
            else c2pa_claim.get("reason", "No C2PA claim was produced for this export.")
        ),
        c2pa_state=_escape(_humanize(c2pa_validation.get("state"), "none")),
        c2pa_library_state=_escape(c2pa_validation.get("library_validation_state") or "Unavailable"),
        c2pa_binding_type=_escape(c2pa_binding.get("type") or "Unavailable"),
        c2pa_trust_scope=_escape(
            _humanize(c2pa_validation.get("trust_anchor_scope"), "unavailable")
        ),
        c2pa_key_id=_escape(
            (c2pa_claim.get("signer") or {}).get("key_id", "Unavailable")
            if isinstance(c2pa_claim.get("signer"), dict) else "Unavailable"
        ),
        c2pa_signer_identity=_escape("Not established"),
        c2pa_ingredient_rows=c2pa_ingredient_rows
        or '<tr><td colspan="5">No ingredients were recorded in the claim.</td></tr>',
        c2pa_caveat=_escape(
            c2pa_claim.get("scope")
            or "No C2PA claim was produced, which is not evidence about the audio itself."
        ),
        c2pa_unresolved=unresolved_refs
        or "<li>Every referenced ingredient was resolved and hashed.</li>",
        handoff_status=_escape(_humanize(handoff.get("status"), "not prepared")),
        handoff_items=handoff_items,
        handoff_href=_escape(presentation.get("downstream_handoff", "#")),
        bundle_href=_escape(presentation.get("evidence_bundle", "#")),
        bundle_index_href=_escape(presentation.get("bundle_index", "#")),
        proof_legend="".join(_proof_badge(proof) for proof in PROOF_LABELS),
        unobserved_items=unobserved_items,
        raw_manifest=raw_manifest,
    )


def write_html_report(manifest: dict[str, object], path: Path) -> Path:
    path = path.expanduser()
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(render_html_report(manifest), encoding="utf-8")
    return path

from __future__ import annotations

import html
from pathlib import Path


def _escape(value: object) -> str:
    return html.escape(str(value), quote=True)


def render_dashboard(status: dict[str, object]) -> str:
    pipeline = status.get("pipeline") if isinstance(status.get("pipeline"), dict) else {}
    counts = status.get("counts") if isinstance(status.get("counts"), dict) else {}
    links = status.get("links") if isinstance(status.get("links"), dict) else {}
    coverage = status.get("coverage") if isinstance(status.get("coverage"), dict) else {}
    receipt = (
        status.get("daemon_receipt_acknowledgement")
        if isinstance(status.get("daemon_receipt_acknowledgement"), dict)
        else {}
    )
    receipt_streams = receipt.get("streams") if isinstance(receipt.get("streams"), list) else []
    instances = status.get("plugin_instance_ids", [])
    steps = [
        ("Plug-in observed", pipeline.get("plugin_observed", "waiting")),
        ("Locally emitted", pipeline.get("plugin_emitted", "waiting")),
        ("Daemon received", pipeline.get("daemon_received", "waiting")),
        ("Daemon ACK issued", pipeline.get("daemon_acknowledged", "waiting")),
        ("Chain continuity", pipeline.get("chain_continuity", "waiting")),
        ("Export detected", pipeline.get("export_detected", "waiting")),
        ("Association evaluated", pipeline.get("audio_association", "waiting")),
        ("Verification", pipeline.get("verification", "waiting")),
    ]
    step_html = "".join(
        f'<article class="step { _escape(state) }"><i></i><span>{_escape(label)}</span><b>{_escape(state)}</b></article>'
        for label, state in steps
    )
    count_items = [
        ("Buffers submitted", counts.get("buffers_submitted", "—")),
        ("Samples submitted", counts.get("samples_submitted", "—")),
        ("Windows hashed", counts.get("windows_hashed", "—")),
        ("Windows received", counts.get("buffer_hash_events_received", 0)),
        ("FIFO samples dropped", counts.get("fifo_samples_dropped", "—")),
        ("Events prepared", counts.get("events_prepared", "—")),
        ("UDP send attempted", counts.get("udp_sends_attempted", "—")),
        ("Locally emitted", counts.get("udp_sends_locally_emitted", "—")),
        ("UDP sends failed", counts.get("udp_sends_failed", "—")),
        ("Daemon ACK sent", counts.get("daemon_acknowledgements_sent", 0)),
        ("ACK dispatch failed", counts.get("daemon_acknowledgements_failed", 0)),
        ("Events rejected", counts.get("events_rejected", 0)),
        ("Sequence gaps", counts.get("sequence_gaps", 0)),
        ("Chain breaks", counts.get("hash_chain_breaks", 0)),
    ]
    counts_html = "".join(
        f'<div><span>{_escape(label)}</span><strong>{_escape(value)}</strong></div>'
        for label, value in count_items
    )
    link_html = "".join(
        f'<a href="{_escape(target)}">{_escape(label.replace("_", " ").title())} ↗</a>'
        for label, target in links.items() if target
    ) or '<span class="muted">Artifacts appear after an export is sealed.</span>'
    stream_html = "".join(
        "<code>{instance} · {session}</code><span class=\"muted\">accepted {accepted} · contiguous {contiguous} · gaps {gaps} · rejected {rejections}</span>".format(
            instance=_escape(stream.get("plugin_instance_id", "unknown")),
            session=_escape(stream.get("plugin_capture_session_id", "unknown")),
            accepted=_escape(stream.get("highest_accepted_sequence", 0)),
            contiguous=_escape(stream.get("highest_contiguous_sequence", 0)),
            gaps=_escape(stream.get("gaps", 0)),
            rejections=_escape(stream.get("rejections", 0)),
        )
        for stream in receipt_streams if isinstance(stream, dict)
    ) or '<code>waiting</code>'
    return f"""<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<meta http-equiv="refresh" content="1"><title>Routed Audio Evidence Adapter</title>
<style>
:root{{color-scheme:dark;--bg:#080c12;--panel:#111923;--line:#243346;--text:#eef6ff;--muted:#8fa2b8;--cyan:#4de3ff;--green:#4fe3a3;--amber:#ffc45c;--red:#ff6f7d;--violet:#b89cff}}
*{{box-sizing:border-box}} body{{margin:0;background:radial-gradient(circle at 75% -20%,#163442 0,var(--bg) 42%);color:var(--text);font:14px/1.45 -apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif}}
main{{width:min(1180px,calc(100% - 44px));margin:auto;padding:42px 0 64px}} header{{display:flex;justify-content:space-between;gap:24px;align-items:end;border-bottom:1px solid var(--line);padding-bottom:26px}}
.eyebrow{{color:var(--cyan);font-size:11px;letter-spacing:.16em;text-transform:uppercase;font-weight:800}} h1{{font-size:clamp(30px,5vw,54px);line-height:1;margin:8px 0;letter-spacing:-.04em}} p,.muted{{color:var(--muted)}} code{{color:var(--cyan)}}
.state{{display:flex;align-items:center;gap:9px;color:var(--green);font-weight:700;text-transform:uppercase;letter-spacing:.08em}} .state i{{width:9px;height:9px;border-radius:50%;background:currentColor;box-shadow:0 0 18px currentColor}}
.state.error{{color:var(--red)}} .state.idle,.state.stopped{{color:var(--amber)}}
section{{margin-top:28px}} h2{{font-size:15px;margin:0 0 12px}} .pipeline{{display:grid;grid-template-columns:repeat(8,1fr);gap:8px}} .step{{min-height:112px;padding:17px;border:1px solid var(--line);border-radius:14px;background:rgba(17,25,35,.92)}}
.step i{{display:block;width:9px;height:9px;border-radius:50%;background:var(--amber);box-shadow:0 0 13px currentColor}} .step span{{display:block;margin-top:22px;color:var(--muted);font-size:12px}} .step b{{display:block;margin-top:5px;color:var(--amber);text-transform:uppercase;font-size:10px;letter-spacing:.08em}}
.step.complete i,.step.checked i,.step.evaluated i,.step.verified i,.step.issued i{{background:var(--green)}} .step.complete b,.step.checked b,.step.evaluated b,.step.verified b,.step.issued b{{color:var(--green)}} .step.error i,.step.changed i,.step.untrusted i,.step.degraded i{{background:var(--red)}} .step.error b,.step.changed b,.step.untrusted b,.step.degraded b{{color:var(--red)}}
.grid{{display:grid;grid-template-columns:1.5fr .8fr;gap:14px}} .panel{{border:1px solid var(--line);border-radius:16px;background:var(--panel);overflow:hidden}} .counts{{display:grid;grid-template-columns:repeat(5,1fr)}} .counts div{{padding:17px;border-right:1px solid var(--line);border-bottom:1px solid var(--line)}} .counts span{{display:block;color:var(--muted);font-size:10px;text-transform:uppercase;letter-spacing:.07em}} .counts strong{{display:block;font-size:21px;margin-top:6px}} .boundary{{padding:22px;border-left:3px solid var(--amber)}}
.ids{{padding:18px 22px}} .ids code{{display:block;overflow-wrap:anywhere;margin-top:5px}} .ids .muted{{display:block;margin:3px 0 10px}} .links{{display:flex;gap:10px;flex-wrap:wrap;padding:18px 22px}} a{{color:var(--cyan);text-decoration:none;border:1px solid var(--line);border-radius:999px;padding:8px 12px}} .legend{{display:flex;gap:8px;flex-wrap:wrap}} .legend span{{padding:5px 9px;border-radius:999px;background:#1d2937;color:var(--muted);font-size:11px}} .legend span:nth-child(1){{color:var(--green)}} .legend span:nth-child(2){{color:var(--amber)}} .legend span:nth-child(3){{color:var(--violet)}}
@media(max-width:850px){{header{{display:block}}.state{{margin-top:14px}}.pipeline{{grid-template-columns:1fr 1fr}}.grid{{grid-template-columns:1fr}}.counts{{grid-template-columns:1fr 1fr}}}}
</style></head><body><main>
<header><div><div class="eyebrow">Creation-stage evidence → downstream trust handoff</div><h1>Routed Audio Evidence Adapter</h1><p>Live, local, opt-in observation for one routed stem.</p></div><div><div class="state {_escape(status.get('state','idle'))}"><i></i>{_escape(status.get('state','idle'))}</div><code>{_escape(status.get('session_id','unknown'))}</code><p>{_escape(str(coverage.get('status','unknown_coverage')).replace('_',' ').title())}</p></div></header>
<section><h2>Evidence pipeline</h2><div class="pipeline">{step_html}</div></section>
<section class="grid"><div><h2>Observed, emitted, received, acknowledged</h2><div class="panel counts">{counts_html}</div></div><div><h2>Scoped receipt streams</h2><div class="panel ids"><span class="muted">Stem</span><code>{_escape(status.get('stem_id','unknown'))}</code><span class="muted">Plug-in instance · capture session</span>{stream_html}<span class="muted">ACK status: {_escape(receipt.get('status','unknown'))}. Daemon dispatch is directly observed; plug-in processing is not visible here.</span></div></div></section>
<section><h2>Current trust boundary</h2><div class="panel boundary">{_escape(status.get('trust_boundary',''))}</div></section>
<section class="grid"><div><h2>Artifacts</h2><div class="panel links">{link_html}</div></div><div><h2>Proof levels</h2><div class="legend"><span>Directly observed</span><span>Inferred</span><span>User declared</span><span>Externally verified</span><span>Unknown / unobserved</span></div></div></section>
</main></body></html>"""


def write_dashboard(status: dict[str, object], path: Path) -> Path:
    path.write_text(render_dashboard(status), encoding="utf-8")
    return path

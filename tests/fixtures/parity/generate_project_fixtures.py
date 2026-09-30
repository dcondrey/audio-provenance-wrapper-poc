#!/usr/bin/env python3
"""Regenerate the Python/Rust project-parser parity fixtures.

Everything expected is produced by the Python oracle (daemon/project_formats and
daemon/project_differ), never by hand and never by the Rust port.

    ./.venv/bin/python tests/fixtures/parity/generate_project_fixtures.py

Writes:
  tests/fixtures/projects/reaper/{basic,edited}.rpp(.golden.json)   copies of tests/fixtures/reaper
  tests/fixtures/projects/als/{basic,edited}.als(.golden.json)      CONSTRUCTED synthetic Live sets
  tests/fixtures/parity/project_parity.json   registry, unsupported events, diffs, session_facts, XML cases
"""
from __future__ import annotations

import gzip
import json
import shutil
import sys
from pathlib import Path
from xml.etree import ElementTree as ET

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]
sys.path.insert(0, str(REPO))

from daemon.manifest_builder.generator import derive_session_facts  # noqa: E402
from daemon.project_differ.differ import compute_diff, diff_to_event  # noqa: E402
from daemon.project_formats import _safe, parse_project, registered_formats, unsupported_format_event  # noqa: E402
from daemon.project_formats._snapshot import canonical_element, golden_json, snapshot_to_golden  # noqa: E402

PROJECTS = REPO / "tests" / "fixtures" / "projects"


def als_document(variant: str) -> bytes:
    """A CONSTRUCTED Ableton-style Live set exercising every element extract_snapshot reads."""
    edited = variant == "edited"
    bpm = "128" if edited else "120"
    extra_clip = (
        '\t\t\t\t\t<ClipSlot Id="2">\n\t\t\t\t\t\t<Value>\n\t\t\t\t\t\t\t<AudioClip Id="9">\n'
        '\t\t\t\t\t\t\t\t<Name Value="Added clip" />\n\t\t\t\t\t\t\t\t<CurrentStart Value="16" />\n'
        '\t\t\t\t\t\t\t\t<CurrentEnd Value="20" />\n\t\t\t\t\t\t\t</AudioClip>\n\t\t\t\t\t\t</Value>\n\t\t\t\t\t</ClipSlot>\n'
        if edited else ""
    )
    third_track = (
        '\t\t\t<ReturnTrack Id="30">\n\t\t\t\t<Name><EffectiveName Value="Reverb Return" /></Name>\n'
        '\t\t\t\t<DeviceChain><Devices><Reverb /></Devices></DeviceChain>\n\t\t\t</ReturnTrack>\n'
        if edited else ""
    )
    text = f"""<?xml version="1.0" encoding="UTF-8"?>
<Ableton MajorVersion="5" MinorVersion="11.0_11300" Creator="constructed fixture">
\t<LiveSet>
\t\t<Tracks>
\t\t\t<AudioTrack Id="10">
\t\t\t\t<Name>
\t\t\t\t\t<EffectiveName Value="Drums &amp; Bass &quot;&#233;&quot; café" />
\t\t\t\t</Name>
\t\t\t\t<TrackGroupId Value="-1" />
\t\t\t\t<ColorIndex Value="7" />
\t\t\t\t<Freeze Value="true" />
\t\t\t\t<DeviceChain>
\t\t\t\t\t<AudioInputRouting><Target Value="AudioIn/External/S0" /></AudioInputRouting>
\t\t\t\t\t<AudioOutputRouting><Target Value="AudioOut/Master" /></AudioOutputRouting>
\t\t\t\t\t<Devices>
\t\t\t\t\t\t<PluginDevice Id="1">
\t\t\t\t\t\t\t<UserName Value="My Synth" />
\t\t\t\t\t\t\t<SelectedPresetName Value="Preset A" />
\t\t\t\t\t\t</PluginDevice>
\t\t\t\t\t\t<Compressor2 Id="2" />
\t\t\t\t\t</Devices>
\t\t\t\t</DeviceChain>
\t\t\t\t<TrackSendHolder>
\t\t\t\t\t<Send><Target Value="Return/A" /><Manual Value="0.75" /></Send>
\t\t\t\t</TrackSendHolder>
\t\t\t\t<TrackSendHolder>
\t\t\t\t\t<Send><Target Value="" /><Manual Value="1" /></Send>
\t\t\t\t</TrackSendHolder>
\t\t\t\t<TrackSendHolder>
\t\t\t\t\t<Send><Target Value="Return/B" /><Manual Value="not a number" /></Send>
\t\t\t\t</TrackSendHolder>
\t\t\t\t<ClipSlotList>
\t\t\t\t\t<ClipSlot Id="0">
\t\t\t\t\t\t<Value>
\t\t\t\t\t\t\t<AudioClip Id="0">
\t\t\t\t\t\t\t\t<Name Value="Loop 1" />
\t\t\t\t\t\t\t\t<CurrentStart Value="0" />
\t\t\t\t\t\t\t\t<CurrentEnd Value="8.5" />
\t\t\t\t\t\t\t\t<WarpMode Value="4" />
\t\t\t\t\t\t\t\t<SampleRef>
\t\t\t\t\t\t\t\t\t<FileRef>
\t\t\t\t\t\t\t\t\t\t<RelativePath Value="Samples/loop 1.wav" />
\t\t\t\t\t\t\t\t\t\t<Path Value="/Users/example/Samples/loop 1.wav" />
\t\t\t\t\t\t\t\t\t</FileRef>
\t\t\t\t\t\t\t\t</SampleRef>
\t\t\t\t\t\t\t</AudioClip>
\t\t\t\t\t\t</Value>
\t\t\t\t\t</ClipSlot>
\t\t\t\t\t<ClipSlot>
\t\t\t\t\t\t<Value>
\t\t\t\t\t\t\t<MidiClip Id="1">
\t\t\t\t\t\t\t\t<Name Value="Riff" />
\t\t\t\t\t\t\t\t<CurrentStart Value="4" />
\t\t\t\t\t\t\t\t<CurrentEnd Value="{'12' if edited else '8'}" />
\t\t\t\t\t\t\t\t<Notes>
\t\t\t\t\t\t\t\t\t<KeyTracks>
\t\t\t\t\t\t\t\t\t\t<KeyTrack Id="0"><Notes><MidiNoteEvent Time="0" Duration="1" /><MidiNoteEvent Time="1" Duration="1" /></Notes></KeyTrack>
\t\t\t\t\t\t\t\t\t\t<KeyTrack Id="1"><Notes><MidiNoteEvent Time="2" Duration="0.5" /></Notes></KeyTrack>
\t\t\t\t\t\t\t\t\t</KeyTracks>
\t\t\t\t\t\t\t\t</Notes>
\t\t\t\t\t\t\t</MidiClip>
\t\t\t\t\t\t</Value>
\t\t\t\t\t</ClipSlot>
{extra_clip}\t\t\t\t</ClipSlotList>
\t\t\t\t<AutomationEnvelopes>
\t\t\t\t\t<Envelopes><AutomationEnvelope><Automation><Events>
\t\t\t\t\t\t<FloatEvent Id="0" Time="0" Value="0.5" />
\t\t\t\t\t\t<AutomationPoint Time="1" Value="0.25" /><AutomationPoint Time="2" Value="0.75" />
\t\t\t\t\t</Events></Automation></AutomationEnvelope></Envelopes>
\t\t\t\t</AutomationEnvelopes>
\t\t\t</AudioTrack>
\t\t\t<MidiTrack>
\t\t\t\t<Name><EffectiveName Value="Untitled" /></Name>
\t\t\t\t<TrackGroupId Value="10" />
\t\t\t\t<ColorIndex Value="oops" />
\t\t\t\t<DeviceChain><Devices><Operator /><Simpler><UserName Value="" /></Simpler></Devices></DeviceChain>
\t\t\t</MidiTrack>
{third_track}\t\t</Tracks>
\t\t<Transport>
\t\t\t<Tempo><Manual Value="{bpm}" /></Tempo>
\t\t\t<LoopOn Value="true" />
\t\t\t<LoopStart Value="4" />
\t\t\t<LoopLength Value="16" />
\t\t</Transport>
\t\t<TimeSignatures><RemoteableTimeSignature><Numerator Value="{'7' if edited else '3'}" /><Denominator Value="8" /></RemoteableTimeSignature></TimeSignatures>
\t\t<Locators><Locators><Locator Id="0" /><Locator Id="1" /></Locators></Locators>
\t</LiveSet>
</Ableton>
"""
    return text.encode("utf-8")


def write_fixtures() -> None:
    (PROJECTS / "reaper").mkdir(parents=True, exist_ok=True)
    for name in ("basic", "edited"):
        shutil.copyfile(REPO / "tests" / "fixtures" / "reaper" / f"{name}.rpp", PROJECTS / "reaper" / f"{name}.rpp")
    (PROJECTS / "als").mkdir(parents=True, exist_ok=True)
    for name in ("basic", "edited"):
        (PROJECTS / "als" / f"{name}.als").write_bytes(gzip.compress(als_document(name), mtime=0))
    for source in (*(PROJECTS / "reaper").glob("*.rpp"), *(PROJECTS / "als").glob("*.als")):
        snapshot = parse_project(source)
        source.with_name(source.name + ".golden.json").write_text(golden_json(snapshot), encoding="utf-8")


def xml_cases() -> list[dict]:
    def nested(depth: int, tag: str = "a") -> bytes:
        return (f"<{tag}>" * depth + f"</{tag}>" * depth).encode()

    cases: list[tuple[str, bytes, str | None]] = [
        ("simple", b"<a/>", None),
        ("attributes_both_quotes", b"<a x='1' y=\"2\"/>", None),
        ("nested_text_and_tails", b"<a>t0<b>in</b>t1<c/>t2</a>", None),
        ("whitespace_between_children", b"<a>\n  <b/>\n  <c/>\n</a>\n", None),
        ("predefined_entities", b"<a x='&lt;&amp;&gt;&quot;&apos;'>&lt;&amp;&gt;&quot;&apos;</a>", None),
        ("char_refs", b"<a x='&#65;&#x42;'>&#67;&#x44;&#233;&#x1F600;</a>", None),
        ("cdata", b"<a><![CDATA[<&>]]> tail <![CDATA[x]]></a>", None),
        ("cdata_with_crlf", b"<a><![CDATA[l1\r\nl2\rl3]]></a>", None),
        ("comments_and_pi", b"<?pi data?><!-- c --><a>x<!-- inner -->y<?p q?>z</a><!-- after -->", None),
        ("crlf_in_text", b"<a>l1\r\nl2\rl3</a>", None),
        ("crlf_and_tab_in_attribute", b"<a x='p\r\nq\rr\ts\nt'/>", None),
        ("char_ref_newline_in_attribute_survives", b"<a x='p&#10;q&#9;r&#13;s'/>", None),
        ("bom", b"\xef\xbb\xbf<a/>", None),
        ("xml_decl", b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<a/>", None),
        ("xml_decl_single_quotes", b"<?xml version='1.0'?><a/>", None),
        ("unicode_names_and_text", "<café å='ü'>中文</café>".encode(), None),
        ("empty_element_pair", b"<a></a>", None),
        ("whitespace_only_text", b"<a> </a>", None),
        ("mixed_content_text_accumulates", b"<a>one<b/>two<b/>three</a>", None),
        ("names_with_colon_dash_dot_underscore", b"<a:b c-d='1' e.f='2' _g='3'/>", None),
        ("attribute_order_kept", b"<a z='1' a='2' m='3'/>", None),
        ("attr_space_around_equals", b"<a x = '1' />", None),
        ("attr_gt_in_value", b"<a x='a>b'/>", None),
        ("end_tag_trailing_space", b"<a></a  >", None),
        ("depth_128", nested(128), None),
        ("depth_129", nested(129), None),
        ("lmms_doctype_allowed", b"<!DOCTYPE lmms-project>\n<lmms-project/>", "lmms-project"),
        ("lmms_doctype_extra_space", b"<!DOCTYPE  lmms-project  >\n<lmms-project/>", "lmms-project"),
        ("lmms_doctype_when_not_allowed", b"<!DOCTYPE lmms-project>\n<lmms-project/>", None),
        ("other_doctype_name", b"<!DOCTYPE other>\n<a/>", "lmms-project"),
        ("doctype_with_system", b"<!DOCTYPE lmms-project SYSTEM 'x.dtd'><a/>", "lmms-project"),
        ("doctype_with_public", b"<!DOCTYPE lmms-project PUBLIC 'p' 'x.dtd'><a/>", "lmms-project"),
        ("doctype_internal_subset", b"<!DOCTYPE lmms-project [<!ENTITY e 'x'>]><a>&e;</a>", "lmms-project"),
        ("doctype_empty_internal_subset", b"<!DOCTYPE lmms-project []><a/>", "lmms-project"),
        ("doctype_after_root", b"<a/><!DOCTYPE lmms-project>", "lmms-project"),
        ("second_doctype", b"<!DOCTYPE lmms-project><!DOCTYPE lmms-project><a/>", "lmms-project"),
        ("doctype_text_inside_cdata_is_fine", b"<a><![CDATA[<!DOCTYPE x [<!ENTITY e 'y'>]>]]></a>", None),
        ("doctype_text_inside_comment_is_fine", b"<!-- <!DOCTYPE x> --><a/>", None),
        ("undefined_entity", b"<a>&nbsp;</a>", None),
        ("undefined_entity_in_attribute", b"<a x='&e;'/>", None),
        ("bare_ampersand", b"<a>a & b</a>", None),
        ("lt_in_attribute", b"<a x='<'/>", None),
        ("duplicate_attribute", b"<a x='1' x='2'/>", None),
        ("unquoted_attribute", b"<a x=1/>", None),
        ("attribute_without_value", b"<a x/>", None),
        ("no_space_between_attributes", b"<a x='1'y='2'/>", None),
        ("unclosed_element", b"<a><b></a>", None),
        ("unclosed_at_eof", b"<a>", None),
        ("mismatched_case", b"<a></A>", None),
        ("two_roots", b"<a/><b/>", None),
        ("text_after_root", b"<a/>tail", None),
        ("text_before_root", b"lead<a/>", None),
        ("empty_document", b"", None),
        ("whitespace_only_document", b"  \n ", None),
        ("comment_only_document", b"<!-- x -->", None),
        ("name_starts_with_digit", b"<1a/>", None),
        ("name_starts_with_dash", b"<-a/>", None),
        ("cdata_end_in_text", b"<a>]]></a>", None),
        ("cdata_end_split_in_text", b"<a>]]&gt;</a>", None),
        ("char_ref_zero", b"<a>&#0;</a>", None),
        ("char_ref_surrogate", b"<a>&#xD800;</a>", None),
        ("char_ref_too_big", b"<a>&#x110000;</a>", None),
        ("char_ref_no_digits", b"<a>&#;</a>", None),
        ("char_ref_bad_hex", b"<a>&#xZZ;</a>", None),
        ("control_char_in_text", b"<a>\x01</a>", None),
        ("control_char_in_attribute", b"<a x='\x01'/>", None),
        ("nul_in_text", b"<a>\x00</a>", None),
        ("unterminated_comment", b"<a/><!-- x", None),
        ("double_dash_in_comment", b"<a/><!-- a -- b -->", None),
        ("comment_ending_triple_dash", b"<a/><!-- a --->", None),
        ("unterminated_cdata", b"<a><![CDATA[x</a>", None),
        ("unterminated_pi", b"<a/><?p q", None),
        ("xml_decl_not_first", b"<a/><?xml version='1.0'?>", None),
        ("xml_decl_after_space", b" <?xml version='1.0'?><a/>", None),
        ("xml_decl_missing_version", b"<?xml encoding='UTF-8'?><a/>", None),
        ("invalid_utf8", b"<a>\xff</a>", None),
        ("utf16_declared_but_utf8_bytes", b"<?xml version='1.0' encoding='UTF-16'?><a/>", None),
        ("stray_end_tag", b"</a>", None),
        ("lone_lt", b"<a><</a>", None),
        ("unterminated_start_tag", b"<a x='1'", None),
        ("unterminated_attribute_value", b"<a x='1/>", None),
        ("end_tag_with_attribute", b"<a></a x='1'>", None),
        ("deeply_nested_siblings", b"<r>" + b"<c/>" * 200 + b"</r>", None),
        ("nonchar_fffe", "<a>￾</a>".encode(), None),
        ("tail_after_cdata_and_comment", b"<r><a/>x<!--c-->y<![CDATA[z]]>w</r>", None),
        ("attr_value_with_entity_and_whitespace", b"<a x='  a &amp;  b\n'/>", None),
        ("empty_attr_value", b"<a x=''/>", None),
        ("plain_gt_in_text", b"<a>1 > 0</a>", None),
    ]
    out = []
    for name, data, doctype in cases:
        try:
            root = _safe.parse_xml(data, allowed_doctype=doctype)
            safe = {"ok": canonical_element(root)}
        except ValueError as error:
            safe = {"error": str(error)}
        entry: dict = {"name": name, "hex": data.hex(), "allowed_doctype": doctype, "safe": safe}
        if b"<!DOCTYPE" not in data and name not in ("depth_128", "depth_129"):
            try:
                parser = ET.XMLParser(target=ET.TreeBuilder())
                parser.feed(data)
                root = parser.close()
                entry["et"] = {"ok": ET.tostring(root).decode("ascii")}
            except (ET.ParseError, ValueError):
                entry["et"] = {"error": True}
        out.append(entry)
    return out


def build_payload() -> dict:
    registry = [
        {"format_id": f.format_id, "host": f.host, "extensions": list(f.extensions), "status": f.status,
         "reason": f.reason, "validation": f.validation}
        for f in registered_formats()
    ]
    events = {}
    for f in registered_formats():
        if f.status == "unsupported":
            event = unsupported_format_event(f, Path("/some/dir/Project" + f.extensions[0].upper()))
            event.pop("timestamp_ms"); event.pop("daemon_observed_monotonic_ms")
            events[f.format_id] = event
    snapshots = {}
    facts = {}
    for path in sorted(PROJECTS.rglob("*.golden.json")):
        source = path.with_name(path.name[: -len(".golden.json")])
        snapshot = parse_project(source)
        key = str(source.relative_to(PROJECTS))
        snapshots[key] = snapshot
        facts[key] = derive_session_facts(snapshot)
    diffs = []
    for a, b in (("reaper/basic.rpp", "reaper/edited.rpp"), ("als/basic.als", "als/edited.als"), ("als/edited.als", "als/basic.als"),
                 ("reaper/basic.rpp", "reaper/basic.rpp")):
        diff = compute_diff(snapshots[a], snapshots[b])
        event = diff_to_event(diff)
        for key in ("source_timestamp_ms", "timestamp_ms", "daemon_observed_monotonic_ms"):
            event.pop(key)
        diffs.append({"from": a, "to": b, "has_changes": diff.has_changes(), "event": event})
    return {"registry": registry, "unsupported_events": events, "session_facts": facts, "diffs": diffs,
            "xml": xml_cases()}


def write_modules_corpus() -> None:
    """Verdicts of the Python oracle for the shared .xm/.mod/.vcv corpus (see module_corpus.py)."""
    sys.path.insert(0, str(PROJECTS))
    import module_corpus

    module_corpus.BASES.clear()
    cases = module_corpus.build_cases()
    for case in cases:
        case["expect"] = module_corpus.evaluate(case)
    document = {"bases": dict(module_corpus.BASES), "cases": cases}
    target = HERE / "project_modules_corpus.json"
    target.write_text(json.dumps(document, separators=(",", ":"), ensure_ascii=True) + "\n", encoding="utf-8")
    ok = sum(1 for case in cases if "ok" in case["expect"])
    print(f"wrote {target}: {len(cases)} cases, {ok} accepted, {len(cases) - ok} refused")


def main() -> None:
    write_fixtures()
    write_modules_corpus()
    payload = build_payload()
    (HERE / "project_parity.json").write_text(json.dumps(payload, indent=1, ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"wrote {HERE / 'project_parity.json'}")


if __name__ == "__main__":
    main()

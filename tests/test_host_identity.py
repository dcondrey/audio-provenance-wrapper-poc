import json

import pytest

from daemon import host_identity as hi


def doc(**over):
    base = {
        "schema_version": 1,
        "hosts": [{
            "host_id": "a", "display_name": "A", "source_url": "https://example.org/a",
            "match": [{"platform": "linux", "executable_name": "aprog"}],
        }],
    }
    base.update(over)
    return json.dumps(base).encode()


def test_shipped_table_valid_and_unique():
    table = hi.load_table()
    assert table
    assert all(r["source_url"].startswith("https://") for r in table.values())
    raw = json.loads(hi.DEFAULT_TABLE_PATH.read_text())
    keys = [(m["platform"], hi.normalise_executable_name(m["executable_name"]))
            for h in raw["hosts"] for m in h["match"]]
    assert len(keys) == len(set(keys)) == len(table)
    mapped = {h["host_id"] for h in raw["hosts"]}
    assert not mapped & {u["host_id"] for u in raw["unmapped"]}


def test_duplicate_match_across_hosts_rejected():
    d = json.loads(doc())
    d["hosts"].append({"host_id": "b", "display_name": "B", "source_url": "https://e.org/b",
                       "match": [{"platform": "linux", "executable_name": "APROG"}]})
    with pytest.raises(hi.HostTableError):
        hi.parse_table(json.dumps(d).encode())


@pytest.mark.parametrize("raw", [
    b"not json", b"[]", b"\xff\xfe",
    doc(schema_version=2), doc(schema_version=True), doc(hosts="x"),
    doc(hosts=[{"host_id": "a", "display_name": "A", "source_url": "http://x", "match": [
        {"platform": "linux", "executable_name": "a"}]}]),
    doc(hosts=[{"host_id": "a", "display_name": "A", "source_url": "https://x", "match": [
        {"platform": "beos", "executable_name": "a"}]}]),
    doc(hosts=[{"host_id": "a", "display_name": "A", "source_url": "https://x", "match": [
        {"platform": "linux", "executable_name": "a.exe"}]}]),
    doc(hosts=[{"host_id": "a", "display_name": "A", "source_url": "https://x", "match": [
        {"platform": "linux", "executable_name": "/usr/bin/a"}]}]),
    doc(hosts=[{"host_id": "a", "display_name": "A", "source_url": "https://x", "match": []}]),
])
def test_malformed_rejected(raw):
    with pytest.raises(hi.HostTableError):
        hi.parse_table(raw)


def test_size_cap_boundary():
    ok = doc()
    padded = ok[:-1] + b" " * (hi.MAX_TABLE_BYTES - len(ok)) + b"}"
    assert len(padded) == hi.MAX_TABLE_BYTES
    hi.parse_table(padded)
    with pytest.raises(hi.HostTableError):
        hi.parse_table(padded + b" ")


def test_load_missing_file(tmp_path):
    with pytest.raises(hi.HostTableError):
        hi.load_table(tmp_path / "nope.json")


@pytest.mark.parametrize("name,platform", [
    ("LMMS", "linux"), ("lmms.exe", "windows"), ("Audacity4.EXE", "windows"),
    ("zrythm", None), ("pd", "macos"),
])
def test_table_match_is_inferred(name, platform):
    r = hi.identify_host(name, False, None, platform)
    assert r.recognised and r.identification == "inferred_from_executable_name"
    assert r.proof_level == "inferred" and r.display_name == r.host_name
    assert r.source_url.startswith("https://") and r.host_id


@pytest.mark.parametrize("name,platform", [
    ("lmms-bin", "linux"), ("xlmms", "linux"), ("lmms.app", "macos"), ("lmms.exe.exe", "windows"),
    ("/usr/bin/lmms", "linux"), ("Rac", "linux"), ("Racks", "linux"), ("audacity4", "linux"),
    ("pdf", "linux"), ("", "linux"), (None, "linux"), ("lmms", "beos"), (" lmms x", None),
])
def test_near_misses_do_not_match(name, platform):
    r = hi.identify_host(name, False, None, platform)
    assert not r.recognised and r.identification == "unrecognised"
    assert r.proof_level == "unknown_unobserved" and r.host_name is None


def test_juce_never_overridden():
    r = hi.identify_host("lmms", True, "Ableton Live", "linux")
    assert (r.host_name, r.identification, r.proof_level) == ("Ableton Live", "juce_plugin_host_type", "directly_observed")
    assert r.source_url is None


def test_ambiguous_across_platforms_without_platform_is_unmatched():
    table = hi.parse_table(json.dumps({"schema_version": 1, "hosts": [
        {"host_id": "a", "display_name": "A", "source_url": "https://e.org", "match": [
            {"platform": "linux", "executable_name": "x"}]},
        {"host_id": "b", "display_name": "B", "source_url": "https://e.org", "match": [
            {"platform": "windows", "executable_name": "x"}]}]}).encode())
    assert not hi.identify_host("x", False, None, None, table).recognised
    assert hi.identify_host("x", False, None, "linux", table).host_id == "a"

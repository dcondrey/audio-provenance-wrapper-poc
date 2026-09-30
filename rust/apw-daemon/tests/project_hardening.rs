//! Untrusted-input bounds of the project parsers: limit-1, limit and limit+1 for
//! every cap (lowered through `Limits`, as the Python tests lower the module
//! constants), traversal rejection, DTD and entity refusal, and decompression
//! bombs. Inputs here are constructed.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::io::Write;

use apw_daemon::project::safe::{check_json_bounds, check_member_name, inflate_zlib, open_zip, parse_json, read_project_bytes};
use apw_daemon::project::xml::{parse_xml, XmlLimits};
use apw_daemon::project::{ardour, dawproject, lmms, maxpat, puredata, reaper, safe, tarzst, Limits};

mod common;

fn limits() -> Limits {
    Limits::default()
}

/// Assert `ok` accepts limit-1 and limit and rejects limit+1.
fn boundary<T, E: std::fmt::Debug>(limit: usize, what: &str, run: impl Fn(usize) -> Result<T, E>) {
    assert!(run(limit - 1).is_ok(), "{what}: limit-1 was refused");
    assert!(run(limit).is_ok(), "{what}: limit was refused");
    assert!(run(limit + 1).is_err(), "{what}: limit+1 was accepted");
}

// ---- a minimal stored/deflated zip writer, able to lie about sizes and repeat names ----

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFF_u32;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

struct Entry<'a> {
    name: &'a str,
    data: &'a [u8],
    deflate: bool,
}

fn raw_zip(entries: &[Entry]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for entry in entries {
        let stored: Vec<u8> = if entry.deflate {
            let mut encoder = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
            encoder.write_all(entry.data).unwrap();
            encoder.finish().unwrap()
        } else {
            entry.data.to_vec()
        };
        let method: u16 = if entry.deflate { 8 } else { 0 };
        let offset = out.len() as u32;
        let crc = crc32(entry.data);
        let mut header = Vec::new();
        header.extend(0x0403_4b50_u32.to_le_bytes());
        header.extend([20, 0, 0, 0]);
        header.extend(method.to_le_bytes());
        header.extend([0, 0, 0x21, 0]);
        header.extend(crc.to_le_bytes());
        header.extend((stored.len() as u32).to_le_bytes());
        header.extend((entry.data.len() as u32).to_le_bytes());
        header.extend((entry.name.len() as u16).to_le_bytes());
        header.extend([0, 0]);
        header.extend(entry.name.as_bytes());
        out.extend(&header);
        out.extend(&stored);
        central.extend(0x0201_4b50_u32.to_le_bytes());
        central.extend([20, 0, 20, 0, 0, 0]);
        central.extend(method.to_le_bytes());
        central.extend([0, 0, 0x21, 0]);
        central.extend(crc.to_le_bytes());
        central.extend((stored.len() as u32).to_le_bytes());
        central.extend((entry.data.len() as u32).to_le_bytes());
        central.extend((entry.name.len() as u16).to_le_bytes());
        central.extend([0; 12]);
        central.extend(offset.to_le_bytes());
        central.extend(entry.name.as_bytes());
    }
    let central_offset = out.len() as u32;
    out.extend(&central);
    out.extend(0x0605_4b50_u32.to_le_bytes());
    out.extend([0, 0, 0, 0]);
    out.extend((entries.len() as u16).to_le_bytes());
    out.extend((entries.len() as u16).to_le_bytes());
    out.extend((central.len() as u32).to_le_bytes());
    out.extend(central_offset.to_le_bytes());
    out.extend([0, 0]);
    out
}

fn stored<'a>(name: &'a str, data: &'a [u8]) -> Entry<'a> {
    Entry { name, data, deflate: false }
}

const PROJECT_XML: &[u8] = b"<Project><Transport><Tempo unit=\"bpm\" value=\"120\"/></Transport></Project>";

// ---- file size ----

#[test]
fn the_project_file_size_cap_has_the_right_boundary() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("p.bin");
    boundary(100, "file size", |size| {
        std::fs::write(&path, vec![b'x'; size]).unwrap();
        read_project_bytes(&path, &Limits { max_project_file_bytes: 100, ..limits() })
    });
}

// ---- zip ----

#[test]
fn zip_member_count_and_declared_size_caps() {
    let names = ["a", "b", "c", "d", "e"];
    boundary(3, "zip members", |count| {
        let entries: Vec<Entry> = names.iter().take(count).map(|name| stored(name, b"x")).collect();
        open_zip(&raw_zip(&entries), &Limits { max_zip_members: 3, ..limits() }).map(|_| ())
    });
    boundary(100, "zip declared total", |total| {
        let data = vec![b'x'; total];
        open_zip(&raw_zip(&[stored("a", &data)]), &Limits { max_zip_total_bytes: 100, ..limits() }).map(|_| ())
    });
}

#[test]
fn zip_member_names_are_validated() {
    for bad in ["../evil", "a/../evil", "/abs", "C:\\x", "c:/x", "a\\..\\b", "nul\0name", ""] {
        assert!(check_member_name(bad).is_err(), "{bad:?} must be refused");
        if !bad.is_empty() && !bad.contains('\0') {
            assert!(open_zip(&raw_zip(&[stored(bad, b"x")]), &limits()).is_err(), "{bad:?} in an archive");
        }
    }
    for good in ["project.xml", "a/b.xml", "dir/..name", "a..b", "a/./b"] {
        assert!(check_member_name(good).is_ok(), "{good:?} must be accepted");
    }
    let duplicate = raw_zip(&[stored("a", b"1"), stored("a", b"2")]);
    assert!(open_zip(&duplicate, &limits()).err().unwrap().contains("duplicate archive member"));
}

#[test]
fn a_member_that_inflates_past_its_cap_is_refused_whatever_it_declares() {
    boundary(1000, "member cap (stored)", |size| {
        let data = vec![b'a'; size];
        let bytes = raw_zip(&[stored("m", &data)]);
        open_zip(&bytes, &limits()).unwrap().read_member("m", 1000)
    });
    boundary(1000, "member cap (deflated)", |size| {
        let data = vec![b'a'; size];
        let bytes = raw_zip(&[Entry { name: "m", data: &data, deflate: true }]);
        open_zip(&bytes, &limits()).unwrap().read_member("m", 1000)
    });
    // A bomb: 10 MiB of zeros deflates to a few KiB and must stop at the cap.
    let bomb = vec![0_u8; 10 * 1024 * 1024];
    let bytes = raw_zip(&[Entry { name: "m", data: &bomb, deflate: true }]);
    assert!(bytes.len() < 64 * 1024);
    assert!(open_zip(&bytes, &limits()).unwrap().read_member("m", 4096).unwrap_err().contains("decompresses past 4096"));
    assert!(open_zip(&bytes, &limits()).unwrap().read_member("missing", 10).is_err());
}

#[test]
fn a_dawproject_needs_project_xml_and_a_root_project_element() {
    assert!(dawproject::extract_dawproject(b"not a zip", &limits()).is_err());
    assert!(dawproject::extract_dawproject(&raw_zip(&[stored("other.xml", b"<Project/>")]), &limits())
        .unwrap_err()
        .contains("no project.xml"));
    assert!(dawproject::extract_dawproject(&raw_zip(&[stored("project.xml", b"<Song/>")]), &limits())
        .unwrap_err()
        .contains("not <Project>"));
    assert!(dawproject::extract_dawproject(&raw_zip(&[stored("project.xml", PROJECT_XML)]), &limits()).is_ok());
}

#[test]
fn a_dawproject_state_member_is_hashed_only_when_named_present_and_internal() {
    let xml = |state: &str| {
        format!("<Project><Structure><Track id='t'><Channel><Devices><ClapPlugin deviceName='X'>{state}</ClapPlugin></Devices></Channel></Track></Structure></Project>")
    };
    let with = |state: &str, members: &[Entry]| {
        let document = xml(state);
        let mut entries = vec![stored("project.xml", document.as_bytes())];
        entries.extend(members.iter().map(|e| Entry { name: e.name, data: e.data, deflate: e.deflate }));
        dawproject::extract_dawproject(&raw_zip(&entries), &limits()).map(|p| p.tracks[0].device_presets.clone())
    };
    let member = [stored("presets/x.state", b"state bytes")];
    let present = with("<State path='presets/x.state'/>", &member).unwrap();
    assert_eq!(present[0].len(), 16);
    assert_eq!(with("<State path='presets/y.state'/>", &member).unwrap()[0], "");
    assert_eq!(with("<State path='presets/x.state' external='true'/>", &member).unwrap()[0], "");
    assert_eq!(with("", &member).unwrap()[0], "");
    // The state member is capped at the project file cap.
    let big = [stored("presets/x.state", &[b'z'; 200])];
    let small_cap = Limits { max_project_file_bytes: 199, ..limits() };
    let document = xml("<State path='presets/x.state'/>");
    let entries = [stored("project.xml", document.as_bytes()), stored("presets/x.state", big[0].data)];
    assert!(dawproject::extract_dawproject(&raw_zip(&entries), &small_cap).is_err());
}

#[test]
fn dawproject_track_nesting_is_capped() {
    let nested = |depth: usize| {
        let mut xml = String::from("<Project><Structure>");
        for i in 0..depth {
            xml.push_str(&format!("<Track id='t{i}'>"));
        }
        for _ in 0..depth {
            xml.push_str("</Track>");
        }
        xml.push_str("</Structure></Project>");
        dawproject::extract_dawproject(&raw_zip(&[stored("project.xml", xml.as_bytes())]), &Limits { max_dawproject_track_depth: 5, ..limits() })
    };
    // limit-1, limit and limit+1 nested tracks.
    assert!(nested(4).is_ok());
    assert!(nested(5).is_ok());
    assert!(nested(6).is_err());
}

// ---- XML ----

fn xml_limits() -> XmlLimits {
    XmlLimits { max_bytes: 1 << 20, max_depth: 5, max_elements: 6, et_mode: false }
}

#[test]
fn xml_depth_element_and_byte_caps_have_the_right_boundary() {
    let nested = |depth: usize| format!("{}{}", "<a>".repeat(depth), "</a>".repeat(depth));
    boundary(5, "xml depth", |depth| parse_xml(nested(depth).as_bytes(), None, xml_limits()));
    boundary(6, "xml elements", |count| {
        let body = "<c/>".repeat(count - 1);
        parse_xml(format!("<r>{body}</r>").as_bytes(), None, xml_limits())
    });
    let sized = |size: usize| {
        let filler = "x".repeat(size - "<a></a>".len());
        parse_xml(format!("<a>{filler}</a>").as_bytes(), None, XmlLimits { max_bytes: 100, ..xml_limits() })
    };
    boundary(100, "xml bytes", sized);
}

#[test]
fn dtds_and_entities_are_refused_structurally() {
    let refuse = |xml: &str| parse_xml(xml.as_bytes(), Some("lmms-project"), xml_limits()).unwrap_err();
    assert!(refuse("<!DOCTYPE lmms-project SYSTEM 'x'><lmms-project/>").contains("DTD"));
    assert!(refuse("<!DOCTYPE lmms-project [<!ENTITY e 'x'>]><lmms-project>&e;</lmms-project>").contains("DTD"));
    assert!(refuse("<!DOCTYPE billion [<!ENTITY a 'aaaa'>]><billion>&a;</billion>").contains("DTD"));
    assert!(refuse("<lmms-project>&undefined;</lmms-project>").contains("undefined entity"));
    // A DOCTYPE lookalike inside CDATA or a comment is not a DTD.
    assert!(parse_xml(b"<a><![CDATA[<!DOCTYPE x [<!ENTITY e 'y'>]>]]></a>", None, xml_limits()).is_ok());
    assert!(parse_xml(b"<!-- <!DOCTYPE x> --><a/>", None, xml_limits()).is_ok());
    // UTF-16 cannot hide a DTD: the bytes are not UTF-8 and are refused outright.
    let mut utf16 = vec![0xFF, 0xFE];
    utf16.extend("<!DOCTYPE lmms-project [<!ENTITY e 'x'>]><a/>".encode_utf16().flat_map(u16::to_le_bytes));
    assert!(parse_xml(&utf16, Some("lmms-project"), xml_limits()).is_err());
}

// ---- zlib (LMMS qCompress) ----

fn zlib(data: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(data).unwrap();
    encoder.finish().unwrap()
}

#[test]
fn zlib_output_is_capped_and_the_lmms_header_must_agree() {
    boundary(1000, "zlib cap", |size| inflate_zlib(&zlib(&vec![b'a'; size]), 1000));
    let bomb = zlib(&vec![0_u8; 20 * 1024 * 1024]);
    assert!(bomb.len() < 64 * 1024);
    assert!(inflate_zlib(&bomb, 4096).unwrap_err().contains("decompresses past 4096"));
    assert!(inflate_zlib(b"not zlib", 100).is_err());

    let xml = b"<!DOCTYPE lmms-project>\n<lmms-project type='song'><head bpm='120'/><song/></lmms-project>";
    let framed = |declared: u32| {
        let mut data = declared.to_be_bytes().to_vec();
        data.extend(zlib(xml));
        data
    };
    assert!(lmms::extract_lmms(&framed(xml.len() as u32), &limits()).is_ok());
    assert!(lmms::extract_lmms(&framed(xml.len() as u32 - 1), &limits()).is_err());
    assert!(lmms::extract_lmms(&framed(xml.len() as u32 + 1), &limits()).is_err());
    let cap = Limits { max_xml_bytes: xml.len() - 1, ..limits() };
    assert!(lmms::extract_lmms(&framed(xml.len() as u32), &cap).unwrap_err().contains("past"));
    assert!(lmms::extract_lmms(b"abc", &limits()).is_err());
    // Plain XML wins whatever the extension, like DataFile::loadData.
    assert!(lmms::extract_lmms(xml, &limits()).is_ok());
    assert!(lmms::extract_lmms(b"<lmms-project type='song'/>", &limits()).is_ok());
    assert!(lmms::extract_lmms(b"<lmms-project type='pattern'/>", &limits()).unwrap_err().contains("only song projects"));
    assert!(lmms::extract_lmms(b"<other/>", &limits()).is_err());
}

// ---- JSON (Max) ----

#[test]
fn json_depth_and_node_caps_have_the_right_boundary() {
    let nested = |depth: usize| format!("{}{}", "[".repeat(depth), "]".repeat(depth));
    boundary(4, "json depth", |depth| parse_json(nested(depth).as_bytes(), &Limits { max_json_depth: 4, ..limits() }));
    boundary(10, "json nodes", |count| {
        let items = vec!["1"; count - 1].join(",");
        parse_json(format!("[{items}]").as_bytes(), &Limits { max_json_nodes: 10, ..limits() })
    });
    // The parser's own recursion limit (127) must not undercut the 128 cap.
    assert!(parse_json(nested(128).as_bytes(), &limits()).is_ok());
    assert!(parse_json(nested(129).as_bytes(), &limits()).is_err());
    assert!(parse_json(b"{\"a\": tru}", &limits()).is_err());
    assert!(parse_json(b"[1] trailing", &limits()).is_err());
    assert!(parse_json(&[0xEF, 0xBB, 0xBF, b'[', b']'], &limits()).is_ok());
    assert!(check_json_bounds(&serde_json::json!([[[1]]]), &Limits { max_json_depth: 2, ..limits() }).is_err());
    // Nesting inside a string is not nesting.
    assert!(parse_json(b"[\"[[[[[[[[[[\"]", &Limits { max_json_depth: 2, ..limits() }).is_ok());
}

#[test]
fn maxpat_patcher_count_is_capped() {
    let doc = |subpatchers: usize| {
        let boxes: Vec<String> = (0..subpatchers)
            .map(|i| format!("{{\"box\":{{\"id\":\"b{i}\",\"maxclass\":\"newobj\",\"text\":\"p s{i}\",\"patcher\":{{\"boxes\":[]}}}}}}"))
            .collect();
        format!("{{\"patcher\":{{\"boxes\":[{}]}}}}", boxes.join(","))
    };
    let cap = Limits { max_maxpat_patchers: 4, ..limits() };
    boundary(4, "patchers", |count| maxpat::extract_maxpat(doc(count - 1).as_bytes(), &cap));
    assert!(maxpat::extract_maxpat(b"{\"nope\":1}", &limits()).is_err());
    assert!(maxpat::extract_maxpat(b"[]", &limits()).is_err());
}

// ---- Pure Data ----

#[test]
fn pd_statement_canvas_and_depth_caps() {
    let cap = Limits { max_pd_statements: 5, max_pd_canvas_depth: 3, max_pd_canvases: 4, ..limits() };
    boundary(5, "pd statements", |count| {
        let body = "#X obj 0 0 osc~;".repeat(count - 1);
        puredata::extract_pd(&format!("#N canvas 0 0 1 1 12;{body}"), &cap)
    });
    let nested = |depth: usize| {
        let opens = "#N canvas 0 0 1 1 sub 0;".repeat(depth - 1);
        let closes = "#X restore 0 0 pd sub;".repeat(depth - 1);
        puredata::extract_pd(&format!("#N canvas 0 0 1 1 12;{opens}{closes}"), &Limits { max_pd_statements: 1000, ..cap.clone() })
    };
    assert!(nested(2).is_ok());
    assert!(nested(3).is_ok());
    assert!(nested(4).is_err());
    assert!(puredata::extract_pd("#N canvas 0 0 1 1 12;#N canvas 0 0 1 1 a 0;#X restore 0 0 pd a;#N canvas 0 0 1 1 b 0;#X restore 0 0 pd b;#N canvas 0 0 1 1 c 0;#X restore 0 0 pd c;#N canvas 0 0 1 1 d 0;", &Limits { max_pd_statements: 1000, ..cap.clone() }).is_err());
    assert!(puredata::extract_pd("#X obj 0 0 x;", &limits()).unwrap_err().contains("first record"));
    assert!(puredata::extract_pd("#N canvas 0 0 1 1 12;#X restore 0 0 pd x;", &limits()).is_err());
    assert!(puredata::extract_pd("#N canvas 0 0 1 1 12;#X restore 0 0 pd x;#N canvas 1;", &limits()).is_err());
    assert!(puredata::extract_pd("#N canvas 0 0 1 1 12;#X obj 0 0 a;#X restore 0 0 pd x;", &limits()).is_err());
}

#[test]
fn pd_escapes_and_atoms_follow_the_oracle_rules() {
    let statements = puredata::tokenize("#X msg 0 0 a\\; b\\, c, d\\\\ e;\n#X obj 1 2 tabread~ drum.wav\\ x;", &limits()).unwrap();
    assert_eq!(statements[0], ["#X", "msg", "0", "0", "a;", "b,", "c", ",", "d\\", "e"]);
    assert_eq!(statements[1], ["#X", "obj", "1", "2", "tabread~", "drum.wav x"]);
    // A trailing lone backslash is kept literally; an unterminated last statement counts.
    assert_eq!(puredata::tokenize("a b\\", &limits()).unwrap(), [vec!["a", "b\\"]]);
}

// ---- REAPER ----

fn rpp(text: &str, limits: &Limits) -> Result<apw_daemon::project::ProjectSnapshot, String> {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("p.rpp");
    std::fs::write(&path, text).unwrap();
    reaper::extract_reaper_snapshot_from_text(text, &path, limits)
}

#[test]
fn rpp_depth_node_and_line_caps() {
    let nested = |depth: usize| {
        let mut text = String::from("<REAPER_PROJECT 0.1\n");
        for _ in 0..depth - 1 {
            text.push_str("<X\n");
        }
        for _ in 0..depth - 1 {
            text.push_str(">\n");
        }
        text.push_str(">\n");
        text
    };
    // The root is depth 1; the limit applies to how many blocks may be open.
    let cap = Limits { max_rpp_depth: 3, max_rpp_nodes: 1000, max_rpp_lines: 1000, ..limits() };
    assert!(rpp(&nested(3), &cap).is_ok());
    assert!(rpp(&nested(4), &cap).is_err());
    let cap = Limits { max_rpp_nodes: 4, max_rpp_lines: 1000, ..limits() };
    boundary(4, "rpp nodes", |count| rpp(&format!("<REAPER_PROJECT 0.1\n{}>\n", "<X\n>\n".repeat(count - 1)), &cap));
    let cap = Limits { max_rpp_lines: 6, ..limits() };
    boundary(6, "rpp lines", |count| rpp(&format!("<REAPER_PROJECT 0.1\n{}>\n", "\n".repeat(count - 2)), &cap));
    for bad in ["", "TEMPO 120\n", "<TRACK\n>\n", "<REAPER_PROJECT 0.1\n", "<REAPER_PROJECT 0.1\n>\n>\n", "<REAPER_PROJECT 0.1\n>\n<REAPER_PROJECT 0.1\n>\n", "<REAPER_PROJECT 0.1\n>\ncontent\n", "<REAPER_PROJECT 0.1\n<\n>\n>\n"] {
        assert!(rpp(bad, &limits()).is_err(), "{bad:?}");
    }
}

#[test]
fn rpp_splits_lines_and_arguments_like_python() {
    // \r, \r\n, \v, \f, \x1c-\x1e, \x85 and the Unicode separators all end a line.
    let text = "<REAPER_PROJECT 0.1\r\nTEMPO 90 3 8\u{2028}SAMPLERATE 48000\u{85}<TRACK \"{T 1}\"\u{0b}NAME 'a b'\u{0c}>\u{1c}>\r";
    let snapshot = rpp(text, &limits()).unwrap();
    assert_eq!(snapshot.transport_bpm, 90.0);
    assert_eq!(snapshot.transport_time_signature, (3, 8));
    assert_eq!(snapshot.sample_rate, Some(48000));
    assert_eq!(snapshot.tracks[0].track_id, "{T 1}");
    assert_eq!(snapshot.tracks[0].name, "a b");
    // An opaque payload line (key longer than 40 characters) is dropped.
    let payload = format!("<REAPER_PROJECT 0.1\n{} x\nTEMPO 100 4 4\n>\n", "K".repeat(41));
    assert_eq!(rpp(&payload, &limits()).unwrap().transport_bpm, 100.0);
}

// ---- Ardour ----

#[test]
fn ardour_refuses_gzip_and_non_sessions() {
    assert!(ardour::extract_ardour(&[0x1f, 0x8b, 8], &limits()).unwrap_err().contains("compressed"));
    assert!(ardour::extract_ardour(b"<Other/>", &limits()).is_err());
    assert!(ardour::extract_ardour(b"<Session/>", &limits()).is_ok());
    let session = |length: &str| {
        format!("<Session sample-rate='48000'><TempoMap superclocks-per-second='282240000'><Tempos><Tempo npm='120' note-type='4'/></Tempos></TempoMap><Playlists><Playlist id='1'><Region name='r' length='{length}'/></Playlist></Playlists><Routes><Route id='r' name='T' audio-playlist='1'/></Routes></Session>")
    };
    let project = ardour::extract_ardour(session("b3840@b1920").as_bytes(), &limits()).unwrap();
    assert_eq!((project.tracks[0].clips[0].length_beats, project.tracks[0].clips[0].position_beats), (2.0, 1.0));
    let project = ardour::extract_ardour(session("a282240000@a0").as_bytes(), &limits()).unwrap();
    assert_eq!(project.tracks[0].clips[0].length_beats, 2.0);
    let project = ardour::extract_ardour(session("garbage").as_bytes(), &limits()).unwrap();
    assert_eq!(project.tracks[0].clips[0].length_beats, 0.0);
}

// ---- Ableton .als ----

#[test]
fn als_decompression_and_dtd_caps() {
    let directory = tempfile::tempdir().unwrap();
    let gz = |name: &str, data: &[u8]| {
        let path = directory.path().join(name);
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(data).unwrap();
        std::fs::write(&path, encoder.finish().unwrap()).unwrap();
        path
    };
    let base = "<Ableton><LiveSet></LiveSet></Ableton>";
    let cap = Limits { max_als_decompressed_bytes: 100, ..limits() };
    boundary(100, "als decompressed", |size| {
        let padding = " ".repeat(size - base.len());
        let path = gz("size.als", format!("{base}{padding}").as_bytes());
        apw_daemon::project::als::extract_snapshot(&path, &cap)
    });
    let doctype = gz("dtd.als", b"<!DOCTYPE x [<!ENTITY e 'y'>]><Ableton><LiveSet/></Ableton>");
    assert!(apw_daemon::project::als::extract_snapshot(&doctype, &limits()).unwrap_err().contains("DTD"));
    let plain = directory.path().join("plain.als");
    std::fs::write(&plain, base).unwrap();
    assert!(apw_daemon::project::als::extract_snapshot(&plain, &limits()).is_err(), "not gzip");
    let no_live_set = gz("empty.als", b"<Ableton/>");
    assert!(apw_daemon::project::als::extract_snapshot(&no_live_set, &limits()).unwrap_err().contains("No LiveSet"));
    let garbage_tempo = gz("tempo.als", b"<Ableton><LiveSet><Transport><Tempo><Manual Value='fast'/></Tempo></Transport></LiveSet></Ableton>");
    assert!(apw_daemon::project::als::extract_snapshot(&garbage_tempo, &limits()).is_err());
    let _ = safe::Limits::default();
}

// ---- .xm / .mod / .vcv ----

fn fixture_bytes(relative: &str) -> Vec<u8> {
    std::fs::read(common::fixtures().join("projects").join(relative)).unwrap()
}

/// A frame of RLE blocks: 1 byte in, 128 KiB of zeros out, so a bomb is a few KiB.
fn rle_frame(blocks: usize) -> Vec<u8> {
    let mut out = vec![0x28, 0xb5, 0x2f, 0xfd, 0x00, 17 << 3];
    for index in 0..blocks {
        let header = ((1_u32 << 17) << 3) | (1 << 1) | u32::from(index == blocks - 1);
        out.extend(&header.to_le_bytes()[..3]);
        out.push(0);
    }
    out
}

#[test]
fn the_zstd_output_cap_has_the_right_boundary_at_the_default() {
    let cap = limits().max_tar_bytes;
    assert_eq!(cap, 2048 * (1 << 17));
    assert_eq!(tarzst::inflate_zstd(&rle_frame(2048), cap).unwrap().len(), cap);
    let error = tarzst::inflate_zstd(&rle_frame(2049), cap).unwrap_err();
    assert!(error.contains("decompresses past"), "{error}");
}

#[test]
fn tar_member_and_byte_caps_have_exact_boundaries() {
    let fixture = fixture_bytes("vcv/basic.vcv");
    let tar = tarzst::inflate_zstd(&fixture, limits().max_tar_bytes).unwrap();
    // "." "./modules" "./patch.json" "./modules/105" "./modules/105/wavetable.wav"
    for (limit, ok) in [(4, false), (5, true), (6, true)] {
        let result = tarzst::read_tar(&tar, &Limits { max_tar_members: limit, ..limits() });
        assert_eq!(result.is_ok(), ok, "members {limit}");
    }
    for (limit, ok) in [(tar.len() - 1, false), (tar.len(), true), (tar.len() + 1, true)] {
        assert_eq!(tarzst::inflate_zstd(&fixture, limit).is_ok(), ok, "bytes {limit}");
    }
    let members = tarzst::read_tar(&tar, &limits()).unwrap();
    assert_eq!(members.iter().map(|m| m.0.as_str()).collect::<Vec<_>>(), ["patch.json", "modules/105/wavetable.wav"]);
}

fn ustar(name: &str, size: usize, kind: u8) -> Vec<u8> {
    let mut block = vec![0_u8; 512];
    block[..name.len()].copy_from_slice(name.as_bytes());
    block[100..108].copy_from_slice(b"0000644\0");
    block[124..136].copy_from_slice(format!("{size:011o}\0").as_bytes());
    block[148..156].copy_from_slice(b"        ");
    block[156] = kind;
    block[257..265].copy_from_slice(b"ustar\x0000");
    let checksum: u32 = block.iter().map(|byte| u32::from(*byte)).sum();
    block[148..156].copy_from_slice(format!("{checksum:06o}\0 ").as_bytes());
    block
}

#[test]
fn unsafe_tar_entries_are_refused_and_nothing_is_written() {
    let directory = tempfile::tempdir().unwrap();
    let before = std::fs::read_dir(directory.path()).unwrap().count();
    for (name, kind) in [("../evil", b'0'), ("/evil", b'0'), ("a/../../evil", b'0'), ("link", b'2'), ("link", b'1'), ("dev", b'3'), ("fifo", b'6')] {
        let mut tar = ustar(name, 0, kind);
        tar.extend(vec![0_u8; 1024]);
        assert!(tarzst::read_tar(&tar, &limits()).is_err(), "{name} type {kind}");
    }
    let mut ok = ustar("./patch.json", 2, b'0');
    ok.extend(b"{}".iter().chain(vec![0_u8; 510].iter()));
    ok.extend(vec![0_u8; 1024]);
    assert_eq!(tarzst::read_tar(&ok, &limits()).unwrap().len(), 1);
    let mut bad_checksum = ok.clone();
    bad_checksum[0] = b'X';
    assert!(tarzst::read_tar(&bad_checksum, &limits()).unwrap_err().contains("checksum"));
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), before);
}

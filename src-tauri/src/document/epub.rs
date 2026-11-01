// EPUB document translation (Phase 3). Mirrors the PDF flow in `document/pdf.rs`:
// `extract_epub` returns one normalized text string per spine chapter (the EPUB
// analogue of `extract_pdf_pages`' per-page strings), so the existing renderer
// pipeline — dedup -> cache -> code/formula pass-through -> batched Gemini
// structured output via `translate_document` — and the bilingual viewer are
// reused unchanged. `assemble_epub` rebuilds the book, swapping each translatable
// text node for its translation (looked up from the frontend-provided map, keyed
// by the SAME normalized source text the chapters were built from), copying every
// other resource (images/CSS/fonts) verbatim, and writing `mimetype` first +
// stored per the OCF (EPUB container) spec.
//
// Granularity is per XML text node (not per block), so inline markup (<b>, <i>,
// <a>) round-trips perfectly. `script`/`style` text is never extracted; code and
// formula pass-through stays frontend-owned (`shouldPassThrough`), so code-like
// paragraphs simply never enter the translations map and are kept as-is.

use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};

use quick_xml::events::{BytesEnd, BytesRef, BytesStart, BytesText, Event};
use quick_xml::reader::Reader;
use quick_xml::writer::Writer;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

const CONTAINER_PATH: &str = "META-INF/container.xml";
const MIMETYPE: &str = "application/epub+zip";

/// Resolved zip-entry paths of the spine (content) documents, in reading order.
struct EpubLayout {
    spine: Vec<String>,
}

// ── shared text helpers ───────────────────────────────────────────────────--

/// Collapse every run of XML whitespace to a single space and trim. Shared by
/// extraction and reassembly so a given text node maps to one stable key.
fn normalize_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Resolve a single XML/HTML entity body (the part between `&` and `;`).
/// Handles numeric (`#NN`, `#xHH`) plus the common named entities that appear in
/// EPUB prose; unknown entities return `None` so the caller keeps them verbatim.
fn decode_entity(entity: &str) -> Option<String> {
    if let Some(num) = entity.strip_prefix('#') {
        let code = match num.strip_prefix('x').or_else(|| num.strip_prefix('X')) {
            Some(hex) => u32::from_str_radix(hex, 16).ok()?,
            None => num.parse::<u32>().ok()?,
        };
        return char::from_u32(code).map(|c| c.to_string());
    }
    let ch = match entity {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => '\u{a0}',
        "copy" => '©',
        "reg" => '®',
        "trade" => '™',
        "mdash" => '—',
        "ndash" => '–',
        "hellip" => '…',
        "ldquo" => '“',
        "rdquo" => '”',
        "lsquo" => '‘',
        "rsquo" => '’',
        "laquo" => '«',
        "raquo" => '»',
        "deg" => '°',
        "middot" => '·',
        "eacute" => 'é',
        "egrave" => 'è',
        "agrave" => 'à',
        "ccedil" => 'ç',
        "uuml" => 'ü',
        "ouml" => 'ö',
        "auml" => 'ä',
        "szlig" => 'ß',
        _ => return None,
    };
    Some(ch.to_string())
}

/// Resolve an entity-reference event to its text. quick-xml 0.40 emits `&...;`
/// as a separate `GeneralRef` event (not part of the surrounding text), so we
/// resolve numeric char refs and the common named entities ourselves; `None`
/// for an unknown named entity so the caller can keep it verbatim.
fn resolve_ref(r: &BytesRef) -> Option<String> {
    if let Ok(Some(c)) = r.resolve_char_ref() {
        return Some(c.to_string());
    }
    let name = r.decode().ok()?;
    decode_entity(&name)
}

// ── XML name / attribute helpers ──────────────────────────────────────────--

/// Case-insensitive compare of a (possibly prefixed) XML name's LOCAL part.
fn local_eq(qname: &[u8], target: &[u8]) -> bool {
    let local = match qname.iter().rposition(|&b| b == b':') {
        Some(i) => &qname[i + 1..],
        None => qname,
    };
    local.eq_ignore_ascii_case(target)
}

/// `script`/`style` contents must never be translated.
fn is_skip_name(qname: &[u8]) -> bool {
    local_eq(qname, b"script") || local_eq(qname, b"style")
}

/// Read an attribute value (matched by local name) off a start/empty element.
fn get_attr(e: &BytesStart, key: &[u8]) -> Option<String> {
    for attr in e.attributes().with_checks(false) {
        let a = match attr {
            Ok(a) => a,
            Err(_) => continue,
        };
        if local_eq(a.key.as_ref(), key) {
            return Some(String::from_utf8_lossy(a.value.as_ref()).into_owned());
        }
    }
    None
}

// ── path resolution ─────────────────────────────────────────────────────────

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Decode `%XX` escapes in an href (EPUB hrefs may percent-encode spaces etc.).
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2])) {
                out.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Collapse `.`/`..`/empty segments in a `/`-separated zip path.
fn normalize_path(p: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for seg in p.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            s => out.push(s),
        }
    }
    out.join("/")
}

fn parent_dir(p: &str) -> String {
    match p.rfind('/') {
        Some(i) => p[..i].to_string(),
        None => String::new(),
    }
}

/// Resolve a manifest href (relative to the OPF directory) to a zip-entry path.
fn resolve_href(base_dir: &str, href: &str) -> String {
    let no_fragment = href.split('#').next().unwrap_or(href);
    let decoded = percent_decode(no_fragment);
    let combined = if base_dir.is_empty() {
        decoded
    } else {
        format!("{base_dir}/{decoded}")
    };
    normalize_path(&combined)
}

// ── EPUB structure parsing ────────────────────────────────────────────────--

/// `META-INF/container.xml` -> path of the OPF package document.
fn parse_container(xml: &str) -> Option<String> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                if local_eq(e.name().as_ref(), b"rootfile") {
                    if let Some(full_path) = get_attr(&e, b"full-path") {
                        return Some(normalize_path(&percent_decode(&full_path)));
                    }
                }
            }
            Ok(Event::Eof) | Err(_) => return None,
            _ => {}
        }
    }
}

/// Parse the OPF: build a manifest id->href map and the ordered spine idrefs,
/// then resolve each spine entry to a zip-entry path (relative to the OPF dir).
fn parse_spine(opf_xml: &str, base_dir: &str) -> Vec<String> {
    let mut manifest: HashMap<String, String> = HashMap::new();
    let mut idrefs: Vec<String> = Vec::new();

    let mut reader = Reader::from_str(opf_xml);
    reader.config_mut().trim_text(true);
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
                let qname = e.name();
                if local_eq(qname.as_ref(), b"item") {
                    if let (Some(id), Some(href)) = (get_attr(&e, b"id"), get_attr(&e, b"href")) {
                        manifest.insert(id, href);
                    }
                } else if local_eq(qname.as_ref(), b"itemref") {
                    if let Some(idref) = get_attr(&e, b"idref") {
                        idrefs.push(idref);
                    }
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }

    idrefs
        .iter()
        .filter_map(|idref| manifest.get(idref))
        .map(|href| resolve_href(base_dir, href))
        .collect()
}

fn read_zip_entry(archive: &mut ZipArchive<std::fs::File>, name: &str) -> Option<Vec<u8>> {
    let mut file = archive.by_name(name).ok()?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf).ok()?;
    Some(buf)
}

fn read_layout(archive: &mut ZipArchive<std::fs::File>) -> Result<EpubLayout, String> {
    let container = read_zip_entry(archive, CONTAINER_PATH)
        .ok_or("EPUB is missing META-INF/container.xml")?;
    let opf_path = parse_container(&String::from_utf8_lossy(&container))
        .ok_or("Could not find the OPF rootfile in container.xml")?;
    let opf_bytes = read_zip_entry(archive, &opf_path)
        .ok_or_else(|| format!("OPF package not found at {opf_path}"))?;
    let base_dir = parent_dir(&opf_path);
    let spine = parse_spine(&String::from_utf8_lossy(&opf_bytes), &base_dir);
    if spine.is_empty() {
        return Err("EPUB spine is empty (no readable chapters).".into());
    }
    Ok(EpubLayout { spine })
}

// ── XHTML text extraction + reassembly ────────────────────────────────────--

/// Push the accumulated character run (text + resolved entities between two
/// element boundaries) as one normalized segment, then reset it.
fn push_run(segments: &mut Vec<String>, run: &mut String) {
    if !run.is_empty() {
        let cleaned = normalize_ws(run);
        if !cleaned.is_empty() {
            segments.push(cleaned);
        }
        run.clear();
    }
}

/// Ordered, normalized translatable text segments of one XHTML document. Adjacent
/// text and entity-reference events are merged into a single run (so e.g.
/// `Tom &amp; Jerry` is one segment), and runs are split at element boundaries so
/// inline markup keeps paragraphs together but separate blocks stay separate.
fn extract_segments(xhtml: &str) -> Vec<String> {
    let mut reader = Reader::from_str(xhtml);
    reader.config_mut().trim_text(false);
    let mut segments = Vec::new();
    let mut skip_depth = 0usize;
    let mut run = String::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                push_run(&mut segments, &mut run);
                if is_skip_name(e.name().as_ref()) {
                    skip_depth += 1;
                }
            }
            Ok(Event::End(e)) => {
                push_run(&mut segments, &mut run);
                if skip_depth > 0 && is_skip_name(e.name().as_ref()) {
                    skip_depth -= 1;
                }
            }
            Ok(Event::Text(e)) => {
                if skip_depth == 0 {
                    if let Ok(t) = e.decode() {
                        run.push_str(&t);
                    }
                }
            }
            Ok(Event::GeneralRef(r)) => {
                if skip_depth == 0 {
                    match resolve_ref(&r) {
                        Some(s) => run.push_str(&s),
                        None => {
                            if let Ok(name) = r.decode() {
                                run.push('&');
                                run.push_str(&name);
                                run.push(';');
                            }
                        }
                    }
                }
            }
            Ok(Event::Eof) | Err(_) => {
                push_run(&mut segments, &mut run);
                break;
            }
            _ => push_run(&mut segments, &mut run),
        }
    }
    segments
}

/// One buffered character-data event (text or entity reference) kept owned so a
/// whole run can be written back verbatim when it has no translation.
enum CharPiece {
    Text(BytesText<'static>),
    Ref(BytesRef<'static>),
}

/// Write the buffered original pieces (text + entity refs) back verbatim.
fn write_pieces(writer: &mut Writer<Vec<u8>>, pending: &mut Vec<(CharPiece, String)>) -> Option<()> {
    for (piece, _) in pending.drain(..) {
        match piece {
            CharPiece::Text(t) => writer.write_event(Event::Text(t)).ok()?,
            CharPiece::Ref(r) => writer.write_event(Event::GeneralRef(r)).ok()?,
        }
    }
    Some(())
}

/// Emit the buffered run. With a translation present: translated-only mode writes a
/// single replacement node (preserving the run's leading/trailing whitespace);
/// bilingual mode keeps the original run and appends the translation as a sibling
/// node (`<br/>` + a classed `<span>`) so both languages show per block. With no
/// translation the original pieces are written back unchanged so structure/entities
/// are preserved.
fn flush_run(
    writer: &mut Writer<Vec<u8>>,
    pending: &mut Vec<(CharPiece, String)>,
    translations: &HashMap<String, String>,
    bilingual: bool,
) -> Option<()> {
    if pending.is_empty() {
        return Some(());
    }
    let merged: String = pending.iter().map(|(_, d)| d.as_str()).collect();
    let key = normalize_ws(&merged);
    if !key.is_empty() {
        if let Some(tr) = translations.get(&key) {
            if bilingual {
                write_pieces(writer, pending)?;
                writer
                    .write_event(Event::Empty(BytesStart::new("br")))
                    .ok()?;
                let mut span = BytesStart::new("span");
                span.push_attribute(("class", "ctranslate-translation"));
                writer.write_event(Event::Start(span)).ok()?;
                writer
                    .write_event(Event::Text(BytesText::new(tr.as_str())))
                    .ok()?;
                writer
                    .write_event(Event::End(BytesEnd::new("span")))
                    .ok()?;
                return Some(());
            }
            let lead = &merged[..merged.len() - merged.trim_start().len()];
            let trail = &merged[merged.trim_end().len()..];
            let replacement = format!("{lead}{tr}{trail}");
            writer
                .write_event(Event::Text(BytesText::new(&replacement)))
                .ok()?;
            pending.clear();
            return Some(());
        }
    }
    write_pieces(writer, pending)
}

/// Re-serialize an XHTML document, replacing each translatable text run with its
/// translation. Returns `None` on a parse error so the caller keeps the original
/// bytes rather than emit garbage.
fn transform_xhtml(
    xhtml: &str,
    translations: &HashMap<String, String>,
    bilingual: bool,
) -> Option<Vec<u8>> {
    let mut reader = Reader::from_str(xhtml);
    reader.config_mut().trim_text(false);
    let mut writer = Writer::new(Vec::new());
    let mut skip_depth = 0usize;
    let mut pending: Vec<(CharPiece, String)> = Vec::new();

    loop {
        match reader.read_event() {
            Ok(Event::Eof) => {
                flush_run(&mut writer, &mut pending, translations, bilingual)?;
                break;
            }
            Ok(Event::Start(e)) => {
                flush_run(&mut writer, &mut pending, translations, bilingual)?;
                if is_skip_name(e.name().as_ref()) {
                    skip_depth += 1;
                }
                writer.write_event(Event::Start(e.into_owned())).ok()?;
            }
            Ok(Event::End(e)) => {
                flush_run(&mut writer, &mut pending, translations, bilingual)?;
                if skip_depth > 0 && is_skip_name(e.name().as_ref()) {
                    skip_depth -= 1;
                }
                writer.write_event(Event::End(e.into_owned())).ok()?;
            }
            Ok(Event::Text(e)) => {
                if skip_depth == 0 {
                    let decoded = e.decode().map(|c| c.into_owned()).unwrap_or_default();
                    pending.push((CharPiece::Text(e.into_owned()), decoded));
                } else {
                    writer.write_event(Event::Text(e.into_owned())).ok()?;
                }
            }
            Ok(Event::GeneralRef(r)) => {
                if skip_depth == 0 {
                    let decoded = resolve_ref(&r)
                        .unwrap_or_else(|| r.decode().map(|n| format!("&{n};")).unwrap_or_default());
                    pending.push((CharPiece::Ref(r.into_owned()), decoded));
                } else {
                    writer.write_event(Event::GeneralRef(r.into_owned())).ok()?;
                }
            }
            Ok(ev) => {
                flush_run(&mut writer, &mut pending, translations, bilingual)?;
                writer.write_event(ev.into_owned()).ok()?;
            }
            Err(_) => return None,
        }
    }
    Some(writer.into_inner())
}

// ── Tauri commands ────────────────────────────────────────────────────────--

/// Extract translatable text per spine chapter, in reading order. Each returned
/// string is the chapter's normalized paragraphs joined by blank lines, matching
/// the per-page shape of `extract_pdf_pages` so the renderer pipeline is reused.
#[tauri::command]
pub fn extract_epub(path: String) -> Result<Vec<String>, String> {
    let file = std::fs::File::open(&path).map_err(|e| format!("Cannot open EPUB: {e}"))?;
    let mut archive =
        ZipArchive::new(file).map_err(|e| format!("Invalid EPUB (zip) archive: {e}"))?;
    let layout = read_layout(&mut archive)?;

    let mut chapters = Vec::with_capacity(layout.spine.len());
    for href in &layout.spine {
        // A spine file missing from the zip is skipped, not fatal for the book.
        if let Some(bytes) = read_zip_entry(&mut archive, href) {
            let segments = extract_segments(&String::from_utf8_lossy(&bytes));
            chapters.push(segments.join("\n\n"));
        }
    }
    if chapters.is_empty() {
        return Err("No readable spine documents found in EPUB.".into());
    }
    Ok(chapters)
}

/// Rebuild the EPUB at `path` into `output_path`, copying all non-spine resources
/// verbatim and writing `mimetype` first + stored. `translations` maps normalized
/// source paragraph -> translation; missing entries (e.g. pass-through code or
/// untranslated paragraphs) keep the original text. When `bilingual` is true the
/// original text is kept and the translation is appended per block; otherwise each
/// translated node is replaced (translated-only output).
#[tauri::command]
pub fn assemble_epub(
    path: String,
    output_path: String,
    translations: HashMap<String, String>,
    bilingual: bool,
) -> Result<(), String> {
    let file = std::fs::File::open(&path).map_err(|e| format!("Cannot open EPUB: {e}"))?;
    let mut archive =
        ZipArchive::new(file).map_err(|e| format!("Invalid EPUB (zip) archive: {e}"))?;
    let spine: HashSet<String> = read_layout(&mut archive)?.spine.into_iter().collect();

    // Read every entry up front (EPUBs are small); we re-emit them in order,
    // skipping directories and the original mimetype (we write our own first).
    let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
    for i in 0..archive.len() {
        let mut f = archive.by_index(i).map_err(|e| e.to_string())?;
        let name = f.name().to_string();
        if name.ends_with('/') || name == "mimetype" {
            continue;
        }
        let mut buf = Vec::new();
        f.read_to_end(&mut buf).map_err(|e| e.to_string())?;
        entries.push((name, buf));
    }

    let out =
        std::fs::File::create(&output_path).map_err(|e| format!("Cannot create output EPUB: {e}"))?;
    let mut zip = ZipWriter::new(out);

    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    zip.start_file("mimetype", stored).map_err(|e| e.to_string())?;
    zip.write_all(MIMETYPE.as_bytes()).map_err(|e| e.to_string())?;

    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    for (name, bytes) in entries {
        let out_bytes = if spine.contains(&name) {
            transform_xhtml(&String::from_utf8_lossy(&bytes), &translations, bilingual)
                .unwrap_or(bytes)
        } else {
            bytes
        };
        zip.start_file(name, deflated).map_err(|e| e.to_string())?;
        zip.write_all(&out_bytes).map_err(|e| e.to_string())?;
    }
    zip.finish().map_err(|e| e.to_string())?;
    Ok(())
}

// ── tests ──────────────────────────────────────────────────────────────────-

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    const CONTAINER_SAMPLE: &str = r#"<?xml version="1.0"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles>
    <rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/>
  </rootfiles>
</container>"#;

    const OPF_SAMPLE: &str = r#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="id">
  <manifest>
    <item id="css" href="style.css" media-type="text/css"/>
    <item id="c2" href="ch2.xhtml" media-type="application/xhtml+xml"/>
    <item id="c1" href="ch1.xhtml" media-type="application/xhtml+xml"/>
  </manifest>
  <spine>
    <itemref idref="c1"/>
    <itemref idref="c2"/>
  </spine>
</package>"#;

    const XHTML_SAMPLE: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<html xmlns="http://www.w3.org/1999/xhtml"><head><title>Title</title><style>.x{color:red}</style></head><body><p>Hello <b>World</b></p><script>var x=1;</script><p>Tom &amp; Jerry</p></body></html>"#;

    #[test]
    fn parse_container_finds_opf() {
        assert_eq!(parse_container(CONTAINER_SAMPLE).as_deref(), Some("OEBPS/content.opf"));
    }

    #[test]
    fn parse_spine_orders_by_itemref_not_manifest() {
        let spine = parse_spine(OPF_SAMPLE, "OEBPS");
        assert_eq!(spine, vec!["OEBPS/ch1.xhtml", "OEBPS/ch2.xhtml"]);
    }

    #[test]
    fn resolve_href_handles_relative_and_percent() {
        assert_eq!(resolve_href("OEBPS/Text", "../Images/a%20b.png"), "OEBPS/Images/a b.png");
        assert_eq!(resolve_href("", "./ch1.xhtml#frag"), "ch1.xhtml");
    }

    #[test]
    fn decode_entity_named_and_numeric() {
        assert_eq!(decode_entity("amp").as_deref(), Some("&"));
        assert_eq!(decode_entity("#169").as_deref(), Some("©"));
        assert_eq!(decode_entity("#xA9").as_deref(), Some("©"));
        assert_eq!(decode_entity("nbsp").as_deref(), Some("\u{a0}"));
        assert_eq!(decode_entity("unknown"), None);
    }

    #[test]
    fn segments_skip_script_style_and_normalize() {
        let segs = extract_segments(XHTML_SAMPLE);
        assert_eq!(segs, vec!["Title", "Hello", "World", "Tom & Jerry"]);
    }

    #[test]
    fn transform_replaces_known_segments_and_preserves_inline_tags() {
        let mut t = HashMap::new();
        t.insert("Hello".to_string(), "Merhaba".to_string());
        t.insert("World".to_string(), "Dünya".to_string());
        let out = transform_xhtml(XHTML_SAMPLE, &t, false).expect("parse ok");
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("Merhaba <b>Dünya</b>"), "inline structure + translations: {s}");
        assert!(s.contains("Tom &amp; Jerry"), "untranslated node kept verbatim: {s}");
        assert!(s.contains("var x=1;"), "script content untouched: {s}");
    }

    #[test]
    fn transform_bilingual_keeps_original_and_appends_translation() {
        let mut t = HashMap::new();
        t.insert("Hello".to_string(), "Merhaba".to_string());
        let out = transform_xhtml(XHTML_SAMPLE, &t, true).expect("parse ok");
        let s = String::from_utf8(out).unwrap();
        // Original is kept (translated-only mode would have dropped "Hello")...
        assert!(s.contains("Hello"), "original kept in bilingual mode: {s}");
        // ...and the translation is appended as a classed sibling node, not a replacement.
        assert!(s.contains("ctranslate-translation"), "translation in sibling span: {s}");
        assert!(s.contains("Merhaba"), "translation text present: {s}");
    }

    fn build_minimal_epub() -> Vec<u8> {
        let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
        let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        zip.start_file("mimetype", stored).unwrap();
        zip.write_all(MIMETYPE.as_bytes()).unwrap();
        let def = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        for (name, body) in [
            ("META-INF/container.xml", CONTAINER_SAMPLE),
            ("OEBPS/content.opf", OPF_SAMPLE),
            ("OEBPS/ch1.xhtml", XHTML_SAMPLE),
            ("OEBPS/ch2.xhtml", XHTML_SAMPLE),
        ] {
            zip.start_file(name, def).unwrap();
            zip.write_all(body.as_bytes()).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    #[test]
    fn extract_and_assemble_roundtrip_through_zip() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir();
        let src = dir.join(format!("ctranslate_epub_src_{nanos}.epub"));
        let dst = dir.join(format!("ctranslate_epub_dst_{nanos}.epub"));
        std::fs::write(&src, build_minimal_epub()).unwrap();

        let chapters = extract_epub(src.to_string_lossy().to_string()).unwrap();
        assert_eq!(chapters.len(), 2, "two spine chapters");
        assert_eq!(chapters[0], "Title\n\nHello\n\nWorld\n\nTom & Jerry");

        let mut t = HashMap::new();
        t.insert("Hello".to_string(), "Merhaba".to_string());
        assert!(assemble_epub(
            src.to_string_lossy().to_string(),
            dst.to_string_lossy().to_string(),
            t,
            false,
        )
        .is_ok());

        // Re-open the produced EPUB: mimetype must be first + stored, and the
        // chapter must contain the translated text.
        let f = std::fs::File::open(&dst).unwrap();
        let mut out_zip = ZipArchive::new(f).unwrap();
        {
            let first = out_zip.by_index(0).unwrap();
            assert_eq!(first.name(), "mimetype");
            assert_eq!(first.compression(), CompressionMethod::Stored);
        }
        let mut ch1 = String::new();
        out_zip
            .by_name("OEBPS/ch1.xhtml")
            .unwrap()
            .read_to_string(&mut ch1)
            .unwrap();
        assert!(ch1.contains("Merhaba"), "translated text present: {ch1}");

        let _ = std::fs::remove_file(&src);
        let _ = std::fs::remove_file(&dst);
    }
}

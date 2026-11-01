//! Lossless inspection of source page text operators.
use lopdf::{content::Content, Document, Object, ObjectId};
use pdfium_render::prelude::*;
use std::collections::HashSet;

fn text_inventory(doc: &Document, page: ObjectId) -> Result<Vec<(usize, String)>, String> {
    let fonts = doc.get_page_fonts(page).map_err(|e| e.to_string())?;
    let content = Content::decode(&doc.get_page_content(page).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    let mut font = None;
    let mut stack = Vec::new();
    let mut result = Vec::new();
    for (index, op) in content.operations.iter().enumerate() {
        match op.operator.as_str() {
            "q" => stack.push(font.clone()),
            "Q" => font = stack.pop().flatten(),
            "Tf" => font = op.operands.first().and_then(|o| o.as_name().ok()).map(|s| s.to_vec()),
            "Tj" | "TJ" | "'" | "\"" => {
                let name = font.as_ref().ok_or("Text operator has no active font")?;
                let dictionary = fonts.get(name).ok_or("Text font is not in page resources")?;
                let encoding = dictionary.get_font_encoding(doc).map_err(|e| e.to_string())?;
                let mut text = String::new();
                let parts: Vec<&Object> = if op.operator == "TJ" {
                    op.operands.first().ok_or("Missing TJ operand")?.as_array().map_err(|e| e.to_string())?.iter().collect()
                } else {
                    vec![op.operands.last().ok_or("Missing text operand")?]
                };
                for part in parts {
                    if let Object::String(bytes, _) = part {
                        text.push_str(&Document::decode_text(&encoding, bytes).map_err(|e| e.to_string())?);
                    }
                }
                if !text.trim().is_empty() { result.push((index, text)); }
            }
            _ => {}
        }
    }
    Ok(result)
}

/// Hide only verified text-show operations. Rendering mode 3 retains all text
/// advances, so subsequent operators and their geometry remain untouched.
fn hidden_content(doc: &Document, page_id: ObjectId, page: &PdfPage, selected: &HashSet<usize>) -> Result<Vec<u8>, String> {
    let inventory = text_inventory(doc, page_id)?;
    let text_page = page.text().map_err(|e| e.to_string())?;
    let objects: Vec<_> = page.objects().iter().enumerate().filter_map(|(i, o)| {
        o.as_text_object().map(|t| (i, text_page.for_object(t)))
    }).filter(|(_, s)| !s.trim().is_empty()).collect();
    let normalize = |s: &str| s.chars().filter(|c| !c.is_whitespace()).collect::<String>();
    if inventory.len() != objects.len() || inventory.iter().zip(&objects).any(|(a, b)| normalize(&a.1) != normalize(&b.1)) {
        return Err("Cannot safely match source text operators to page objects".into());
    }
    let mut targets = HashSet::new();
    for ((op, _), (index, _)) in inventory.iter().zip(&objects) {
        if selected.contains(index) { targets.insert(*op); }
    }
    if targets.len() != selected.len() { return Err("Some translated text objects could not be matched".into()); }
    let bytes = doc.get_page_content(page_id).map_err(|e| e.to_string())?;
    let content = Content::decode(&bytes).map_err(|e| e.to_string())?;
    let spans = operation_spans(&bytes, &content)?;
    let mut inserts = Vec::new();
    let mut mode = 0_i64;
    let mut stack = Vec::new();
    for (i, op) in content.operations.iter().enumerate() {
        match op.operator.as_str() {
            "q" => stack.push(mode),
            "Q" => mode = stack.pop().ok_or("Unbalanced PDF graphics state")?,
            "Tr" => mode = op.operands.first().and_then(|o| o.as_i64().ok()).ok_or("Invalid text rendering mode")?,
            _ => {}
        }
        if targets.contains(&i) {
            if mode >= 4 { return Err("Cannot hide text that contributes to a clipping path".into()); }
            inserts.push((spans[i].0, b"\n3 Tr\n".to_vec()));
            inserts.push((spans[i].1, format!("\n{mode} Tr\n").into_bytes()));
        }
    }
    if !stack.is_empty() { return Err("Unbalanced PDF graphics state".into()); }
    let mut output = b"q\n".to_vec();
    let mut cursor = 0;
    for (at, insert) in inserts {
        output.extend_from_slice(&bytes[cursor..at]);
        output.extend(insert);
        cursor = at;
    }
    output.extend_from_slice(&bytes[cursor..]);
    output.extend_from_slice(b"\nQ\n");
    Ok(output)
}

// Scan top-level tokens solely to locate original operator spans. The lopdf
// parser independently validates the operator sequence before any insertion.
// Inline image streams are rejected because their binary data is not tokenized.
fn operation_spans(bytes: &[u8], content: &Content) -> Result<Vec<(usize, usize)>, String> {
    fn ws(c: u8) -> bool { matches!(c, 0 | 9 | 10 | 12 | 13 | 32) }
    fn delimiter(c: u8) -> bool { ws(c) || b"()<>[]{}/%".contains(&c) }
    let mut i = 0;
    let mut start = 0;
    let mut depth = 0_i32;
    let mut spans = Vec::new();
    while i < bytes.len() {
        let c = bytes[i];
        if ws(c) { i += 1; continue; }
        if c == b'%' { while i < bytes.len() && !matches!(bytes[i], b'\r' | b'\n') { i += 1; } continue; }
        if c == b'(' {
            i += 1; let mut nesting = 1;
            while i < bytes.len() && nesting > 0 {
                match bytes[i] {
                    b'\\' => { i += 1; if i < bytes.len() && bytes[i] == b'\r' && bytes.get(i+1) == Some(&b'\n') { i += 1; } },
                    b'(' => nesting += 1,
                    b')' => nesting -= 1,
                    _ => {}
                }
                i += 1;
            }
            if nesting != 0 { return Err("Unterminated PDF string".into()); }
            continue;
        }
        if c == b'<' && bytes.get(i+1) != Some(&b'<') {
            i += 1; while i < bytes.len() && bytes[i] != b'>' { i += 1; }
            if i == bytes.len() { return Err("Unterminated PDF hex string".into()); }
            i += 1; continue;
        }
        if c == b'[' || (c == b'<' && bytes.get(i+1) == Some(&b'<')) {
            depth += 1; i += if c == b'<' { 2 } else { 1 }; continue;
        }
        if c == b']' || (c == b'>' && bytes.get(i+1) == Some(&b'>')) {
            depth -= 1; if depth < 0 { return Err("Unbalanced PDF container".into()); }
            i += if c == b'>' { 2 } else { 1 }; continue;
        }
        if c == b'/' { i += 1; while i < bytes.len() && !delimiter(bytes[i]) { i += 1; } continue; }
        if delimiter(c) { return Err("Unsupported PDF content token".into()); }
        let token_start = i;
        while i < bytes.len() && !delimiter(bytes[i]) { i += 1; }
        if depth != 0 { continue; }
        let token = &bytes[token_start..i];
        if token.first().is_some_and(|c| c.is_ascii_digit() || matches!(*c, b'+' | b'-' | b'.')) || [b"true".as_slice(), b"false", b"null"].contains(&token) { continue; }
        let expected = content.operations.get(spans.len()).ok_or("Unexpected PDF operator")?;
        if token != expected.operator.as_bytes() || token == b"BI" { return Err("Unsupported PDF operator stream".into()); }
        spans.push((start, i));
        start = i;
    }
    if depth != 0 || spans.len() != content.operations.len() { return Err("Incomplete PDF operator stream".into()); }
    Ok(spans)
}

pub(crate) fn page_content(doc: &Document, page_id: ObjectId, page: &PdfPage, selected: &HashSet<usize>) -> Result<Vec<u8>, String> {
    hidden_content(doc, page_id, page, selected)
}

/// Attach the separate text-only PDF pages as isolated Form XObjects; every
/// original graphics operator and original resource remains in the source PDF.
pub(crate) fn combine(mut source: Document, overlay_bytes: &[u8], contents: Vec<Vec<u8>>) -> Result<Vec<u8>, String> {
    use lopdf::{Dictionary, Stream, content::Operation};
    let mut overlay = Document::load_mem(overlay_bytes).map_err(|e| e.to_string())?;
    super::unicode_text::restore(&mut overlay)?;
    overlay.renumber_objects_with(source.max_id + 1);
    let source_pages: Vec<_> = source.get_pages().values().copied().collect();
    let overlay_pages: Vec<_> = overlay.get_pages().values().copied().collect();
    if source_pages.len() != overlay_pages.len() || source_pages.len() != contents.len() {
        return Err("Overlay page count mismatch".into());
    }
    let mut overlay_data = Vec::new();
    for id in overlay_pages {
        let resources = super::image_resources::inherited_resources(&overlay, id)?;
        let page = overlay.get_dictionary(id).map_err(|e| e.to_string())?;
        let bbox = page.get(b"MediaBox").map_err(|e| e.to_string())?.clone();
        let bytes = overlay.get_page_content(id).map_err(|e| e.to_string())?;
        overlay_data.push((resources, bbox, bytes));
    }
    source.max_id = overlay.max_id;
    source.objects.extend(overlay.objects);
    for ((id, mut original), (overlay_resources, bbox, overlay_content)) in source_pages.into_iter().zip(contents).zip(overlay_data) {
        let mut form = Dictionary::new();
        form.set("Type", Object::Name(b"XObject".to_vec()));
        form.set("Subtype", Object::Name(b"Form".to_vec()));
        form.set("BBox", bbox);
        form.set("Resources", Object::Dictionary(overlay_resources));
        let form_id = source.add_object(Stream::new(form, overlay_content));
        let mut resources = super::image_resources::inherited_resources(&source, id)?;
        let mut xobjects = match resources.get(b"XObject") {
            Ok(value) => source.dereference(value).and_then(|(_, o)| o.as_dict()).map_err(|e| e.to_string())?.clone(),
            Err(_) => Dictionary::new(),
        };
        let mut name = b"CTranslateOverlay".to_vec();
        while xobjects.has(&name) { name.push(b'_'); }
        xobjects.set(name.clone(), Object::Reference(form_id));
        resources.set("XObject", Object::Dictionary(xobjects));
        original.push(b'\n');
        original.extend(Content { operations: vec![Operation::new("q", vec![]), Operation::new("Do", vec![Object::Name(name)]), Operation::new("Q", vec![])] }.encode().map_err(|e| e.to_string())?);
        let stream = source.add_object(Stream::new(Dictionary::new(), original));
        let page = source.get_dictionary_mut(id).map_err(|e| e.to_string())?;
        page.set("Resources", Object::Dictionary(resources));
        page.set("Contents", Object::Reference(stream));
    }
    let mut out = Vec::new();
    source.save_to(&mut out).map_err(|e| e.to_string())?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn raw_operator_spans_preserve_numbers_and_nested_strings() {
        let bytes = b"0.23999999 0 0 -0.23999999 0 841.91998 cm\nBT /F1 12 Tf [(a\\(b\\)) 10 <0123>] TJ ET";
        let content = Content::decode(bytes).unwrap();
        let spans = operation_spans(bytes, &content).unwrap();
        assert_eq!(spans.len(), 5);
        assert_eq!(&bytes[spans[0].0..spans[0].1], b"0.23999999 0 0 -0.23999999 0 841.91998 cm");
        assert!(std::str::from_utf8(&bytes[spans[3].0..spans[3].1]).unwrap().contains("TJ"));
    }
    #[test]
    #[ignore = "Requires CTRANSLATE_TEST_PDF and CTRANSLATE_PDFIUM"]
    fn audit_text_inventory() {
        let path = std::env::var("CTRANSLATE_TEST_PDF").unwrap();
        let lib = std::env::var("CTRANSLATE_PDFIUM").unwrap();
        let doc = Document::load(&path).unwrap();
        let pdfium = Pdfium::new(Pdfium::bind_to_library(lib).unwrap());
        let pdf = pdfium.load_pdf_from_file(&path, None).unwrap();
        for (i, (_, id)) in doc.get_pages().iter().enumerate() {
            let ops = text_inventory(&doc, *id).unwrap();
            let page = pdf.pages().get(i as u16).unwrap();
            let text = page.text().unwrap();
            let objects: Vec<_> = page.objects().iter().filter_map(|o| o.as_text_object().map(|t| text.for_object(t))).filter(|s| !s.trim().is_empty()).collect();
            let normalize = |s: &str| s.chars().filter(|c| !c.is_whitespace()).collect::<String>();
            let mismatches: Vec<_> = ops.iter().zip(&objects).enumerate().filter(|(_, (a,b))| normalize(&a.1) != normalize(b)).take(5).collect();
            eprintln!("page {}: ops {}, objects {}, mismatches {:?}", i+1, ops.len(), objects.len(), mismatches);
        }
    }
}

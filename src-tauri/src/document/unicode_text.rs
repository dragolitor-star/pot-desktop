//! PDFium 7543 SetText splits supplementary UTF-16 pairs. Transport those runs
//! through ASCII, then encode their real CIDs using the embedded ToUnicode map.
//! Glyphs, metrics, styles and copyable Unicode remain unchanged.
use lopdf::{content::Content, Document, Encoding, Object, ObjectId, Stream};
use std::collections::HashMap;

const PREFIX: &str = "CTRANSLATEUNI_";
const SUFFIX: &str = "_END";

pub fn transport(text: &str) -> String {
    if !text.chars().any(|c| c as u32 > 0xffff) {
        return text.to_string();
    }
    format!(
        "{PREFIX}{}{SUFFIX}",
        text.chars()
            .map(|c| format!("{:06X}", c as u32))
            .collect::<Vec<_>>()
            .join("_")
    )
}

fn original(text: &str) -> Result<Option<String>, String> {
    let Some(body) = text.strip_prefix(PREFIX) else {
        return Ok(None);
    };
    let body = body
        .strip_suffix(SUFFIX)
        .ok_or("Incomplete Unicode transport marker")?;
    let result = body
        .split('_')
        .map(|code| {
            u32::from_str_radix(code, 16)
                .ok()
                .and_then(char::from_u32)
                .ok_or_else(|| "Invalid scalar in Unicode transport marker".to_string())
        })
        .collect::<Result<String, _>>()?;
    if !result.chars().any(|c| c as u32 > 0xffff) {
        return Err("Unexpected BMP-only Unicode transport marker".into());
    }
    Ok(Some(result))
}

pub fn restore(document: &mut Document) -> Result<usize, String> {
    let mut encodings: HashMap<ObjectId, Encoding<'static>> = HashMap::new();
    let mut updates = Vec::new();
    let mut restored = 0;
    for (_, page) in document.get_pages() {
        let bytes = document.get_page_content(page).map_err(|e| e.to_string())?;
        let mut content = Content::decode(&bytes).map_err(|e| e.to_string())?;
        let fonts = document.get_page_fonts(page).map_err(|e| e.to_string())?;
        let mut names = HashMap::new();
        for (name, font) in fonts {
            let Ok(id) = font.get(b"ToUnicode").and_then(Object::as_reference) else {
                continue;
            };
            if !encodings.contains_key(&id) {
                if let Ok(Encoding::UnicodeMapEncoding(map)) = font.get_font_encoding(document) {
                    encodings.insert(id, Encoding::UnicodeMapEncoding(map));
                }
            }
            names.insert(name, id);
        }
        let mut active = None;
        let mut stack = Vec::new();
        let mut changed = false;
        for operation in &mut content.operations {
            match operation.operator.as_str() {
                "q" => stack.push(active),
                "Q" => active = stack.pop().flatten(),
                "Tf" => {
                    active = operation
                        .operands
                        .first()
                        .and_then(|v| v.as_name().ok())
                        .and_then(|name| names.get(name))
                        .copied()
                }
                "Tj" => {
                    let Some(encoding) = active.and_then(|id| encodings.get(&id)) else {
                        continue;
                    };
                    let Some(Object::String(bytes, _)) = operation.operands.first_mut() else {
                        continue;
                    };
                    let text = encoding.bytes_to_string(bytes).map_err(|e| e.to_string())?;
                    let Some(text) = original(&text)? else {
                        continue;
                    };
                    let actual = encoding.string_to_bytes(&text);
                    if encoding
                        .bytes_to_string(&actual)
                        .map_err(|e| e.to_string())?
                        != text
                    {
                        return Err(
                            "Embedded font cannot encode complete supplementary text".into()
                        );
                    }
                    *bytes = actual;
                    changed = true;
                    restored += 1;
                }
                _ => {}
            }
        }
        if changed {
            updates.push((page, content.encode().map_err(|e| e.to_string())?));
        }
    }
    for (page, bytes) in updates {
        let stream = document.add_object(Stream::new(lopdf::Dictionary::new(), bytes));
        document
            .get_object_mut(page)
            .and_then(Object::as_dict_mut)
            .map_err(|e| e.to_string())?
            .set("Contents", Object::Reference(stream));
    }
    Ok(restored)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn transport_preserves_all_scalars_and_rejects_incomplete_markers() {
        let text = "∆𝒂 + ∆𝒃 = 𝒄";
        assert_eq!(original(&transport(text)).unwrap().as_deref(), Some(text));
        assert_eq!(transport("Türkçe Δa"), "Türkçe Δa");
        assert!(original("CTRANSLATEUNI_01D482").is_err());
        assert!(original("CTRANSLATEUNI_110000_END").is_err());
    }

    #[test]
    #[ignore = "Requires CTRANSLATE_PDFIUM and CTRANSLATE_AUDIT_DIR; real font roundtrip"]
    fn native_supplementary_math_roundtrip() {
        use pdfium_render::prelude::*;
        let pdfium = Pdfium::new(
            Pdfium::bind_to_library(std::env::var("CTRANSLATE_PDFIUM").unwrap()).unwrap(),
        );
        let mut doc = pdfium.create_new_pdf().unwrap();
        let font = doc
            .fonts_mut()
            .load_true_type_from_file("C:/Windows/Fonts/seguisym.ttf", true)
            .unwrap();
        let text = "∆𝒂 + ∆𝒃 = 𝒄";
        let mut page = doc
            .pages_mut()
            .create_page_at_end(PdfPagePaperSize::a4())
            .unwrap();
        page.objects_mut()
            .create_text_object(
                PdfPoints::new(40.0),
                PdfPoints::new(700.0),
                transport(text),
                font,
                PdfPoints::new(24.0),
            )
            .unwrap();
        drop(page);
        let path = std::path::PathBuf::from(std::env::var("CTRANSLATE_AUDIT_DIR").unwrap())
            .join("supplementary-math.pdf");
        doc.save_to_file(&path).unwrap();
        drop(doc);
        let mut doc = Document::load(&path).unwrap();
        assert_eq!(restore(&mut doc).unwrap(), 1);
        doc.save(&path).unwrap();
        let doc = pdfium.load_pdf_from_file(&path, None).unwrap();
        let page = doc.pages().get(0).unwrap();
        assert_eq!(page.text().unwrap().all().trim(), text);
        page.render_with_config(&PdfRenderConfig::new().set_target_width(900))
            .unwrap()
            .as_image()
            .save(path.with_extension("png"))
            .unwrap();
    }
}

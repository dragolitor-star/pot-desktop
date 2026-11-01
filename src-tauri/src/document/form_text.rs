//! Surgical text removal inside vector Forms. Paths, images and graphics state
//! operators are retained; translated labels are drawn in page coordinates.
use lopdf::{content::Content, Document, Object};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FormTextRef {
    pub signature: String,
    pub text_index: usize,
    pub source: String,
}

pub fn normalized(text: &str) -> String {
    text.chars().filter(|c| !c.is_whitespace()).collect()
}
pub fn signature(texts: &[String]) -> String {
    let mut hash = Sha256::new();
    for text in texts {
        let text = normalized(text);
        hash.update((text.len() as u64).to_le_bytes());
        hash.update(text.as_bytes());
    }
    format!("{:x}", hash.finalize())
}

/// Match the COMPLETE ordered text inventory before changing any operator.
/// Ambiguous/unsupported encodings fail the export, never erase guessed text.
pub fn remove_translated_form_text(path: &Path, refs: &[FormTextRef]) -> Result<(), String> {
    let mut targets: HashMap<String, HashMap<usize, String>> = HashMap::new();
    for r in refs {
        targets
            .entry(r.signature.clone())
            .or_default()
            .insert(r.text_index, normalized(&r.source));
    }
    #[cfg(test)]
    eprintln!("Form cleanup: loading document");
    let mut doc = Document::load(path).map_err(|e| format!("Read PDF vector Forms: {e}"))?;
    crate::document::unicode_text::restore(&mut doc)?;
    #[cfg(test)]
    eprintln!("Form cleanup: document loaded");
    let ids: Vec<_> = doc
        .objects
        .iter()
        .filter_map(|(id, object)| {
            object
                .as_stream()
                .ok()
                .filter(|s| s.dict.get(b"Subtype").and_then(Object::as_name).ok() == Some(b"Form"))
                .map(|_| *id)
        })
        .collect();
    let mut found = HashSet::new();
    for id in ids {
        #[cfg(test)]
        eprintln!("Form cleanup: processing {id:?}");
        let stream = doc
            .get_object(id)
            .and_then(Object::as_stream)
            .map_err(|e| e.to_string())?;
        let decoded = stream
            .decompressed_content()
            .unwrap_or_else(|_| stream.content.clone());
        let mut content = Content::decode(&decoded).map_err(|e| e.to_string())?;
        #[cfg(test)]
        eprintln!(
            "Form cleanup: decoded {} operations",
            content.operations.len()
        );
        let resources = match stream
            .dict
            .get(b"Resources")
            .and_then(|r| doc.dereference(r).map(|r| r.1))
            .and_then(Object::as_dict)
        {
            Ok(r) => r,
            Err(_) => continue,
        };
        let fonts = match resources
            .get(b"Font")
            .and_then(|r| doc.dereference(r).map(|r| r.1))
            .and_then(Object::as_dict)
        {
            Ok(r) => r,
            Err(_) => continue,
        };
        let mut font_name: Option<Vec<u8>> = None;
        let mut stack = Vec::new();
        let mut records = Vec::new();
        let mut text_block = 0usize;
        let mut shows_per_block = HashMap::<usize, usize>::new();
        let mut unsupported = false;
        for (index, op) in content.operations.iter().enumerate() {
            match op.operator.as_str() {
                "q" => stack.push(font_name.clone()),
                "Q" => font_name = stack.pop().flatten(),
                "BT" => text_block += 1,
                "Tf" => {
                    font_name = op
                        .operands
                        .first()
                        .and_then(|o| o.as_name().ok())
                        .map(|s| s.to_vec())
                }
                "Tj" | "TJ" | "'" | "\"" => {
                    let result = (|| {
                        let name = font_name.as_ref().ok_or("No font for Form text")?;
                        let font = fonts
                            .get(name)
                            .and_then(|f| doc.dereference(f).map(|v| v.1))
                            .and_then(Object::as_dict)
                            .map_err(|e| e.to_string())?;
                        let encoding = font.get_font_encoding(&doc).map_err(|e| e.to_string())?;
                        #[cfg(test)]
                        eprintln!(
                            "Form cleanup: decoded font {}",
                            String::from_utf8_lossy(name)
                        );
                        let mut text = String::new();
                        if op.operator == "TJ" {
                            for item in op
                                .operands
                                .first()
                                .ok_or("Missing TJ array")?
                                .as_array()
                                .map_err(|e| e.to_string())?
                            {
                                if let Object::String(bytes, _) = item {
                                    text.push_str(
                                        &Document::decode_text(&encoding, bytes)
                                            .map_err(|e| e.to_string())?,
                                    );
                                }
                            }
                        } else if let Some(Object::String(bytes, _)) = op.operands.last() {
                            text = Document::decode_text(&encoding, bytes)
                                .map_err(|e| e.to_string())?;
                        }
                        Ok::<_, String>(text)
                    })();
                    match result {
                        Ok(text) => {
                            if !normalized(&text).is_empty() {
                                records.push((index, text, text_block));
                                *shows_per_block.entry(text_block).or_default() += 1;
                            }
                        }
                        Err(_) => {
                            unsupported = true;
                            break;
                        }
                    }
                }
                _ => {}
            }
        }
        if unsupported {
            continue;
        }
        let key = signature(&records.iter().map(|r| r.1.clone()).collect::<Vec<_>>());
        #[cfg(test)]
        eprintln!(
            "Form cleanup: inventory {} entries, signature {key}",
            records.len()
        );
        let Some(removals) = targets.get(&key) else {
            continue;
        };
        for (&text_index, expected) in removals {
            let (index, text, block) = records
                .get(text_index)
                .ok_or("Vector Form text inventory changed")?;
            if normalized(text) != *expected {
                return Err("Vector Form text no longer matches canonical source".into());
            }
            // A later text-show in the same BT could depend on this run's advance.
            // Reject that case rather than shift untouched labels or formulas.
            if shows_per_block.get(block).copied().unwrap_or(0) > 1 {
                return Err(
                    "Vector Form has dependent text runs; cannot safely remove one label".into(),
                );
            }
            let op = &mut content.operations[*index];
            if op.operator == "TJ" {
                op.operands = vec![Object::Array(Vec::new())];
            } else if let Some(last) = op.operands.last_mut() {
                *last = Object::string_literal(Vec::new());
            }
            found.insert((key.clone(), text_index));
        }
        let bytes = content.encode().map_err(|e| e.to_string())?;
        doc.get_object_mut(id)
            .and_then(Object::as_stream_mut)
            .map_err(|e| e.to_string())?
            .set_plain_content(bytes);
    }
    for (key, removals) in &targets {
        for index in removals.keys() {
            if !found.contains(&(key.clone(), *index)) {
                return Err(
                    "Could not safely match a translated vector Form label; no PDF was written"
                        .into(),
                );
            }
        }
    }
    #[cfg(test)]
    eprintln!("Form cleanup: saving document");
    // Drop replaced content streams, including internal Unicode transport runs.
    // Referenced original graphics and metadata remain reachable from the root.
    doc.prune_objects();
    doc.compress();
    doc.save(path)
        .map_err(|e| format!("Save translated vector Forms: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn signature_is_ordered_and_whitespace_independent() {
        assert_eq!(
            signature(&["偏 移量".into(), "x".into()]),
            signature(&["偏移量".into(), "x".into()])
        );
        assert_ne!(
            signature(&["偏移量".into(), "x".into()]),
            signature(&["x".into(), "偏移量".into()])
        );
    }
}

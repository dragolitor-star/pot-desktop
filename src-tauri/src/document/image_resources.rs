//! Give every image invocation a private PDF resource before bitmap mutation.
//! PDFium shares CPDF_Image instances across Do operators. SetBitmap on one
//! occurrence can otherwise change a different chapter/step on another page.
//! Content order, matrices, clip paths and all non-image drawing operators stay.
use lopdf::{content::Content, Dictionary, Document, Object, ObjectId, Stream};
use std::path::{Path, PathBuf};

pub struct PrivateImageSource {
    path: PathBuf,
    owned: bool,
}
impl PrivateImageSource {
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn create(source: &Path, nonce: u64) -> Result<Self, String> {
        let path = std::env::temp_dir().join(format!(
            "ctranslate-images-{}-{nonce}.pdf",
            std::process::id()
        ));
        let mut guard = Self { path, owned: false };
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&guard.path)
            .map_err(|e| e.to_string())?;
        guard.owned = true;
        let mut document =
            Document::load(source).map_err(|e| format!("Read PDF image resources: {e}"))?;
        let mut serial = 0;
        for (_, page) in document.get_pages() {
            let resources = inherited_resources(&document, page)?;
            let bytes = document.get_page_content(page).map_err(|e| e.to_string())?;
            let (bytes, resources) =
                private_stream(&mut document, &bytes, resources, &mut serial, 0)?;
            let content = document.add_object(Stream::new(Dictionary::new(), bytes));
            let page = document
                .get_object_mut(page)
                .and_then(Object::as_dict_mut)
                .map_err(|e| e.to_string())?;
            page.set("Resources", Object::Dictionary(resources));
            page.set("Contents", Object::Reference(content));
        }
        document.prune_objects();
        document.compress();
        document.save_to(&mut file).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        Ok(guard)
    }
}
impl Drop for PrivateImageSource {
    fn drop(&mut self) {
        if self.owned {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

pub(crate) fn inherited_resources(document: &Document, mut id: ObjectId) -> Result<Dictionary, String> {
    for _ in 0..32 {
        let dictionary = document
            .get_object(id)
            .and_then(Object::as_dict)
            .map_err(|e| e.to_string())?;
        if let Ok(resources) = dictionary.get(b"Resources") {
            return document
                .dereference(resources)
                .and_then(|(_, o)| o.as_dict())
                .cloned()
                .map_err(|e| e.to_string());
        }
        match dictionary.get(b"Parent") {
            Ok(parent) => id = parent.as_reference().map_err(|e| e.to_string())?,
            Err(_) if dictionary.get(b"Type").and_then(Object::as_name).ok() == Some(b"Pages") => {
                // A valid blank page tree need not declare any resources.
                return Ok(Dictionary::new());
            }
            Err(e) => return Err(e.to_string()),
        }
    }
    Err("PDF resource inheritance exceeds supported depth".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn blank_page_tree_can_have_no_resources() {
        let mut document = Document::with_version("1.5");
        let mut pages = Dictionary::new();
        pages.set("Type", Object::Name(b"Pages".to_vec()));
        let parent = document.add_object(pages);
        let mut page = Dictionary::new();
        page.set("Type", Object::Name(b"Page".to_vec()));
        page.set("Parent", Object::Reference(parent));
        let id = document.add_object(page);
        assert!(inherited_resources(&document, id).unwrap().is_empty());
        let (content, resources) =
            private_stream(&mut document, b"", Dictionary::new(), &mut 0, 0).unwrap();
        assert!(content.is_empty());
        assert!(resources.is_empty());
    }
}

fn private_image(
    document: &mut Document,
    mut stream: Stream,
    depth: usize,
) -> Result<ObjectId, String> {
    if depth > 16 {
        return Err("PDF image-mask nesting exceeds supported depth".into());
    }
    for name in [b"SMask".as_slice(), b"Mask".as_slice()] {
        if let Ok(Object::Reference(id)) = stream.dict.get(name) {
            let mask = document
                .get_object(*id)
                .and_then(Object::as_stream)
                .cloned()
                .map_err(|e| e.to_string())?;
            let mask = private_image(document, mask, depth + 1)?;
            stream.dict.set(name, Object::Reference(mask));
        }
    }
    Ok(document.add_object(stream))
}

fn private_stream(
    document: &mut Document,
    bytes: &[u8],
    mut resources: Dictionary,
    serial: &mut u64,
    depth: usize,
) -> Result<(Vec<u8>, Dictionary), String> {
    if depth > 16 {
        return Err("PDF image Form nesting exceeds supported depth".into());
    }
    let original = match resources.get(b"XObject") {
        Ok(object) => document
            .dereference(object)
            .and_then(|(_, o)| o.as_dict())
            .cloned()
            .map_err(|e| e.to_string())?,
        Err(_) => Dictionary::new(),
    };
    let mut updated = Dictionary::new();
    let mut content =
        Content::decode(bytes).map_err(|e| format!("Read PDF image operators: {e}"))?;
    for operation in &mut content.operations {
        if operation.operator != "Do" {
            continue;
        }
        let name = operation
            .operands
            .first()
            .and_then(|o| o.as_name().ok())
            .ok_or("Invalid PDF XObject invocation")?
            .to_vec();
        let reference = original.get(&name).map_err(|e| e.to_string())?;
        let mut stream = document
            .dereference(reference)
            .and_then(|(_, o)| o.as_stream())
            .cloned()
            .map_err(|e| e.to_string())?;
        let subtype = stream
            .dict
            .get(b"Subtype")
            .and_then(Object::as_name)
            .map_err(|e| e.to_string())?;
        let object = if subtype == b"Image" {
            Object::Reference(private_image(document, stream, 0)?)
        } else if subtype == b"Form" {
            let child_resources = match stream.dict.get(b"Resources") {
                Ok(object) => document
                    .dereference(object)
                    .and_then(|(_, o)| o.as_dict())
                    .cloned()
                    .map_err(|e| e.to_string())?,
                Err(_) => resources.clone(),
            };
            let decoded = stream
                .decompressed_content()
                .unwrap_or_else(|_| stream.content.clone());
            let (decoded, child_resources) =
                private_stream(document, &decoded, child_resources, serial, depth + 1)?;
            stream
                .dict
                .set("Resources", Object::Dictionary(child_resources));
            stream.set_plain_content(decoded);
            Object::Reference(document.add_object(stream))
        } else {
            reference.clone()
        };
        *serial += 1;
        let name = format!("CTranslateImage{serial}").into_bytes();
        updated.set(name.clone(), object);
        operation.operands[0] = Object::Name(name);
    }
    if !original.is_empty() {
        resources.set("XObject", Object::Dictionary(updated));
    }
    Ok((content.encode().map_err(|e| e.to_string())?, resources))
}

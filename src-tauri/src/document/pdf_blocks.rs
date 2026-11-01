//! Canonical, object-owned text blocks shared by extraction and layout export.
use super::*;
use crate::document::form_text::FormTextRef;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct BlockBox {
    pub left: f32,
    pub bottom: f32,
    pub right: f32,
    pub top: f32,
}
impl BlockBox {
    pub fn width(&self) -> f32 {
        self.right - self.left
    }
    pub fn height(&self) -> f32 {
        self.top - self.bottom
    }
    fn union(self, other: Self) -> Self {
        Self {
            left: self.left.min(other.left),
            bottom: self.bottom.min(other.bottom),
            right: self.right.max(other.right),
            top: self.top.max(other.top),
        }
    }
    fn center(self) -> (f32, f32) {
        (
            (self.left + self.right) / 2.0,
            (self.bottom + self.top) / 2.0,
        )
    }
}
impl From<PdfRect> for BlockBox {
    fn from(r: PdfRect) -> Self {
        Self {
            left: r.left().value,
            bottom: r.bottom().value,
            right: r.right().value,
            top: r.top().value,
        }
    }
}
impl From<PdfQuadPoints> for BlockBox {
    fn from(r: PdfQuadPoints) -> Self {
        Self {
            left: r.left().value,
            bottom: r.bottom().value,
            right: r.right().value,
            top: r.top().value,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum TextAlign {
    Left,
    Center,
    Right,
    TocLine,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextBlock {
    pub block_id: String,
    pub text: String,
    pub translation_text: String,
    pub bbox: BlockBox,
    pub available_right: f32,
    pub available_left: f32,
    pub available_bottom: f32,
    pub font_size: f32,
    pub is_bold: bool,
    pub is_italic: bool,
    pub text_color: (u8, u8, u8),
    pub text_alpha: u8,
    pub alignment: TextAlign,
    pub obj_indices: Vec<usize>,
    pub form_text_refs: Vec<FormTextRef>,
    pub form_bounds: Option<BlockBox>,
    pub protected: bool,
    pub in_table: bool,
    pub toc_page: Option<String>,
    pub rotation: f32,
    pub baseline: (f32, f32),
    pub first_line_indent: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageLayout {
    pub page_index: usize,
    pub width: f32,
    pub height: f32,
    pub blocks: Vec<TextBlock>,
}

#[derive(Clone)]
struct Run {
    text: String,
    bbox: BlockBox,
    index: usize,
    size: f32,
    bold: bool,
    italic: bool,
    color: (u8, u8, u8),
    alpha: u8,
    rotation: f32,
    baseline: (f32, f32),
    math_font: bool,
    form_ref: Option<FormTextRef>,
    form_bounds: Option<BlockBox>,
}

pub(crate) fn transformed_box(b: BlockBox, m: PdfMatrix) -> BlockBox {
    let points = [
        (b.left, b.bottom),
        (b.left, b.top),
        (b.right, b.bottom),
        (b.right, b.top),
    ]
    .map(|(x, y)| (m.a() * x + m.c() * y + m.e(), m.b() * x + m.d() * y + m.f()));
    BlockBox {
        left: points.iter().map(|p| p.0).fold(f32::INFINITY, f32::min),
        right: points.iter().map(|p| p.0).fold(f32::NEG_INFINITY, f32::max),
        bottom: points.iter().map(|p| p.1).fold(f32::INFINITY, f32::min),
        top: points.iter().map(|p| p.1).fold(f32::NEG_INFINITY, f32::max),
    }
}
pub(crate) fn compose(p: PdfMatrix, c: PdfMatrix) -> PdfMatrix {
    PdfMatrix::new(
        p.a() * c.a() + p.c() * c.b(),
        p.b() * c.a() + p.d() * c.b(),
        p.a() * c.c() + p.c() * c.d(),
        p.b() * c.c() + p.d() * c.d(),
        p.a() * c.e() + p.c() * c.f() + p.e(),
        p.b() * c.e() + p.d() * c.f() + p.f(),
    )
}
fn object_run(
    text_page: &PdfPageText,
    object: &PdfPageObject,
    index: usize,
    parent: PdfMatrix,
    form_ref: Option<FormTextRef>,
) -> Result<Option<Run>, String> {
    let Some(text) = object.as_text_object() else {
        return Ok(None);
    };
    let source = text_page.for_object(text);
    if source.trim().is_empty() {
        return Ok(None);
    }
    let font = text.font();
    let name = font.name().to_lowercase();
    let color = object.fill_color().unwrap_or(PdfColor::BLACK);
    let matrix = compose(parent, object.matrix().map_err(|e| e.to_string())?);
    Ok(Some(Run {
        text: source,
        bbox: transformed_box(object.bounds().map_err(|e| e.to_string())?.into(), parent),
        index,
        size: (text.scaled_font_size().value * parent.c().hypot(parent.d())).max(1.0),
        bold: name.contains("bold")
            || name.contains("simhei")
            || name.contains("黑体")
            || matches!(
                font.weight(),
                Ok(PdfFontWeight::Weight700Bold
                    | PdfFontWeight::Weight800
                    | PdfFontWeight::Weight900)
            ),
        italic: font.is_italic(),
        color: (color.red(), color.green(), color.blue()),
        alpha: color.alpha(),
        rotation: matrix.b().atan2(matrix.a()).to_degrees(),
        baseline: (matrix.e(), matrix.f()),
        math_font: ["cmmi", "cmsy", "cmex", "stix", "symbol", "math"]
            .iter()
            .any(|s| name.contains(s)),
        form_ref,
        form_bounds: None,
    }))
}
fn collect_form_runs(
    text_page: &PdfPageText,
    form: &PdfPageXObjectFormObject,
    parent: PdfMatrix,
    form_bounds: BlockBox,
    index: usize,
    runs: &mut Vec<Run>,
    obstacles: &mut Vec<BlockBox>,
    depth: usize,
) -> Result<(), String> {
    if depth > 16 {
        return Err("Vector Form nesting exceeds supported depth".into());
    }
    let mut texts = Vec::new();
    for i in 0..form.len() {
        let object = form.get(i).map_err(|e| e.to_string())?;
        if let Some(text) = object.as_text_object() {
            let text = text_page.for_object(text);
            if !text.trim().is_empty() {
                texts.push(text);
            }
        }
    }
    let signature = crate::document::form_text::signature(&texts);
    let mut text_index = 0;
    for i in 0..form.len() {
        let object = form.get(i).map_err(|e| e.to_string())?;
        if let Some(text) = object.as_text_object() {
            let source = text_page.for_object(text);
            if source.trim().is_empty() {
                continue;
            }
            let reference = FormTextRef {
                signature: signature.clone(),
                text_index,
                source,
            };
            text_index += 1;
            if let Some(mut run) = object_run(text_page, &object, index, parent, Some(reference))? {
                run.form_bounds = Some(form_bounds);
                runs.push(run);
            }
        } else if let Some(child) = object.as_x_object_form_object() {
            collect_form_runs(
                text_page,
                child,
                compose(parent, object.matrix().map_err(|e| e.to_string())?),
                transformed_box(object.bounds().map_err(|e| e.to_string())?.into(), parent),
                index,
                runs,
                obstacles,
                depth + 1,
            )?;
        } else if object.as_path_object().is_some() {
            obstacles.push(transformed_box(
                object.bounds().map_err(|e| e.to_string())?.into(),
                parent,
            ));
        }
    }
    Ok(())
}

pub(crate) fn contains_han(s: &str) -> bool {
    s.chars()
        .any(|c| matches!(c, '\u{3400}'..='\u{9fff}' | '\u{f900}'..='\u{faff}'))
}

pub(crate) fn is_formula(s: &str) -> bool {
    let n = s.chars().filter(|c| !c.is_whitespace()).count().max(1);
    let han = s.chars().filter(|&c| contains_han(&c.to_string())).count();
    if han as f32 / n as f32 >= 0.15 {
        return false;
    }
    let operators = s.chars().filter(|c| "=+−-×÷/∆Δ∑∫√{}".contains(*c)).count();
    (s.contains('=') && operators >= 2)
        || s.contains("atan(")
        || s.contains("\\frac")
        || (operators >= 3 && operators as f32 / n as f32 > 0.18)
}

pub(crate) fn is_code_line(text: &str) -> bool {
    if !text.is_ascii() {
        return false;
    }
    let words: Vec<_> = text.split_whitespace().collect();
    let Some(head) = words.first() else {
        return false;
    };
    let command = head.contains('_')
        || matches!(
            *head,
            "helm" | "git" | "python" | "pip" | "npm" | "sudo" | "curl"
        );
    let file = words.len() == 1
        && [
            ".txt", ".bin", ".xml", ".yuv", ".json", ".log", ".dll", ".exe", ".cfg",
        ]
        .iter()
        .any(|ext| head.to_lowercase().ends_with(ext));
    file || (command
        && (words.len() == 1 || words.iter().skip(1).any(|word| word.parse::<f64>().is_ok())))
}

/// Returns title/page separately; dot leaders are geometry, never model input.
pub(crate) fn split_toc(s: &str) -> Option<(String, String)> {
    let s = s.trim();
    let last_separator = s.rfind(|c: char| !c.is_ascii_digit())?;
    let page_start = last_separator + s[last_separator..].chars().next()?.len_utf8();
    let page = s.get(page_start..)?.trim();
    if page.is_empty() || page.len() > 4 {
        return None;
    }
    let before = s[..page_start].trim_end();
    let title = before.trim_end_matches(|c: char| c.is_whitespace() || ".·⋅…・‧".contains(c));
    let leader = &before[title.len()..];
    if title.is_empty() || leader.chars().filter(|c| ".·⋅…・‧".contains(*c)).count() < 3 {
        return None;
    }
    Some((title.trim().to_string(), page.to_string()))
}

fn join_runs(runs: &[Run]) -> String {
    let mut result = String::new();
    for (i, run) in runs.iter().enumerate() {
        if i > 0 {
            let previous = &runs[i - 1];
            let gap = run.bbox.left - previous.bbox.right;
            if gap > run.size * 0.12
                && !result.ends_with(char::is_whitespace)
                && !run.text.starts_with(char::is_whitespace)
            {
                result.push(' ');
            }
        }
        result.push_str(&run.text);
    }
    result.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn technical_sentence_continues(previous: &str, next: &str) -> bool {
    let previous = previous.trim_end();
    !previous.ends_with(['。', '！', '？', '；', '：', '.', '!', '?', ';', ':'])
        && (previous.ends_with(['、', '，', ','])
            || (next
                .trim_start()
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic())
                && contains_han(next)))
}

fn mixed_cell_fragments(a: &str, b: &str) -> bool {
    (contains_han(a) || contains_han(b)) && !is_formula(a) && !is_formula(b)
}

fn has_inline_heading_prefix(bbox: BlockBox, size: f32, images: &[BlockBox]) -> bool {
    size >= 18.0
        && images.iter().any(|image| {
            image.right <= bbox.left
                && bbox.left - image.right <= size * 1.5
                && image.top.min(bbox.top) - image.bottom.max(bbox.bottom) >= size * 0.5
                && image.height() <= size * 1.7
                && image.width() <= size * 8.0
        })
}

// Fixed lanes are only appropriate for multiple independent running headers on
// the same row. A single cover heading needs the measured space beside it.
fn has_neighboring_header(block: &TextBlock, blocks: &[TextBlock], page_height: f32) -> bool {
    blocks.iter().any(|other| {
        other.block_id != block.block_id
            && other.bbox.top > page_height - 70.0
            && other.rotation.abs() < 1.0
            && other.bbox.bottom < block.bbox.top
            && other.bbox.top > block.bbox.bottom
            && (other.bbox.left >= block.bbox.right || other.bbox.right <= block.bbox.left)
    })
}

pub(crate) fn canonical_page_layout(
    page: &PdfPage,
    page_index: usize,
) -> Result<PageLayout, String> {
    let rules = collect_rules(page);
    let text_page = page.text().map_err(|e| e.to_string())?;
    let mut runs = Vec::new();
    let mut obstacles: Vec<BlockBox> = Vec::new();
    let mut inline_images = Vec::new();
    for (index, object) in page.objects().iter().enumerate() {
        let bounds: BlockBox = object
            .bounds()
            .map_err(|e| format!("Page {} object {index}: {e}", page_index + 1))?
            .into();
        if let Some(run) = object_run(&text_page, &object, index, PdfMatrix::IDENTITY, None)? {
            runs.push(run);
        } else if let Some(form) = object.as_x_object_form_object() {
            collect_form_runs(
                &text_page,
                form,
                object.matrix().map_err(|e| e.to_string())?,
                bounds,
                index,
                &mut runs,
                &mut obstacles,
                0,
            )?;
            obstacles.push(bounds);
        } else if object.as_image_object().is_some() || rectilinear_path(&object) {
            obstacles.push(bounds);
            if object.as_image_object().is_some() {
                inline_images.push(bounds);
            }
        }
    }
    // Quantized visual rows, independent of PDF stream order. ASCII spans stay
    // with their adjacent Han spans; no language/length filter runs here.
    runs.sort_by(|a, b| {
        b.bbox
            .top
            .total_cmp(&a.bbox.top)
            .then(a.bbox.left.total_cmp(&b.bbox.left))
            .then(a.index.cmp(&b.index))
    });
    let mut lines: Vec<Vec<Run>> = Vec::new();
    for run in runs {
        let slot = lines.iter().position(|line| {
            let a = &line[0];
            (a.baseline.1 - run.baseline.1).abs() < a.size.min(run.size) * 0.32
                && (a.rotation - run.rotation).abs() < 1.0
        });
        if let Some(i) = slot {
            lines[i].push(run);
        } else {
            lines.push(vec![run]);
        }
    }
    let mut blocks: Vec<TextBlock> = Vec::new();
    for mut row in lines {
        row.sort_by(|a, b| a.bbox.left.total_cmp(&b.bbox.left));
        let mut groups: Vec<Vec<Run>> = Vec::new();
        for run in row {
            let append = groups.last().is_some_and(|g| {
                let prev = &g[g.len() - 1];
                let gap = run.bbox.left - prev.bbox.right;
                let same_cell = in_same_cell(
                    &rules,
                    prev.bbox.center(),
                    run.bbox.center(),
                    page.width().value * 0.9,
                );
                !has_rule_between(
                    &rules.vertical,
                    prev.bbox.right,
                    run.bbox.left,
                    run.bbox.center().1,
                ) && (same_cell || gap < prev.size.max(run.size) * 1.8 || prev.text.contains("..."))
                    && (prev.rotation - run.rotation).abs() < 1.0
            });
            if append {
                if let Some(g) = groups.last_mut() {
                    g.push(run);
                }
            } else {
                groups.push(vec![run]);
            }
        }
        for group in groups {
            let first = &group[0];
            let text = join_runs(&group);
            let bbox = group.iter().fold(first.bbox, |b, r| b.union(r.bbox));
            let cell = cell_span(&rules.vertical, bbox.center().0, bbox.center().1)
                .filter(|s| s.1 - s.0 < page.width().value * 0.9);
            let toc = split_toc(&text);
            let protected = (group.iter().any(|r| r.form_ref.is_some())
                && text
                    .chars()
                    .all(|c| c.is_ascii_alphabetic() || c.is_whitespace())
                && text.chars().filter(|c| !c.is_whitespace()).count() <= 3)
                || is_formula(&text)
                || is_code_line(&text)
                || (!contains_han(&text) && group.iter().any(|r| r.math_font))
                || !text.chars().any(char::is_alphabetic);
            let content_right = (page.width().value - 55.0)
                .max(bbox.right)
                .min(page.width().value - 5.0);
            let mut block = TextBlock {
                block_id: String::new(),
                translation_text: toc
                    .as_ref()
                    .map(|t| t.0.clone())
                    .unwrap_or_else(|| text.clone()),
                text,
                bbox,
                available_right: cell.map(|s| s.1 - 2.0).unwrap_or(content_right),
                available_left: bbox.left,
                available_bottom: bbox.bottom,
                font_size: first.size,
                is_bold: first.bold,
                is_italic: first.italic,
                text_color: first.color,
                text_alpha: first.alpha,
                alignment: if toc.is_some() {
                    TextAlign::TocLine
                } else {
                    TextAlign::Left
                },
                obj_indices: group
                    .iter()
                    .filter(|r| r.form_ref.is_none())
                    .map(|r| r.index)
                    .collect(),
                form_text_refs: group.iter().filter_map(|r| r.form_ref.clone()).collect(),
                form_bounds: if group.iter().all(|r| r.form_ref.is_some()) {
                    first.form_bounds
                } else {
                    None
                },
                protected,
                in_table: cell.is_some(),
                toc_page: toc.map(|t| t.1),
                rotation: first.rotation,
                baseline: first.baseline,
                first_line_indent: 0.0,
            };
            // Wrapped lines within one cell are a single translation unit.
            let previous = blocks.iter_mut().rev().find(|b| {
                b.in_table
                    && block.in_table
                    && (b.font_size - block.font_size).abs() < 0.75
                    && b.is_bold == block.is_bold
                    && b.is_italic == block.is_italic
                    && b.text_color == block.text_color
                    && mixed_cell_fragments(&b.text, &block.text)
                    && in_same_cell(
                        &rules,
                        b.bbox.center(),
                        block.bbox.center(),
                        page.width().value * 0.9,
                    )
                    && (b.bbox.bottom - block.bbox.top).abs() < block.font_size * 1.6
            });
            if let Some(previous) = previous {
                previous.text.push(' ');
                previous.text.push_str(&block.text);
                previous.translation_text = previous.text.clone();
                previous.protected = false;
                previous.bbox = previous.bbox.union(block.bbox);
                previous.obj_indices.append(&mut block.obj_indices);
                previous.form_text_refs.append(&mut block.form_text_refs);
            } else {
                blocks.push(block);
            }
        }
    }
    blocks.sort_by(|a, b| {
        b.bbox
            .top
            .total_cmp(&a.bbox.top)
            .then(a.bbox.left.total_cmp(&b.bbox.left))
    });
    // Paragraph continuations are grouped HERE, once, before assigning IDs.
    // Export consumes these exact groups; it never splits translations by line.
    let mut paragraphs: Vec<TextBlock> = Vec::new();
    let mut last_line: Option<BlockBox> = None;
    for mut block in blocks {
        let continuation = paragraphs
            .last()
            .zip(last_line)
            .is_some_and(|(previous, last)| {
                let right_margin = page.width().value - 55.0;
                let compact: Vec<_> = block
                    .text
                    .chars()
                    .filter(|c| !c.is_whitespace())
                    .take(16)
                    .collect();
                let digits = compact.iter().take_while(|c| c.is_ascii_digit()).count();
                let numbered = (digits > 0
                    && compact
                        .get(digits)
                        .is_some_and(|c| matches!(c, ')' | '）' | '、' | '.')))
                    || block.text.starts_with(['•', '\u{f06c}'])
                    || block.text.trim_start().starts_with("图")
                    || block.text.trim_start().starts_with("表");
                let gap = last.bottom - block.bbox.top;
                // Centered, tightly stacked Form labels are one phrase. Keep
                // their lines together instead of fitting each into a narrower
                // Chinese-only word box (e.g. motion-center coordinate system).
                let form_continuation = previous
                    .form_text_refs
                    .last()
                    .zip(block.form_text_refs.first())
                    .is_some_and(|(a, b)| a.signature == b.signature)
                    && previous.text_color == block.text_color
                    && (last.center().0 - block.bbox.center().0).abs() < block.font_size * 0.8
                    && gap >= 0.0
                    && gap < block.font_size * 0.5;
                !previous.in_table
                    && !block.in_table
                    && !previous.protected
                    && !block.protected
                    && previous.toc_page.is_none()
                    && block.toc_page.is_none()
                    && previous.rotation.abs() < 1.0
                    && block.rotation.abs() < 1.0
                    && previous.bbox.top < page.height().value - 70.0
                    && block.font_size < 13.0
                    && (previous.font_size - block.font_size).abs() < 0.75
                    && previous.is_bold == block.is_bold
                    && !numbered
                    && !previous.text.contains("——")
                    && !block.text.contains("——")
                    && !previous.text.starts_with('图')
                    && !previous.text.starts_with('表')
                    && (form_continuation
                        || last.right > right_margin - block.font_size * 2.5
                        || (technical_sentence_continues(&previous.text, &block.text)
                            && last.right > right_margin - block.font_size * 6.0))
                    && gap >= -0.5
                    && gap < block.font_size * 1.4
                    && (last.left - block.bbox.left).abs() < block.font_size * 4.0
                    && block.bbox.left <= last.left + block.font_size * 0.8
                    && !(previous.text.trim_end().ends_with(['。', '！', '？', '；'])
                        && block.bbox.left >= last.left - 1.0)
                    && !has_rule_between(
                        &rules.horizontal,
                        last.bottom,
                        block.bbox.top,
                        block.bbox.left,
                    )
            });
        let current_line = block.bbox;
        if continuation {
            if let Some(previous) = paragraphs.last_mut() {
                previous.text.push(' ');
                previous.text.push_str(&block.text);
                previous.translation_text = previous.text.clone();
                previous.bbox = previous.bbox.union(block.bbox);
                previous.first_line_indent = if previous.form_bounds.is_some() {
                    0.0
                } else {
                    (previous.baseline.0 - previous.bbox.left).max(0.0)
                };
                previous.obj_indices.append(&mut block.obj_indices);
                previous.form_text_refs.append(&mut block.form_text_refs);
            }
        } else {
            paragraphs.push(block);
        }
        last_line = Some(current_line);
    }
    let mut blocks = paragraphs;
    for (i, block) in blocks.iter_mut().enumerate() {
        block.block_id = format!("p{}_b{}", page_index + 1, i);
    }
    let snapshot = blocks.clone();
    for (i, block) in blocks.iter_mut().enumerate() {
        block.translation_text = block
            .translation_text
            .replace(['\u{f09f}', '\u{f06c}'], "•");
        block.block_id = format!("p{}_b{}", page_index + 1, i);
        let size = block.font_size;
        if let Some(container) = block.form_bounds {
            block.available_left = (container.left + 1.0).min(block.bbox.left);
            block.available_right = block
                .available_right
                .min(container.right - 1.0)
                .max(block.bbox.right);
            for obstacle in &obstacles {
                if obstacle.top <= block.bbox.bottom || obstacle.bottom >= block.bbox.top {
                    continue;
                }
                if obstacle.left <= block.bbox.left
                    && obstacle.right >= block.bbox.right
                    && obstacle.top >= block.bbox.top
                    && obstacle.bottom <= block.bbox.bottom
                {
                    block.available_left = block
                        .available_left
                        .max(obstacle.left + 1.0)
                        .min(block.bbox.left);
                    block.available_right = block
                        .available_right
                        .min(obstacle.right - 1.0)
                        .max(block.bbox.right);
                }
                if obstacle.right <= block.bbox.left + 0.1 {
                    block.available_left = block
                        .available_left
                        .max(obstacle.right + 1.0)
                        .min(block.bbox.left);
                }
                if obstacle.left > block.bbox.left && obstacle.left >= block.bbox.right - size * 0.3
                {
                    block.available_right = block
                        .available_right
                        .min((obstacle.left - 1.0).max(block.bbox.right));
                } else if obstacle.left > block.bbox.left
                    && obstacle.left < block.bbox.right
                    && obstacle.right > block.bbox.right
                    && obstacle.height() > size * 2.0
                {
                    block.available_right = block
                        .available_right
                        .min(obstacle.right - 1.0)
                        .max(block.bbox.right);
                }
            }
        }
        if block.rotation.abs() > 1.0 {
            block.available_right = block.bbox.right;
            continue;
        }
        for other in snapshot
            .iter()
            .filter(|o| o.block_id != block.block_id && o.rotation.abs() < 1.0)
        {
            if block.form_bounds.is_some()
                && other.bbox.bottom < block.bbox.top
                && other.bbox.top > block.bbox.bottom
                && other.bbox.right <= block.bbox.left + 0.1
            {
                block.available_left = block
                    .available_left
                    .max(other.bbox.right + 1.0)
                    .min(block.bbox.left);
            }
            if other.bbox.bottom < block.bbox.top
                && other.bbox.top > block.bbox.bottom
                && other.bbox.left >= block.bbox.right - 1.0
            {
                block.available_right = block.available_right.min(other.bbox.left - 6.0);
            }
        }
        for obstacle in &obstacles {
            if obstacle.bottom < block.bbox.top
                && obstacle.top > block.bbox.bottom
                && obstacle.left >= block.bbox.right - 1.0
            {
                block.available_right = block.available_right.min(obstacle.left - 2.0);
            }
        }
        // Physical objects below and rules bound growth. A page-wide frame is
        // a container, so its top edge must not block all interior text.
        let right = block.available_right;
        let next_top = snapshot
            .iter()
            .filter(|o| {
                o.block_id != block.block_id
                    && o.rotation.abs() < 1.0
                    && o.bbox.top <= block.bbox.bottom + size * 0.15
                    // Reserve the next line's possible translated width too.
                    // Its short Chinese text can expand into this column.
                    && o.available_right.max(o.bbox.right) > block.bbox.left
                    && o.bbox.left < right
            })
            .map(|o| o.bbox.top)
            .chain(
                obstacles
                    .iter()
                    .filter(|o| {
                        o.top <= block.bbox.bottom + 0.5
                            && o.right > block.bbox.left
                            && o.left < right
                    })
                    .map(|o| o.top),
            )
            .fold(35.0_f32, f32::max);
        block.available_bottom = (next_top + 2.0).min(block.bbox.bottom);
        if let Some(boundary) = rules
            .horizontal
            .iter()
            .filter(|r| r.pos <= block.bbox.bottom && r.spans(block.bbox.center().0))
            .map(|r| r.pos + 2.0)
            .max_by(f32::total_cmp)
        {
            block.available_bottom = block.available_bottom.max(boundary.min(block.bbox.bottom));
        }
        let center = block.bbox.center().0;
        let container = cell_span(&rules.vertical, center, block.bbox.center().1);
        let (left, right) = container.unwrap_or((55.0, page.width().value - 55.0));
        if block.alignment != TextAlign::TocLine
            && block.bbox.width() < (right - left) * 0.85
            && (center - (left + right) / 2.0).abs() < size * 0.8
        {
            block.alignment = TextAlign::Center;
            block.available_right = block
                .available_right
                .min(center + (center - left))
                .min(right - 2.0);
        }
        // A centered source heading can have a separate Word bitmap prefix.
        // Re-centering its longer translation on the old Chinese center moves
        // it over that bitmap. Keep the original start after the prefix and
        // use the measured free space to the right instead.
        if block.alignment == TextAlign::Center
            && block.form_bounds.is_none()
            && has_inline_heading_prefix(block.bbox, block.font_size, &inline_images)
        {
            block.alignment = TextAlign::Left;
        }
        // Running furniture gets distinct, fixed lanes, including a 12pt gap.
        if block.bbox.top > page.height().value - 70.0
            && block.font_size < 15.0
            && block.rotation.abs() < 1.0
            && !block.in_table
            && has_neighboring_header(block, &snapshot, page.height().value)
        {
            if block.bbox.left < 180.0 {
                block.available_right = block.available_right.min(178.0);
            } else if block.bbox.left < 430.0 {
                block.bbox.left = 190.0;
                block.bbox.right = 430.0;
                block.available_right = 430.0;
                block.alignment = TextAlign::Center;
            }
            block.available_bottom = block.bbox.bottom;
        }
    }
    Ok(PageLayout {
        page_index,
        width: page.width().value,
        height: page.height().value,
        blocks,
    })
}

#[tauri::command]
pub fn extract_pdf_layout(
    app_handle: tauri::AppHandle,
    path: String,
) -> Result<Vec<PageLayout>, String> {
    let library = ensure_pdfium_library(&app_handle)?;
    let pdfium = Pdfium::new(Pdfium::bind_to_library(library).map_err(|e| e.to_string())?);
    let doc = pdfium
        .load_pdf_from_file(&path, None)
        .map_err(|e| e.to_string())?;
    doc.pages()
        .iter()
        .enumerate()
        .map(|(i, page)| canonical_page_layout(&page, i))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn chapter_heading_stays_after_its_bitmap_prefix() {
        let heading = BlockBox {
            left: 270.86,
            bottom: 730.94,
            right: 357.95,
            top: 752.05,
        };
        let badge = BlockBox {
            left: 203.04,
            bottom: 730.44,
            right: 259.56,
            top: 752.64,
        };
        assert!(has_inline_heading_prefix(heading, 22.08, &[badge]));
        assert!(!has_inline_heading_prefix(heading, 10.56, &[badge]));
        let logo = BlockBox {
            left: 441.95,
            bottom: 780.62,
            right: 535.55,
            top: 795.17,
        };
        assert!(!has_inline_heading_prefix(heading, 22.08, &[logo]));
    }
    #[test]
    fn a_wrapped_filename_in_a_chinese_cell_is_not_an_independent_code_block() {
        assert!(mixed_cell_fragments(
            "配置参数载入异常，clb/fld 文件夹缺少",
            "prm.txt"
        ));
        assert!(mixed_cell_fragments(
            "clb/fld",
            "中 input 文件夹缺少 laser_data.bin"
        ));
        assert!(!mixed_cell_fragments("-2001", "prm.txt"));
        assert!(!mixed_cell_fragments(
            "式中",
            "∆θ = atan(abs(∆y) / 2L) × 1000 (2-1)"
        ));
    }
    #[test]
    fn wrapped_technical_identifiers_belong_to_the_complete_sentence() {
        assert!(technical_sentence_continues(
            "得到激光外参 laser_x 、 laser_y 、",
            "laser_theta 三个参数。"
        ));
        assert!(technical_sentence_continues(
            "及定位中心",
            "Olaser 。错误的激光外参"
        ));
        assert!(!technical_sentence_continues(
            "设置完成。",
            "MapStudioPro 配置"
        ));
        assert!(!technical_sentence_continues("参数如下：", "laser_x 参数"));
    }
    #[test]
    fn formulas_and_explanations_are_distinct() {
        assert!(is_formula("∆θ = atan(abs(∆y) / 2L) × 1000 (2-1)"));
        assert!(is_formula("{ delt_x1 = x2 - x1 } (3-1)"));
        assert!(!is_formula("式中，atan——反正切计算"));
        assert!(!is_formula(
            "clb/fld 中 input 文件夹缺少 laser_data.bin 文件"
        ));
        assert!(is_code_line("castor_set_helm_the_calib 0 300 300"));
        assert!(is_code_line("helm set 0 1018"));
        assert!(!is_code_line("The calibration starts here"));
    }
    #[test]
    fn toc_page_is_never_part_of_the_translation_input() {
        assert_eq!(
            split_toc("校准步骤 . . . . . 129"),
            Some(("校准步骤".into(), "129".into()))
        );
        assert_eq!(split_toc("图 2-1"), None);
    }

    #[test]
    #[ignore = "Requires CTRANSLATE_TEST_PDF, CTRANSLATE_PDFIUM and CTRANSLATE_AUDIT_DIR"]
    fn audit_manual_canonical_blocks() {
        let path = std::env::var("CTRANSLATE_TEST_PDF").expect("test PDF path");
        let library = std::env::var("CTRANSLATE_PDFIUM").expect("PDFium library path");
        let output = std::path::PathBuf::from(
            std::env::var("CTRANSLATE_AUDIT_DIR").expect("audit output directory"),
        );
        let pdfium = Pdfium::new(Pdfium::bind_to_library(library).unwrap());
        let doc = pdfium.load_pdf_from_file(&path, None).unwrap();
        let mut layouts = Vec::new();
        for (i, page) in doc.pages().iter().enumerate() {
            let layout = canonical_page_layout(&page, i).unwrap();
            let again = canonical_page_layout(&page, i).unwrap();
            assert_eq!(
                serde_json::to_string(&layout).unwrap(),
                serde_json::to_string(&again).unwrap()
            );
            let mut owned = std::collections::HashSet::new();
            for block in &layout.blocks {
                for index in &block.obj_indices {
                    assert!(
                        owned.insert(index),
                        "Duplicate text object owner on page {}",
                        i + 1
                    );
                }
            }
            let text_page = page.text().unwrap();
            for (index, object) in page.objects().iter().enumerate() {
                if let Some(text) = object.as_text_object() {
                    if !text_page.for_object(text).trim().is_empty() {
                        assert!(owned.contains(&index), "Unowned text on page {}", i + 1);
                    }
                }
            }
            layouts.push(layout);
        }
        std::fs::create_dir_all(&output).unwrap();
        std::fs::write(
            output.join("canonical-layout.json"),
            serde_json::to_vec_pretty(&layouts).unwrap(),
        )
        .unwrap();
        println!(
            "Audited {} pages and {} canonical blocks",
            layouts.len(),
            layouts.iter().map(|p| p.blocks.len()).sum::<usize>()
        );
    }
}

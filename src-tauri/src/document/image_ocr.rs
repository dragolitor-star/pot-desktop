//! Local measured OCR geometry. Tesseract already ships in the frontend; this
//! broker keeps document pixels local and accepts results only for its own job.
use super::image_translate::TextRegion;
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc, Mutex,
    },
    time::{Duration, Instant},
};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct OcrBox {
    pub x0: u32,
    pub y0: u32,
    pub x1: u32,
    pub y1: u32,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct OcrWord {
    pub text: String,
    pub bbox: OcrBox,
    #[serde(default)]
    pub confidence: Option<f32>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct OcrLine {
    pub words: Vec<OcrWord>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct OcrDocument {
    pub width: u32,
    pub height: u32,
    pub lines: Vec<OcrLine>,
}

pub fn contrast_copy(image: &image::RgbaImage) -> image::RgbaImage {
    let gray = image::DynamicImage::ImageRgba8(image.clone()).into_luma8();
    let mut result = image::RgbaImage::new(image.width(), image.height());
    for top in (0..image.height()).step_by(64) {
        for left in (0..image.width()).step_by(64) {
            let right = (left + 64).min(image.width());
            let bottom = (top + 64).min(image.height());
            let mut histogram = [0u32; 256];
            let (mut low, mut high) = (255u8, 0u8);
            for y in top..bottom {
                for x in left..right {
                    let v = gray.get_pixel(x, y)[0];
                    histogram[v as usize] += 1;
                    low = low.min(v);
                    high = high.max(v);
                }
            }
            let mut cumulative = 0;
            let mut median = 0;
            for (v, n) in histogram.iter().enumerate() {
                cumulative += n;
                if cumulative >= (right - left) * (bottom - top) / 2 {
                    median = v;
                    break;
                }
            }
            for y in top..bottom {
                for x in left..right {
                    let value = if high.saturating_sub(low) < 12 {
                        255
                    } else {
                        let v = (gray.get_pixel(x, y)[0].saturating_sub(low) as u32 * 255
                            / (high - low) as u32) as u8;
                        if median < 128 {
                            255 - v
                        } else {
                            v
                        }
                    };
                    result.put_pixel(x, y, image::Rgba([value, value, value, 255]));
                }
            }
        }
    }
    result
}

type Reply = Result<OcrDocument, String>;
static NEXT: AtomicU64 = AtomicU64::new(1);
static PENDING: Lazy<Mutex<HashMap<u64, mpsc::Sender<Reply>>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));
struct Pending(u64);
impl Drop for Pending {
    fn drop(&mut self) {
        if let Ok(mut map) = PENDING.lock() {
            map.remove(&self.0);
        }
    }
}

#[tauri::command]
pub fn submit_layout_image_ocr(
    job_id: u64,
    document: Option<OcrDocument>,
    error: Option<String>,
) -> Result<bool, String> {
    let sender = PENDING.lock().map_err(|e| e.to_string())?.remove(&job_id);
    if let Some(sender) = sender {
        let reply =
            document.ok_or_else(|| error.unwrap_or_else(|| "Local image OCR failed".into()));
        return Ok(sender.send(reply).is_ok());
    }
    Ok(false)
}

pub fn request(
    app: &tauri::AppHandle,
    run_id: u64,
    png: &[u8],
    normalized_png: &[u8],
    width: u32,
    height: u32,
    language: &str,
    psm: u32,
    stop: &impl Fn() -> bool,
) -> Reply {
    use base64::Engine;
    use tauri::Manager;
    let job_id = NEXT.fetch_add(1, Ordering::SeqCst);
    let (sender, receiver) = mpsc::channel();
    PENDING
        .lock()
        .map_err(|e| e.to_string())?
        .insert(job_id, sender);
    let _guard = Pending(job_id);
    let payload = serde_json::json!({"jobId":job_id,"runId":run_id,"imageBase64":base64::engine::general_purpose::STANDARD.encode(png),"normalizedBase64":base64::engine::general_purpose::STANDARD.encode(normalized_png),"width":width,"height":height,"language":language,"psm":psm});
    app.trigger_global("layout_export://image_ocr", Some(payload.to_string()));
    app.emit_all("layout_export://image_ocr",payload).map_err(|e|e.to_string())?;
    let started = Instant::now();
    loop {
        if stop() {
            return Err("cancelled".into());
        }
        match receiver.recv_timeout(Duration::from_millis(100)) {
            Ok(reply) => return reply,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err("Local OCR worker disconnected".into())
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        if started.elapsed() > Duration::from_secs(180) {
            return Err("Local OCR worker timed out; retry the export after language data finishes downloading".into());
        }
    }
}

fn normalized(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}
fn similarity_chars(a: &[char], b: &[char]) -> f32 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    if a.len().min(b.len()) * 4 < a.len().max(b.len()) * 3 {
        return 0.0;
    }
    let mut row: Vec<_> = (0..=b.len()).collect();
    for (i, x) in a.iter().enumerate() {
        let mut previous = row[0];
        row[0] = i + 1;
        for (j, y) in b.iter().enumerate() {
            let old = row[j + 1];
            row[j + 1] = (row[j + 1] + 1)
                .min(row[j] + 1)
                .min(previous + usize::from(x != y));
            previous = old;
        }
    }
    1.0 - row[b.len()] as f32 / a.len().max(b.len()) as f32
}
fn overlaps(a: [u32; 4], b: [u32; 4]) -> bool {
    let area = (a[3].min(b[3]).saturating_sub(a[1].max(b[1]))) as u64
        * (a[2].min(b[2]).saturating_sub(a[0].max(b[0]))) as u64;
    let smaller = ((a[3] - a[1]) as u64 * (a[2] - a[0]) as u64)
        .min((b[3] - b[1]) as u64 * (b[2] - b[0]) as u64);
    area * 2 > smaller
}

/// A semantic detector's empty answer is not proof that a bitmap has no text.
/// Use independent, reliable local observations to reject omitted source labels.
pub fn uncovered_han_words(regions: &[TextRegion], ocr: &OcrDocument) -> Vec<OcrWord> {
    if ocr.width == 0 || ocr.height == 0 {
        return Vec::new();
    }
    let mut seen = std::collections::HashSet::new();
    let mut missing = Vec::new();
    for word in ocr.lines.iter().flat_map(|line| &line.words) {
        let han = word
            .text
            .chars()
            .filter(|c| matches!(*c as u32, 0x3400..=0x9fff))
            .count();
        if word
            .confidence
            .is_none_or(|confidence| !confidence.is_finite() || confidence < 90.0)
            || han == 0
            || ["三", "口", "一"].contains(&word.text.trim())
        {
            continue;
        }
        let b = &word.bbox;
        if b.x1 <= b.x0 || b.y1 <= b.y0 || b.x1 > ocr.width || b.y1 > ocr.height {
            continue;
        }
        let covered = regions.iter().any(|region| {
            let [y0, x0, y1, x1] = region.box_2d;
            let (x0, y0, x1, y1) = (
                x0 as f64 * ocr.width as f64 / 1000.0,
                y0 as f64 * ocr.height as f64 / 1000.0,
                x1 as f64 * ocr.width as f64 / 1000.0,
                y1 as f64 * ocr.height as f64 / 1000.0,
            );
            let overlap = (x1.min(b.x1 as f64) - x0.max(b.x0 as f64)).max(0.0)
                * (y1.min(b.y1 as f64) - y0.max(b.y0 as f64)).max(0.0);
            overlap * 2.0 >= (b.x1 - b.x0) as f64 * (b.y1 - b.y0) as f64
        });
        if !covered && seen.insert((word.text.clone(), b.x0, b.y0, b.x1, b.y1)) {
            missing.push(word.clone());
        }
    }
    missing
}

/// Coordinate coverage alone is insufficient: a mask can erase only one glyph
/// from a measured word. Reject unchanged reliable source ink before caching.
pub fn unchanged_han_words(
    original: &image::RgbaImage,
    translated: &image::RgbaImage,
    ocr: &OcrDocument,
) -> Vec<OcrWord> {
    let words = uncovered_han_words(&[], ocr);
    if original.dimensions() != translated.dimensions() {
        return words;
    }
    words
        .into_iter()
        .filter(|word| {
            let b = &word.bbox;
            let x0 = (b.x0 as u64 * original.width() as u64 / ocr.width as u64) as u32;
            let y0 = (b.y0 as u64 * original.height() as u64 / ocr.height as u64) as u32;
            let x1 = ((b.x1 as u64 * original.width() as u64 + ocr.width as u64 - 1)
                / ocr.width as u64) as u32;
            let y1 = ((b.y1 as u64 * original.height() as u64 + ocr.height as u64 - 1)
                / ocr.height as u64) as u32;
            let mut changed = 0u64;
            let mut visible = false;
            for y in y0..y1.min(original.height()) {
                for x in x0..x1.min(original.width()) {
                    let a = original.get_pixel(x, y);
                    let b = translated.get_pixel(x, y);
                    visible |= a[3] > 16;
                    changed += u64::from((0..4).any(|c| a[c].abs_diff(b[c]) > 10));
                }
            }
            visible && changed * 100 < (x1 - x0) as u64 * (y1 - y0) as u64
        })
        .collect()
}

pub fn localize(regions: &mut [TextRegion], ocr: &OcrDocument) -> Result<Vec<usize>, String> {
    if ocr.width == 0 || ocr.height == 0 || ocr.width > 16000 || ocr.height > 16000 {
        return Err("Invalid local OCR image dimensions".into());
    }
    let mut candidates = Vec::<(String, [u32; 4])>::new();
    let mut whole_lines = Vec::new();
    for line in &ocr.lines {
        let words: Vec<_> = line
            .words
            .iter()
            .filter(|w| {
                !normalized(&w.text).is_empty()
                    && w.bbox.x0 < w.bbox.x1
                    && w.bbox.y0 < w.bbox.y1
                    && w.bbox.x1 <= ocr.width
                    && w.bbox.y1 <= ocr.height
            })
            .collect();
        for start in 0..words.len() {
            let mut text = String::new();
            let mut rect = [u32::MAX, u32::MAX, 0, 0];
            for end in start..words.len().min(start + 48) {
                let w = words[end];
                text.push_str(&normalized(&w.text));
                rect = [
                    rect[0].min(w.bbox.y0),
                    rect[1].min(w.bbox.x0),
                    rect[2].max(w.bbox.y1),
                    rect[3].max(w.bbox.x1),
                ];
                let norm = [
                    (rect[0] as u64 * 1000 / ocr.height as u64) as u32,
                    (rect[1] as u64 * 1000 / ocr.width as u64) as u32,
                    ((rect[2] as u64 * 1000 + ocr.height as u64 - 1) / ocr.height as u64) as u32,
                    ((rect[3] as u64 * 1000 + ocr.width as u64 - 1) / ocr.width as u64) as u32,
                ];
                candidates.push((text.clone(), norm));
                if start == 0 && end + 1 == words.len() {
                    whole_lines.push((text.clone(), norm));
                }
            }
        }
    }
    for pair in whole_lines.windows(2) {
        let (a, ra) = &pair[0];
        let (b, rb) = &pair[1];
        if ra[0] < rb[0]
            && rb[0].saturating_sub(ra[2]) < (ra[2] - ra[0]) * 2
            && ra[1].abs_diff(rb[1]) < 60
        {
            candidates.push((
                format!("{a}{b}"),
                [ra[0], ra[1].min(rb[1]), rb[2], ra[3].max(rb[3])],
            ));
        }
    }
    let max_lines = regions
        .iter()
        .map(|r| r.source_text.lines().count())
        .max()
        .unwrap_or(1)
        .min(8);
    for count in 3..=max_lines {
        for paragraph in whole_lines.windows(count) {
            if paragraph.windows(2).all(|pair| {
                let a = pair[0].1;
                let b = pair[1].1;
                a[0] < b[0]
                    && b[0].saturating_sub(a[2]) < (a[2] - a[0]) * 2
                    && a[1].abs_diff(b[1]) < 60
            }) {
                let text = paragraph
                    .iter()
                    .map(|(s, _)| s.as_str())
                    .collect::<String>();
                let rect = paragraph.iter().fold([1000, 1000, 0, 0], |a, (_, b)| {
                    [
                        a[0].min(b[0]),
                        a[1].min(b[1]),
                        a[2].max(b[2]),
                        a[3].max(b[3]),
                    ]
                });
                candidates.push((text, rect));
            }
        }
    }
    let one_printed_line = whole_lines.first().is_some_and(|(_, first)| {
        whole_lines.iter().all(|(_, r)| {
            r[2].min(first[2]).saturating_sub(r[0].max(first[0])) * 2
                >= (r[2] - r[0]).min(first[2] - first[0])
        })
    });
    candidates.sort_unstable();
    candidates.dedup();
    // Reuse code points across all source labels. Dense screenshots otherwise
    // allocate the same character arrays for every candidate/label pair.
    let prepared = candidates
        .iter()
        .map(|(s, r)| {
            let chars: Vec<char> = s.chars().collect();
            let han: Vec<char> = chars
                .iter()
                .copied()
                .filter(|c| ('\u{3400}'..='\u{9fff}').contains(c))
                .collect();
            (chars, han, *r)
        })
        .collect::<Vec<_>>();
    let mut order: Vec<_> = (0..regions.len()).collect();
    order.sort_by_key(|i| std::cmp::Reverse(regions[*i].source_text.chars().count()));
    let mut taken = Vec::new();
    let mut missing = Vec::new();
    for i in order {
        if regions[i].source_text.trim() == regions[i].translation.trim() {
            continue;
        }
        let source = normalized(&regions[i].source_text);
        let source_chars: Vec<char> = source.chars().collect();
        let required_han: Vec<char> = source_chars
            .iter()
            .copied()
            .filter(|c| ('\u{3400}'..='\u{9fff}').contains(c))
            .collect();
        let mixed = source.chars().any(|c| c.is_ascii_alphabetic()) && !required_han.is_empty();
        let badge_word =
            regions.len() == 1 && one_printed_line && matches!(source.as_str(), "注意" | "说明");
        let hint = regions[i].box_2d;
        let best = prepared
            .iter()
            .filter(|(_, _, r)| !taken.iter().any(|t| overlaps(*t, *r)))
            .filter_map(|(chars, han, r)| {
                let text_score = similarity_chars(&source_chars, chars);
                // An English product name must not outweigh missing Chinese
                // words. Otherwise "MapStudioPro 菜单" could match only its
                // logo, erase the logo and leave the actual menu untranslated.
                if mixed && similarity_chars(&required_han, han) < 0.70 {
                    return None;
                }
                if text_score < if badge_word { 0.50 } else { 0.70 } {
                    return None;
                }
                let distance = (hint[0] + hint[2]).abs_diff(r[0] + r[2])
                    + (hint[1] + hint[3]).abs_diff(r[1] + r[3]);
                Some((text_score - distance as f32 / 20000.0, *r))
            })
            .max_by(|a, b| a.0.total_cmp(&b.0));
        let chapter_or_step =
            (source.starts_with('第') && source.ends_with('章')) || source.starts_with("步骤");
        let template_box =
            if best.is_none() && regions.len() == 1 && one_printed_line && chapter_or_step {
                let expected_digits: String = source.chars().filter(char::is_ascii_digit).collect();
                let measured_digits: String = ocr.lines[0]
                    .words
                    .iter()
                    .flat_map(|w| w.text.chars())
                    .filter(char::is_ascii_digit)
                    .collect();
                if !expected_digits.is_empty() && expected_digits == measured_digits {
                    candidates
                        .iter()
                        .max_by_key(|(s, _)| s.chars().count())
                        .map(|(_, r)| (1.0, *r))
                } else {
                    None
                }
            } else {
                None
            };
        if let Some((_, rect)) = best.or(template_box) {
            regions[i].box_2d = rect;
            taken.push(rect);
        } else {
            missing.push(i);
        }
    }
    missing.sort_unstable();
    Ok(missing)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_partial_render_cannot_pass_just_because_its_boxes_cover_the_source() {
        let ocr: OcrDocument = serde_json::from_value(
            serde_json::json!({"width":100,"height":50,"lines":[{"words":[
                {"text":"测试","confidence":98,"bbox":{"x0":10,"y0":10,"x1":30,"y1":30}},
                {"text":"成功","confidence":97,"bbox":{"x0":50,"y0":10,"x1":70,"y1":30}},
                {"text":"口","confidence":99,"bbox":{"x0":80,"y0":10,"x1":90,"y1":30}}
            ]}]}),
        )
        .unwrap();
        let mut source = image::RgbaImage::from_pixel(100, 50, image::Rgba([255, 255, 255, 255]));
        for y in 12..28 {
            for x in [14, 15, 24, 25, 54, 55, 64, 65] {
                source.put_pixel(x, y, image::Rgba([0, 0, 0, 255]));
            }
        }
        let mut target = source.clone();
        assert_eq!(unchanged_han_words(&source, &target, &ocr).len(), 2);
        for y in 10..30 {
            for x in 10..30 {
                target.put_pixel(x, y, image::Rgba([255, 255, 255, 255]));
            }
        }
        let surviving = unchanged_han_words(&source, &target, &ocr);
        assert_eq!(surviving.len(), 1);
        assert_eq!(surviving[0].text, "成功");
        for y in 10..30 {
            for x in 50..70 {
                target.put_pixel(x, y, image::Rgba([255, 255, 255, 255]));
            }
        }
        assert!(unchanged_han_words(&source, &target, &ocr).is_empty());
    }
    #[test]
    fn small_pixel_noise_does_not_count_as_translating_a_label() {
        let ocr: OcrDocument = serde_json::from_value(
            serde_json::json!({"width":20,"height":20,"lines":[{"words":[
                {"text":"标定","confidence":99,"bbox":{"x0":0,"y0":0,"x1":20,"y1":20}}
            ]}]}),
        )
        .unwrap();
        let source = image::RgbaImage::from_pixel(20, 20, image::Rgba([180, 180, 180, 255]));
        let target = image::RgbaImage::from_pixel(20, 20, image::Rgba([184, 184, 184, 255]));
        assert_eq!(unchanged_han_words(&source, &target, &ocr).len(), 1);
    }
    #[test]
    fn an_empty_vision_answer_cannot_hide_reliable_local_chinese() {
        let ocr:OcrDocument=serde_json::from_value(serde_json::json!({"width":1000,"height":1000,"lines":[{"words":[
            {"text":"RCU客户端1.3.10","confidence":98,"bbox":{"x0":20,"y0":10,"x1":180,"y1":30}},
            {"text":"MapStudioPro","confidence":99,"bbox":{"x0":200,"y0":10,"x1":350,"y1":30}},
            {"text":"口","confidence":99,"bbox":{"x0":900,"y0":10,"x1":920,"y1":30}},
            {"text":"不可靠猜测","confidence":35,"bbox":{"x0":20,"y0":50,"x1":150,"y1":70}}
        ]}]})).unwrap();
        let missing = uncovered_han_words(&[], &ocr);
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].text, "RCU客户端1.3.10");
    }
    #[test]
    fn coverage_requires_the_actual_measured_label_instead_of_a_neighbor() {
        let ocr: OcrDocument = serde_json::from_value(
            serde_json::json!({"width":1000,"height":1000,"lines":[{"words":[
                {"text":"客户端","confidence":97,"bbox":{"x0":100,"y0":100,"x1":200,"y1":130}}
            ]}]}),
        )
        .unwrap();
        let mut regions = vec![TextRegion {
            box_2d: [100, 250, 130, 350],
            source_text: "客户端".into(),
            translation: "İstemci".into(),
            text_color_hex: None,
            bg_color_hex: None,
        }];
        assert_eq!(uncovered_han_words(&regions, &ocr).len(), 1);
        regions[0].box_2d = [100, 100, 130, 200];
        assert!(uncovered_han_words(&regions, &ocr).is_empty());
    }
    #[test]
    fn complete_multiline_instructions_join_all_measured_lines_without_the_next_heading() {
        let text = "安装位置\n请放置标定工装\n保持水平\n点击确定开始测试";
        let mut regions = vec![TextRegion {
            box_2d: [20, 20, 500, 800],
            source_text: text.into(),
            translation: "Kalibrasyon talimatı".into(),
            text_color_hex: None,
            bg_color_hex: None,
        }];
        let lines = [
            "安装位置",
            "请放置标定工装",
            "保持水平",
            "点击确定开始测试",
            "下一章节",
        ];
        let ocr = OcrDocument {
            width: 1000,
            height: 1000,
            lines: lines
                .iter()
                .enumerate()
                .map(|(i, t)| OcrLine {
                    words: vec![OcrWord {
                        text: (*t).into(),
                        confidence: None,
                        bbox: OcrBox {
                            x0: 100,
                            y0: 100 + i as u32 * 30,
                            x1: 500,
                            y1: 120 + i as u32 * 30,
                        },
                    }],
                })
                .collect(),
        };
        assert!(localize(&mut regions, &ocr).unwrap().is_empty());
        assert_eq!(regions[0].box_2d, [100, 100, 210, 500]);
    }
    #[test]
    fn an_english_brand_cannot_stand_in_for_missing_chinese_menu_words() {
        let mut regions = vec![TextRegion {
            box_2d: [0, 0, 200, 900],
            source_text: "MapStudioPro 菜单".into(),
            translation: "MapStudioPro Menü".into(),
            text_color_hex: None,
            bg_color_hex: None,
        }];
        let ocr = OcrDocument {
            width: 500,
            height: 100,
            lines: vec![OcrLine {
                words: vec![OcrWord {
                    text: "MapStudioPro".into(),
                    confidence: None,
                    bbox: OcrBox {
                        x0: 10,
                        y0: 5,
                        x1: 150,
                        y1: 20,
                    },
                }],
            }],
        };
        assert_eq!(localize(&mut regions, &ocr).unwrap(), vec![0]);
    }
    #[test]
    fn local_contrast_copy_makes_both_dark_and_light_label_ink_dark() {
        let mut source = image::RgbaImage::from_pixel(128, 64, image::Rgba([240, 240, 240, 255]));
        for y in 0..64 {
            for x in 0..64 {
                source.put_pixel(x, y, image::Rgba([20, 20, 20, 255]));
            }
        }
        for y in 15..35 {
            for x in 15..30 {
                source.put_pixel(x, y, image::Rgba([90, 90, 90, 255]));
                source.put_pixel(x + 64, y, image::Rgba([80, 80, 80, 255]));
            }
        }
        let normalized = contrast_copy(&source);
        assert_eq!(normalized.get_pixel(20, 20)[0], 0);
        assert_eq!(normalized.get_pixel(84, 20)[0], 0);
        assert_eq!(normalized.get_pixel(2, 2)[0], 255);
        assert_eq!(source.get_pixel(20, 20)[0], 90);
    }
    #[test]
    fn single_chapter_badge_uses_measured_glyphs_when_ocr_misreads_decorative_han() {
        let mut regions = vec![TextRegion {
            box_2d: [147, 23, 850, 976],
            source_text: "第2章".into(),
            translation: "Bölüm 2".into(),
            text_color_hex: None,
            bg_color_hex: None,
        }];
        let ocr = OcrDocument {
            width: 624,
            height: 244,
            lines: vec![OcrLine {
                words: vec![
                    OcrWord {
                        text: "笺".into(),
                        confidence: None,
                        bbox: OcrBox {
                            x0: 7,
                            y0: 6,
                            x1: 620,
                            y1: 240,
                        },
                    },
                    OcrWord {
                        text: "2".into(),
                        confidence: None,
                        bbox: OcrBox {
                            x0: 292,
                            y0: 0,
                            x1: 461,
                            y1: 244,
                        },
                    },
                    OcrWord {
                        text: "茨".into(),
                        confidence: None,
                        bbox: OcrBox {
                            x0: 460,
                            y0: 0,
                            x1: 613,
                            y1: 244,
                        },
                    },
                ],
            }],
        };
        assert!(localize(&mut regions, &ocr).unwrap().is_empty());
        assert_eq!(regions[0].box_2d, [0, 11, 1000, 994]);
        regions[0].source_text = "第3章".into();
        assert_eq!(localize(&mut regions, &ocr).unwrap(), vec![0]);
    }
    #[test]
    fn measured_words_correct_a_shifted_vision_box_without_losing_the_number() {
        let mut regions = vec![TextRegion {
            box_2d: [237, 154, 272, 221],
            source_text: "间距0.5m".into(),
            translation: "Aralık 0.5m".into(),
            text_color_hex: None,
            bg_color_hex: None,
        }];
        let ocr = OcrDocument {
            width: 844,
            height: 513,
            lines: vec![OcrLine {
                words: vec![
                    OcrWord {
                        text: "间距".into(),
                        confidence: None,
                        bbox: OcrBox {
                            x0: 144,
                            y0: 140,
                            x1: 185,
                            y1: 163,
                        },
                    },
                    OcrWord {
                        text: "0.5m".into(),
                        confidence: None,
                        bbox: OcrBox {
                            x0: 185,
                            y0: 140,
                            x1: 249,
                            y1: 163,
                        },
                    },
                ],
            }],
        };
        assert!(localize(&mut regions, &ocr).unwrap().is_empty());
        assert_eq!(regions[0].box_2d, [272, 170, 318, 296]);
        assert_eq!(regions[0].translation, "Aralık 0.5m");
        regions[0].source_text = "未找到的注释".into();
        assert_eq!(localize(&mut regions, &ocr).unwrap(), vec![0]);
    }
}

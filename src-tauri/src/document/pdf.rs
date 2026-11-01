use log::{info, warn};
use pdfium_render::prelude::*;
use std::path::{Path, PathBuf};

#[path = "pdf_blocks.rs"]
mod blocks;
pub use blocks::*;

// Must match pdfium-render 0.8.37 (`pdfium_7543`). A newer bblanchon tag (e.g.
// chromium/7843) binds but then hangs or crashes inside Pdfium API calls.
const PDFIUM_RELEASE: &str = "chromium/7543";

/// Ensure that the PDFium dynamic library is present in the App Data folder.
/// If not, downloads it based on OS and architecture from bblanchon/pdfium-binaries
/// and extracts it.
pub fn ensure_pdfium_library(app_handle: &tauri::AppHandle) -> Result<PathBuf, String> {
    let config_dir = dirs::config_dir().ok_or("Could not find user config directory")?;
    let app_dir = config_dir.join(&app_handle.config().tauri.bundle.identifier);
    std::fs::create_dir_all(&app_dir).map_err(|e| e.to_string())?;

    let lib_name = if cfg!(target_os = "windows") {
        "pdfium.dll"
    } else if cfg!(target_os = "macos") {
        "libpdfium.dylib"
    } else {
        "libpdfium.so"
    };

    let lib_path = app_dir.join(lib_name);
    let tag_path = app_dir.join("pdfium.release");
    let tag_ok = std::fs::read_to_string(&tag_path)
        .map(|s| s.trim() == PDFIUM_RELEASE)
        .unwrap_or(false);

    if lib_path.exists() && tag_ok {
        return Ok(lib_path);
    }

    if lib_path.exists() {
        warn!(
            "Removing stale PDFium library at {:?} (release tag mismatch or missing tag)",
            lib_path
        );
        std::fs::remove_file(&lib_path).map_err(|e| e.to_string())?;
    }

    info!(
        "PDFium dynamic library ({}) not found. Fetching from bblanchon/pdfium-binaries {}...",
        lib_name, PDFIUM_RELEASE
    );
    download_pdfium(&app_dir, lib_name, &lib_path)?;
    std::fs::write(&tag_path, PDFIUM_RELEASE).map_err(|e| e.to_string())?;

    Ok(lib_path)
}

fn download_pdfium(_app_dir: &Path, lib_name: &str, lib_path: &Path) -> Result<(), String> {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;

    let asset_name = match (os, arch) {
        ("windows", "x86") => "pdfium-win-x86.tgz",
        ("windows", "x86_64") => "pdfium-win-x64.tgz",
        ("windows", "aarch64") => "pdfium-win-arm64.tgz",
        ("macos", "x86_64") => "pdfium-mac-x64.tgz",
        ("macos", "aarch64") => "pdfium-mac-arm64.tgz",
        ("linux", "x86") => "pdfium-linux-x86.tgz",
        ("linux", "x86_64") => "pdfium-linux-x64.tgz",
        ("linux", "aarch64") => "pdfium-linux-arm64.tgz",
        ("linux", "arm") => "pdfium-linux-arm.tgz",
        _ => {
            return Err(format!(
                "Unsupported platform/architecture: {}-{}",
                os, arch
            ))
        }
    };

    let release_slug = PDFIUM_RELEASE.replace('/', "%2F");
    let url = format!(
        "https://github.com/bblanchon/pdfium-binaries/releases/download/{}/{}",
        release_slug, asset_name
    );

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .map_err(|e| e.to_string())?;

    info!("Downloading PDFium from {}...", url);
    let response = client.get(&url).send().map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!(
            "Failed to download PDFium: HTTP {}",
            response.status()
        ));
    }

    let bytes = response.bytes().map_err(|e| e.to_string())?.to_vec();
    info!(
        "Download complete. Size: {} bytes. Extracting dynamic library...",
        bytes.len()
    );

    if asset_name.ends_with(".zip") {
        let reader = std::io::Cursor::new(bytes);
        let mut archive = zip::ZipArchive::new(reader).map_err(|e| e.to_string())?;
        let mut found = false;

        for i in 0..archive.len() {
            let mut file = archive.by_index(i).map_err(|e| e.to_string())?;
            if file.name().ends_with(lib_name) {
                let mut out_file = std::fs::File::create(lib_path).map_err(|e| e.to_string())?;
                std::io::copy(&mut file, &mut out_file).map_err(|e| e.to_string())?;
                found = true;
                break;
            }
        }

        if !found {
            return Err(format!(
                "Could not find {} inside downloaded zip archive",
                lib_name
            ));
        }
    } else {
        // Extract tgz (tar + gzip)
        let tar_bytes = flate2::read::GzDecoder::new(std::io::Cursor::new(bytes));
        let mut archive = tar::Archive::new(tar_bytes);
        let mut found = false;

        for entry in archive.entries().map_err(|e| e.to_string())? {
            let mut entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path().map_err(|e| e.to_string())?;
            if path.to_string_lossy().ends_with(lib_name) {
                // Ensure parent directory exists for unpack
                if let Some(parent) = lib_path.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
                // Unpack expects a folder or takes the file path directly.
                // We'll write to a temp location or directly write the file bytes.
                let mut out_file = std::fs::File::create(lib_path).map_err(|e| e.to_string())?;
                std::io::copy(&mut entry, &mut out_file).map_err(|e| e.to_string())?;
                found = true;
                break;
            }
        }

        if !found {
            return Err(format!(
                "Could not find {} inside downloaded tgz archive",
                lib_name
            ));
        }
    }

    info!(
        "PDFium dynamic library extracted successfully to {:?}",
        lib_path
    );
    Ok(())
}

/// Extract all pages of text from the PDF file at path.
#[tauri::command]
pub fn extract_pdf_pages(
    app_handle: tauri::AppHandle,
    path: String,
) -> Result<Vec<String>, String> {
    let lib_path = ensure_pdfium_library(&app_handle)?;
    let pdfium = Pdfium::new(
        Pdfium::bind_to_library(lib_path.to_str().ok_or("Invalid PDFium library path")?)
            .map_err(|e| e.to_string())?,
    );

    let doc = pdfium
        .load_pdf_from_file(&path, None)
        .map_err(|e| e.to_string())?;
    let mut pages_text = Vec::new();

    for page in doc.pages().iter() {
        let page_text = extract_page_text_with_column_split(&page)?;
        pages_text.push(page_text);
    }

    Ok(pages_text)
}

pub(crate) fn extract_page_text_with_column_split(page: &PdfPage) -> Result<String, String> {
    let text_obj = page.text().map_err(|e| e.to_string())?;
    let rules = collect_rules(page);
    // A span wider than this is a page frame, not a table column.
    let max_cell_width = page.width().value * MAX_CELL_WIDTH_RATIO;
    let mut result = String::new();
    let mut prev_char: Option<(char, f32, f32, f32, f32)> = None;
    let mut pending_newline = false;

    for ch in text_obj.chars().iter() {
        let uc = ch.unicode_char().unwrap_or('\u{FFFD}');
        let bounds = match ch.loose_bounds() {
            Ok(b) => b,
            Err(_) => {
                result.push(uc);
                prev_char = None;
                continue;
            }
        };

        let l = bounds.left().value;
        let b = bounds.bottom().value;
        let r = bounds.right().value;
        let t = bounds.top().value;
        let h = t - b;
        let w = r - l;

        // Hold pdfium's own line breaks until we can see the character that
        // follows: inside a table cell they usually mean "this cell's text
        // wrapped", not "a new block starts".
        if uc == '\n' || uc == '\r' {
            pending_newline = true;
            continue;
        }

        // Only real glyphs decide where a block ends. Spaces are often reported
        // with degenerate (zero) bounds, which would look like a huge backwards
        // jump and split a cell's text apart mid-word. A blank also must not
        // consume a pending line break — the next real glyph resolves that.
        if uc.is_whitespace() || w <= 0.0 || h <= 0.0 {
            result.push(uc);
            continue;
        }

        if let Some((_prev_uc, prev_l, prev_b, prev_r, prev_t)) = prev_char {
            let prev = Glyph { l: prev_l, b: prev_b, r: prev_r, t: prev_t };
            let cur = Glyph { l, b, r, t };
            let same_cell = in_same_cell(&rules, prev.centre(), cur.centre(), max_cell_width);

            if is_block_break(&rules, &prev, &cur, max_cell_width) {
                if !result.ends_with("\n\n") {
                    if result.ends_with('\n') {
                        result.push('\n');
                    } else {
                        result.push_str("\n\n");
                    }
                }
                pending_newline = false;
            } else if pending_newline {
                // Same cell, so this is wrapped text rather than a new block.
                // Keeping the break here is what shattered a narrow cell into
                // per-fragment translation units that were then drawn on top of
                // one another. Outside a table the break is preserved.
                result.push(if same_cell { ' ' } else { '\n' });
                pending_newline = false;
            }
        } else if pending_newline {
            result.push('\n');
            pending_newline = false;
        }

        result.push(uc);
        prev_char = Some((uc, l, b, r, t));
    }

    Ok(result)
}

/// Widest a rule-bracketed span may be, as a fraction of the page width, and
/// still count as a table column rather than a page frame.
const MAX_CELL_WIDTH_RATIO: f32 = 0.6;
/// Minimum height for a drawn path to count as a column rule, in points.
const MIN_RULE_HEIGHT: f32 = 5.0;
/// Widest a path may be and still be a rule rather than a filled shape.
const MAX_RULE_WIDTH: f32 = 3.0;
/// Character gap a rule must sit in before it splits cells, so a rule that
/// merely passes near a word cannot break it apart.
const MIN_RULE_GAP: f32 = 2.0;
/// Tolerance when asking whether a rule reaches a coordinate.
const RULE_SPAN_SLACK: f32 = 1.0;

/// One drawn rule: its position on one axis and the extent it covers on the
/// other. The extent matters — without it a table rule near the BOTTOM of a
/// page was allowed to split a sentence near the top, because only the x
/// positions were ever compared.
#[derive(Clone, Copy)]
struct Rule {
    pos: f32,
    from: f32,
    to: f32,
}

impl Rule {
    /// Whether the rule actually reaches this coordinate on its long axis.
    fn spans(&self, v: f32) -> bool {
        v >= self.from - RULE_SPAN_SLACK && v <= self.to + RULE_SPAN_SLACK
    }
}

/// The rules drawn on a page — the grid a table's cells sit in. Thin paths
/// contribute their centre line; wider filled shapes (a shaded header cell, a
/// bordered box) contribute their edges, which are cell boundaries too.
///
/// BOTH axes matter: with only the vertical rules, two cells stacked in the
/// same column are indistinguishable from one cell, and a column of a legend
/// table collapsed into a single translation unit.
#[derive(Default)]
struct PageRules {
    vertical: Vec<Rule>,
    horizontal: Vec<Rule>,
}

fn collect_rules(page: &PdfPage) -> PageRules {
    let mut rules = PageRules::default();
    for object in page.objects().iter() {
        if object.object_type() != PdfPageObjectType::Path {
            continue;
        }
        // A path's bounding rectangle is not necessarily a drawn rectangle:
        // outlined watermark glyphs previously became phantom table borders.
        if !rectilinear_path(&object) { continue; }
        let Ok(b) = object.bounds() else { continue };
        let (left, right) = (b.left().value, b.right().value);
        let (bottom, top) = (b.bottom().value, b.top().value);
        let (width, height) = (right - left, top - bottom);

        if height >= MIN_RULE_HEIGHT && width <= MAX_RULE_WIDTH {
            rules.vertical.push(Rule { pos: (left + right) / 2.0, from: bottom, to: top });
        } else if width >= MIN_RULE_HEIGHT && height <= MAX_RULE_WIDTH {
            rules.horizontal.push(Rule { pos: (bottom + top) / 2.0, from: left, to: right });
        } else if width >= MIN_RULE_HEIGHT && height >= MIN_RULE_HEIGHT {
            for pos in [left, right] {
                rules.vertical.push(Rule { pos, from: bottom, to: top });
            }
            for pos in [bottom, top] {
                rules.horizontal.push(Rule { pos, from: left, to: right });
            }
        }
    }
    rules
}

fn rectilinear_path(object: &PdfPageObject) -> bool {
    let Some(path)=object.as_path_object() else {return false;};
    let Ok(matrix)=object.matrix() else {return false;};
    let mut previous: Option<(f32,f32)>=None;
    let mut count=0;
    for segment in path.segments().transform(matrix).iter() {
        count+=1;
        if count>6 || matches!(segment.segment_type(),PdfPathSegmentType::BezierTo | PdfPathSegmentType::Unknown) {return false;}
        let point=(segment.x().value,segment.y().value);
        if segment.segment_type()==PdfPathSegmentType::LineTo {
            if let Some(prev)=previous {if (point.0-prev.0).abs()>0.3 && (point.1-prev.1).abs()>0.3 {return false;}}
        }
        previous=Some(point);
    }
    count>=2
}

/// One laid-out glyph's box, in PDF points.
struct Glyph {
    l: f32,
    b: f32,
    r: f32,
    t: f32,
}

impl Glyph {
    /// Centre point, used to decide which table cell the glyph belongs to.
    fn centre(&self) -> (f32, f32) {
        ((self.l + self.r) / 2.0, (self.b + self.t) / 2.0)
    }
}

/// Whether a block boundary belongs between two consecutive glyphs.
///
/// Table cells are the organising principle here. The drawn rules say where a
/// cell ends, so:
///   * a rule between the glyphs is always a boundary, whatever their
///     baselines — cells in one row centre their own contents and routinely
///     sit half a line apart, which the baseline-based gap test cannot see;
///   * inside ONE cell neither a wide gap nor a backwards jump is a boundary —
///     a narrow cell sets its text spread out ("充 电" with a 20.9 pt gap
///     against a 13.7 pt threshold) and wraps back to the cell's left edge,
///     and treating either as a boundary shattered the cell into one
///     translation unit per fragment;
///   * outside a table (no bracketing rules) both fall back to the original
///     heuristics, so ordinary and multi-column text are unaffected.
fn is_block_break(rules: &PageRules, prev: &Glyph, cur: &Glyph, max_cell_width: f32) -> bool {
    let font_height = (prev.t - prev.b).max(cur.t - cur.b).max(1.0);
    let vertical_diff = ((cur.t + cur.b) / 2.0 - (prev.t + prev.b) / 2.0).abs();
    let horiz_gap = cur.l - prev.r;
    let same_cell = in_same_cell(rules, prev.centre(), cur.centre(), max_cell_width);

    if has_rule_between(&rules.vertical, prev.r, cur.l, cur.centre().1) {
        return true;
    }
    if same_cell {
        return false;
    }
    let wide_gap = vertical_diff < font_height * 0.4 && horiz_gap > font_height * 1.5;
    let jumped_back = horiz_gap < -font_height;
    wide_gap || jumped_back
}

/// The pair of rules immediately bracketing `x` — the cell it sits in. Returns
/// None when `x` is not enclosed on both sides, i.e. it is not inside a table.
fn cell_span(rules: &[Rule], x: f32, y: f32) -> Option<(f32, f32)> {
    let reaching = || rules.iter().filter(move |r| r.spans(y)).map(|r| r.pos);
    let left = reaching().filter(|&p| p <= x).fold(f32::NEG_INFINITY, f32::max);
    let right = reaching().filter(|&p| p >= x).fold(f32::INFINITY, f32::min);
    (left.is_finite() && right.is_finite()).then_some((left, right))
}

/// Whether two glyph positions sit inside the same table cell. Outside a table
/// (no bracketing rules) this is false, so ordinary text keeps its line breaks.
///
/// A cell is bounded on all four sides, so BOTH axes are checked: the two
/// positions must share a column AND have no rule drawn between them
/// vertically. Checking only the column made a legend table's stacked cells
/// look like one cell, so four rows merged into a single translation unit and
/// only the first of them was ever translated.
///
/// `max_cell_width` rejects a span too wide to be a column: many pages draw a
/// frame around the whole text area, whose edges bracket every line on the page.
fn in_same_cell(rules: &PageRules, a: (f32, f32), b: (f32, f32), max_cell_width: f32) -> bool {
    let (Some(sa), Some(sb)) = (
        cell_span(&rules.vertical, a.0, a.1),
        cell_span(&rules.vertical, b.0, b.1),
    ) else {
        return false;
    };
    if sa.1 - sa.0 > max_cell_width
        || (sa.0 - sb.0).abs() >= 0.5
        || (sa.1 - sb.1).abs() >= 0.5
    {
        return false;
    }
    !has_rule_between(&rules.horizontal, a.1, b.1, (a.0 + b.0) / 2.0)
}

/// Whether a column rule falls in the gap between two characters.
/// Whether a rule is drawn across the span between two coordinates AND
/// actually reaches `across` on its long axis.
fn has_rule_between(rules: &[Rule], from: f32, to: f32, across: f32) -> bool {
    let (lo, hi) = if from <= to { (from, to) } else { (to, from) };
    if hi - lo < MIN_RULE_GAP {
        return false;
    }
    rules.iter().any(|r| r.pos > lo && r.pos < hi && r.spans(across))
}

/// Render a single PDF page to a cache PNG file for OCR processing, returning the path.
#[tauri::command]
pub fn render_pdf_page(
    app_handle: tauri::AppHandle,
    path: String,
    page_index: usize,
) -> Result<String, String> {
    let lib_path = ensure_pdfium_library(&app_handle)?;
    let pdfium = Pdfium::new(
        Pdfium::bind_to_library(lib_path.to_str().ok_or("Invalid PDFium library path")?)
            .map_err(|e| e.to_string())?,
    );

    let doc = pdfium
        .load_pdf_from_file(&path, None)
        .map_err(|e| e.to_string())?;
    if page_index >= doc.pages().len() as usize {
        return Err(format!(
            "Page index out of bounds: {} / {}",
            page_index,
            doc.pages().len()
        ));
    }
    let page = doc
        .pages()
        .get(page_index as u16)
        .map_err(|e| e.to_string())?;

    let render_config = PdfRenderConfig::new().set_target_width(1200);
    let bitmap = page
        .render_with_config(&render_config)
        .map_err(|e| e.to_string())?;
    let image = bitmap.as_image();

    let mut cache_dir_path = dirs::cache_dir().ok_or("Could not find cache directory")?;
    cache_dir_path.push(&app_handle.config().tauri.bundle.identifier);
    std::fs::create_dir_all(&cache_dir_path).map_err(|e| e.to_string())?;

    let cache_filename = format!("pdf_page_{}.png", page_index);
    let cache_file_path = cache_dir_path.join(cache_filename);

    image.save(&cache_file_path).map_err(|e| e.to_string())?;

    Ok(cache_file_path.to_string_lossy().to_string())
}

#[cfg(test)]
mod tests {
    use super::{cell_span, has_rule_between, in_same_cell, is_block_break, Glyph, PageRules, Rule,
        MIN_RULE_GAP};

    /// A rule spanning the whole page in its long axis — the common case in the
    /// tests below, where only the crossing position matters.
    fn rule(pos: f32) -> Rule {
        Rule { pos, from: -1.0e6, to: 1.0e6 }
    }

    /// The rules measured on page 20/21 of the user's charger manual, plus a
    /// row rule above and below the band those measurements came from.
    fn table_rules() -> PageRules {
        PageRules {
            vertical: [154.0, 226.6, 276.4, 460.0].map(rule).to_vec(),
            horizontal: [780.0, 100.0].map(rule).to_vec(),
        }
    }
    /// A4 width; cells must be narrower than 60% of it.
    const PAGE_W: f32 = 595.4 * 0.6;

    fn glyph(l: f32, r: f32, y: f32) -> Glyph {
        Glyph { l, b: y - 4.55, r, t: y + 4.55 }
    }

    #[test]
    fn neighbouring_cells_break_even_on_different_baselines() {
        // '输入单相三线' ends at 208.4 (y-centre 602.9); '4000W' starts at 231.9
        // (y-centre 608.0) with the table rule at 226.6 between them. The 5.1 pt
        // baseline difference made the old same-line test skip the check, so the
        // two cells became ONE translation unit.
        let prev = glyph(199.0, 208.4, 602.9);
        let cur = glyph(231.9, 237.0, 608.0);
        assert!(is_block_break(&table_rules(), &prev, &cur, PAGE_W));
    }

    #[test]
    fn a_spread_out_cell_is_not_broken_apart() {
        // '充' and '电' inside one cell: same baseline, 20.9 pt apart, against a
        // 13.7 pt threshold. Splitting here shattered the cell into fragments
        // that were translated — and then drawn — separately.
        let prev = glyph(231.9, 241.1, 745.3);
        let cur = glyph(262.0, 271.1, 745.3);
        assert!(!is_block_break(&table_rules(), &prev, &cur, PAGE_W));
    }

    #[test]
    fn wrapping_inside_a_cell_is_not_a_new_block() {
        // '电' (ends 271.1) wraps to '柜' at the cell's left edge (231.9) on the
        // next line: a 39.2 pt backwards jump that is just a line wrap.
        let prev = glyph(262.0, 271.1, 745.3);
        let cur = glyph(231.9, 241.1, 732.6);
        assert!(!is_block_break(&table_rules(), &prev, &cur, PAGE_W));
    }

    #[test]
    fn a_new_table_row_breaks_on_the_backwards_jump() {
        // '侧充' in the 位置 column (ends 455.9) is followed by the next row's
        // first cell at 154.1 — a 301.8 pt jump back, across cells.
        let prev = glyph(446.0, 455.9, 193.3);
        let cur = glyph(154.1, 160.0, 198.6);
        assert!(is_block_break(&table_rules(), &prev, &cur, PAGE_W));
    }

    #[test]
    fn without_rules_the_original_gap_heuristic_still_splits_columns() {
        // A page with no table: two columns separated by a wide gutter must
        // still split, and ordinary letter spacing must not.
        let prev = glyph(200.0, 208.0, 400.0);
        let far = glyph(260.0, 268.0, 400.0);
        assert!(is_block_break(&PageRules::default(), &prev, &far, PAGE_W));
        let near = glyph(210.0, 218.0, 400.0);
        assert!(!is_block_break(&PageRules::default(), &prev, &near, PAGE_W));
    }

    #[test]
    fn cell_span_brackets_a_position_between_rules() {
        let rules = [56.0, 150.0, 226.6, 300.0].map(rule);
        assert_eq!(cell_span(&rules, 200.0, 400.0), Some((150.0, 226.6)));
        // Outside the outermost rules there is no enclosing cell.
        assert_eq!(cell_span(&rules, 20.0, 400.0), None);
        assert_eq!(cell_span(&rules, 500.0, 400.0), None);

        // A rule that does not reach this height cannot bracket anything: a
        // table low on the page must not split a sentence above it.
        let short = [Rule { pos: 150.0, from: 100.0, to: 200.0 }];
        assert_eq!(cell_span(&short, 160.0, 400.0), None);
    }

    #[test]
    fn same_cell_detects_wrapped_text_inside_one_cell() {
        let rules = PageRules {
            vertical: [56.0, 150.0, 226.6, 300.0].map(rule).to_vec(),
            horizontal: [500.0, 400.0].map(rule).to_vec(),
        };
        // Two wrapped lines of one cell (same column, same row band).
        assert!(in_same_cell(&rules, (160.0, 450.0), (180.0, 440.0), PAGE_W));
        // Neighbouring cells in the same row.
        assert!(!in_same_cell(&rules, (160.0, 450.0), (260.0, 450.0), PAGE_W));
    }

    #[test]
    fn stacked_cells_in_one_column_are_not_the_same_cell() {
        // The legend table defect: a column's cells share their vertical rules,
        // so checking only the column merged four rows into ONE unit and left
        // all but the first untranslated. A row rule separates them.
        let rules = PageRules {
            vertical: [56.0, 150.0, 300.0].map(rule).to_vec(),
            horizontal: [500.0, 440.0, 380.0].map(rule).to_vec(),
        };
        assert!(!in_same_cell(&rules, (200.0, 460.0), (200.0, 400.0), PAGE_W));
        // …while two lines within one row band still belong together.
        assert!(in_same_cell(&rules, (200.0, 460.0), (200.0, 450.0), PAGE_W));
    }

    #[test]
    fn outside_a_table_nothing_is_the_same_cell() {
        // Body text must keep its own line breaks, so a page without rules
        // never reports two positions as sharing a cell.
        let none = PageRules::default();
        assert!(!in_same_cell(&none, (100.0, 400.0), (120.0, 400.0), PAGE_W));
        // Only one rule on the page: not enclosed on both sides.
        let one = PageRules { vertical: vec![rule(100.0)], horizontal: Vec::new() };
        assert!(!in_same_cell(&one, (120.0, 400.0), (140.0, 400.0), PAGE_W));
    }

    #[test]
    fn rule_between_cells_is_detected_regardless_of_baseline() {
        // Measured from the user's spec table: the 输入参数 cell ends at x=208.4,
        // the 输入功率 cell starts at x=231.9, and the table rule sits at 226.6.
        // The two cells' baselines differ by half a line because each cell
        // centres its own content, so the baseline-based gap heuristic never
        // fired and the cells were merged into ONE translation unit.
        let rules = [226.6, 276.3].map(rule);
        assert!(has_rule_between(&rules, 208.4, 231.9, 600.0));

        // The measured false split: path edges at x≈257 belong to a table down
        // at y=348..365, and must not break a sentence at y≈476.
        let elsewhere = [Rule { pos: 257.1, from: 348.1, to: 364.9 }];
        assert!(!has_rule_between(&elsewhere, 255.4, 258.3, 476.0));
    }

    #[test]
    fn rule_outside_the_gap_does_not_split() {
        let rules = [rule(226.6)];
        // Rule sits to the right of both characters.
        assert!(!has_rule_between(&rules, 100.0, 150.0, 600.0));
        // Rule exactly on a character edge is not "between" them.
        assert!(!has_rule_between(&rules, 226.6, 240.0, 600.0));
        assert!(!has_rule_between(&rules, 200.0, 226.6, 600.0));
    }

    #[test]
    fn a_rule_cannot_break_a_word_apart() {
        // Normal inter-letter spacing must never split, even if some path
        // edge happens to fall inside it.
        let tight = 0.5 * MIN_RULE_GAP;
        let rules = [rule(100.0 + tight / 2.0)];
        assert!(!has_rule_between(&rules, 100.0, 100.0 + tight, 600.0));
    }

    #[test]
    fn no_rules_means_no_split() {
        assert!(!has_rule_between(&[], 100.0, 200.0, 600.0));
    }

    #[test]
    fn test_platform_naming() {
        let os = std::env::consts::OS;
        let lib_name = if os == "windows" {
            "pdfium.dll"
        } else if os == "macos" {
            "libpdfium.dylib"
        } else {
            "libpdfium.so"
        };
        assert!(lib_name.contains("pdfium"));
    }
}

//! Legacy heuristic planner regression coverage. Production uses canonical blocks.
use super::*;

// Shrink-to-fit bounds (PDF points).
const MIN_FONT_PT: f32 = 4.0;
const MAX_FONT_PT: f32 = 48.0;
const LINE_FACTOR: f32 = 1.18;
/// Fallback when pdfium reports no usable font size for a run.
const DEFAULT_FONT_PT: f32 = 11.0;
/// The translation is drawn at the SOURCE font size whenever it fits. When it
/// does not, shrinking stops at this fraction of the source size so a block can
/// never collapse into unreadable 4pt text while its neighbours stay large.
const MIN_SHRINK_RATIO: f32 = 0.72;
/// Style (weight/colour) is sampled from at most this many characters per run.
const STYLE_SAMPLE_CHARS: usize = 24;
/// Stroke width used to emulate bold, as a fraction of the font size.
const BOLD_STROKE_RATIO: f32 = 0.035;
/// Thickest a drawn box may be and still be read as a rule between rows.
const MAX_RULE_THICKNESS: f32 = 3.0;
/// Vertical gap, in line heights, beyond which two rows of one match are
/// separate places on the page rather than consecutive wrapped lines.
const MAX_WRAP_GAP_LINES: f32 = 1.2;
/// Pixels per PDF point when rendering a page to recover text backdrops.
/// Low on purpose — only a flat colour is read back from it.
const BACKDROP_RENDER_SCALE: f32 = 1.0;
/// RGB distance within which a sampled pixel counts as ink rather than
/// backdrop. Generous enough to swallow anti-aliased glyph edges.
const INK_EXCLUSION_DISTANCE: f32 = 120.0;
/// A backdrop must differ from the ink by at least this much; anything closer
/// means the sample was still dominated by glyphs and is discarded.
const MIN_BACKDROP_CONTRAST: f32 = 90.0;
/// Blocks narrower than this fraction of the text column are treated as table
/// cells / labels: they may shrink further to stay inside their cell, and they
/// are left out of the page-wide size unification.
const NARROW_BOX_RATIO: f32 = 0.55;
/// Shrink floor for those narrow blocks — overflowing a cell is worse than
/// small text, because the overflow lands on top of the neighbouring cell.
const MIN_SHRINK_RATIO_NARROW: f32 = 0.5;
// Padding added around a redaction box so anti-aliased glyph edges of the
// original text are fully covered.
const REDACT_PAD: f32 = 1.25;

// ── Geometry planning ────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct Bbox {
    left: f32,
    bottom: f32,
    right: f32,
    top: f32,
}

impl Bbox {
    fn width(&self) -> f32 {
        (self.right - self.left).max(0.0)
    }
    fn height(&self) -> f32 {
        (self.top - self.bottom).max(0.0)
    }
}

/// Horizontal alignment inferred from where the source block sits inside the
/// page's text column. A centered title redrawn flush-left is one of the most
/// obvious "this document was machine-translated" tells, so it is preserved.
/// Right-alignment is deliberately absent: outside page furniture it is rare,
/// and inferring it from page margins misfires on table cells (which then get
/// their text pushed to the cell's right edge). Left is the safe fallback.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Align {
    Left,
    Center,
}

fn spread(values: &[f32]) -> f32 {
    let max = values.iter().copied().fold(f32::MIN, f32::max);
    let min = values.iter().copied().fold(f32::MAX, f32::min);
    max - min
}

struct ParagraphEdit {
    /// Box the translation is laid out in — may extend into verified-free
    /// space to the right/below so the source font size can be preserved.
    bbox: Bbox,
    /// Box painted white to hide the source glyphs. Always the original text
    /// union, never the grown layout box.
    redact: Bbox,
    size: f32,
    lines: Vec<String>,
    /// Source text fill color, applied to the redrawn translation so colored
    /// headings/captions keep their color. Near-white colors fall back to
    /// black at draw time (the redaction backing box is white).
    color: PdfColor,
    /// Source text was bold. The bundled face has no bold variant, so bold is
    /// emulated by stroking the glyph outline (FilledThenStroked).
    bold: bool,
    align: Align,
    /// The font size this block had in the source. Kept so `unify_sizes` can
    /// give every block of the same source size the same final size.
    source_size: f32,
    /// The drawable translation, retained so the block can be re-wrapped when
    /// `unify_sizes` changes its size.
    text: String,
    /// Table cell / label rather than body text — sized independently so one
    /// cramped cell cannot shrink a whole page of body copy.
    narrow: bool,
    /// What is painted behind the source text, used for the redaction fill so
    /// a shaded table header stays shaded.
    backdrop: PdfColor,
}

/// One matched source LINE of a paragraph: its geometry plus the text
/// attributes sampled from the source characters.
struct LineRec {
    orig_idx: usize,
    line_idx: usize,
    /// False for pieces of a table-like run split across visual rows — those
    /// must never be glued back into a paragraph.
    groupable: bool,
    /// False for source lines with no translation (pass-through code, blocks
    /// the model left alone). They are still located, because they occupy
    /// space: they must block a neighbour's box from growing over them.
    drawable: bool,
    /// A table-of-contents entry ("Heading …….. 12"). Its dot leader runs to
    /// the column edge, which would otherwise look exactly like a wrapped
    /// line and chain every TOC row into one block.
    is_toc: bool,
    bbox: Bbox,
    size: f32,
    color: PdfColor,
    bold: bool,
}

/// Three or more leader characters in a row mark a table-of-contents entry.
fn looks_like_toc_line(text: &str) -> bool {
    let mut run = 0usize;
    for c in text.chars() {
        if matches!(c, '.' | '·' | '⋅' | '…' | '・' | '‧') {
            run += 1;
            if run >= 3 {
                return true;
            }
        } else if !c.is_whitespace() {
            run = 0;
        }
    }
    false
}

// Build the list of (redaction box + styled translated lines) for one page.
//
// Three phases:
//   1. Locate every source LINE in the page char stream and sample its real
//      text attributes (font size, fill color, boldness) from the matched
//      characters — never guessed from bounding-box heights.
//   2. Re-group consecutive lines that are wrapped continuations of the same
//      source paragraph back into one block. Without this a wrapped tail like
//      "上）。" gets its own narrow box and the translation is shrunk to an
//      unreadable size while the line above it stays large.
//   3. Lay the translation out at the SOURCE font size, using free space to
//      the right / below when the translation is longer than the source (very
//      common: Turkish runs much longer than Chinese). Shrinking is the last
//      resort and is floored relative to the source size.
fn plan_page_edits(
    page: &PdfPage,
    metrics: &FontMetrics,
    paragraphs: &[LayoutParagraph],
    progress: &mut dyn FnMut(&str),
) -> Result<Vec<ParagraphEdit>, String> {
    let expansion = expand_layout_paragraphs(paragraphs);
    let text = page.text().map_err(|e| e.to_string())?;
    let page_all = text.all();
    let mut slow_geometry: Option<PageGeometry> = None;
    let mut from_byte = 0usize;
    let total = expansion.lines.len();

    // Drawn geometry (cell rules, callout borders) is needed from Phase 1 on:
    // it tells wrapped text inside ONE cell apart from a run spanning cells.
    let obstacles = collect_obstacles(page);

    // ── Phase 1 — match each source line, sample its style ───────────────────
    let mut recs: Vec<LineRec> = Vec::new();
    for (pi, line) in expansion.lines.iter().enumerate() {
        if pi % 3 == 0 {
            progress(&format!("Matching translated block {} of {}…", pi + 1, total));
        }

        let original = line.original.trim();
        if original.is_empty() {
            continue;
        }
        // Untranslated lines (pass-through code, blocks the model returned
        // unchanged) are located too but never drawn: they still occupy space,
        // and a neighbouring block must not grow its box over them.
        let para_translated = expansion.full[line.orig_idx].trim();
        let drawable = !para_translated.is_empty()
            && para_translated != expansion.full_source[line.orig_idx].trim();

        let (start_char, end_char) = if let Some((start_byte, end_byte)) =
            find_run(&page_all, original, from_byte)
        {
            from_byte = end_byte;
            char_range_for_byte_span(&page_all, start_byte, end_byte)
        } else if let Some((start_byte, end_byte)) = find_run(&page_all, original, 0) {
            from_byte = end_byte;
            char_range_for_byte_span(&page_all, start_byte, end_byte)
        } else {
            if slow_geometry.is_none() {
                progress("Building fallback character map (slow path)…");
                slow_geometry = Some(PageGeometry::build(&text, progress)?);
            }
            match slow_geometry.as_mut().unwrap().locate(original) {
                Some(range) => range,
                None => {
                    warn!("Could not locate paragraph in page text: {:?}", original);
                    continue;
                }
            }
        };

        // ONE pass over the matched characters collects geometry AND style.
        let scan = scan_char_range(&text, start_char, end_char);
        let Some(bbox) = scan.bbox() else { continue };
        if bbox.width() <= 1.0 || bbox.height() <= 1.0 {
            continue;
        }

        // A matched run spanning several visual rows is table-like (pdfium
        // merges adjacent cells). Split it per row and mark the pieces
        // ungroupable so table cells never get glued back together.
        // A match covering several visual rows is a table run — UNLESS every
        // row sits inside the same drawn cell, in which case it is simply that
        // cell's text wrapping and must stay one block. Splitting it made each
        // row ask for the same unit's translation, which is how one sentence
        // ended up repeated down the whole page.
        let rows = scan.split_rows(&bbox);
        let rows = if rows.len() > 1 && !rows_need_splitting(&obstacles, &rows) {
            vec![bbox.clone()]
        } else {
            rows
        };
        let multi_row = rows.len() > 1;
        let is_toc = looks_like_toc_line(original);
        for row in rows {
            recs.push(LineRec {
                orig_idx: line.orig_idx,
                line_idx: line.line_idx,
                groupable: !multi_row && drawable,
                drawable,
                is_toc,
                bbox: row,
                size: scan.size,
                color: scan.color,
                bold: scan.bold,
            });
        }
    }

    // ── Phase 2 — group wrapped continuation lines back together ─────────────
    // Reading order (top-to-bottom, then left-to-right) is what makes adjacency
    // meaningful; matches arrive in text order, but sorting makes it explicit.
    recs.sort_by(|a, b| {
        b.bbox
            .top
            .partial_cmp(&a.bbox.top)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(
                a.bbox
                    .left
                    .partial_cmp(&b.bbox.left)
                    .unwrap_or(std::cmp::Ordering::Equal),
            )
    });
    let column = TextColumn::of(&recs, page.width().value);
    let groups = group_wrapped_lines(&recs, column.right, &obstacles);
    let backdrop = PageBackdrop::render(page);
    let column_width = (column.right - column.left).max(1.0);

    // ── Phase 3 — lay out each block at the source font size ─────────────────
    let mut edits = Vec::new();
    // A unit's whole translation may be drawn ONCE. Without this, a match that
    // splits across several boxes drew the entire paragraph into every one of
    // them — the same sentence repeated down the page.
    let mut placed = Placed::default();
    for group in &groups {
        let members: Vec<&LineRec> = group.iter().map(|&i| &recs[i]).collect();
        let first = members[0];
        if !first.drawable {
            continue;
        }

        let clean = drawable_translation(
            group_text(&expansion, &members, &recs, &mut placed).trim(),
            metrics,
        );
        if clean.is_empty() {
            continue;
        }

        let redact = union_of(members.iter().map(|m| m.bbox.clone()));
        let align = column.align_of(&members, &redact, first.size);
        // Width the translation needs to stay on one line at the source size.
        let needed_width = metrics.text_w(&clean, first.size);
        let layout = grow_layout_box(
            &redact,
            &recs,
            group,
            &obstacles,
            &column,
            first.size,
            align,
            needed_width,
        );
        let narrow = layout.width() < NARROW_BOX_RATIO * column_width;
        let floor_ratio = if narrow {
            MIN_SHRINK_RATIO_NARROW
        } else {
            MIN_SHRINK_RATIO
        };
        // Standalone body lines (headings, captions) keep their single-line
        // shape; inside a narrow cell wrapping is normal and expected.
        let keep_on_one_line = members.len() == 1 && !narrow;
        let (size, lines) = fit_text(
            metrics,
            &clean,
            layout.width(),
            layout.height(),
            first.size,
            floor_ratio,
            keep_on_one_line,
        );
        if lines.is_empty() {
            continue;
        }
        let fill = backdrop
            .as_ref()
            .and_then(|b| b.background_color(&redact, &first.color))
            .unwrap_or(PdfColor::WHITE);
        edits.push(ParagraphEdit {
            bbox: layout,
            redact,
            size,
            lines,
            color: first.color,
            bold: first.bold,
            align,
            source_size: first.size,
            text: clean,
            narrow,
            backdrop: fill,
        });
    }

    unify_sizes(&mut edits, metrics);
    Ok(edits)
}

/// Resolve the translated text for one group of source lines.
///
/// `used_whole` records the units whose complete translation has already been
/// placed, so it can never be drawn a second time somewhere else on the page.
fn group_text(
    expansion: &Expansion,
    members: &[&LineRec],
    recs: &[LineRec],
    placed: &mut Placed,
) -> String {
    let first = members[0];
    // When the group covers every matched line of a single multi-line unit, the
    // whole-unit translation flows better than re-joining per-line slices.
    if members.iter().all(|m| m.orig_idx == first.orig_idx) {
        let matched_lines = recs.iter().filter(|r| r.orig_idx == first.orig_idx).count();
        if first.groupable && members.len() == matched_lines {
            return placed.take_whole(expansion, first.orig_idx);
        }
    }

    let mut parts: Vec<String> = Vec::new();
    let mut seen: Vec<(usize, usize)> = Vec::new();
    for m in members {
        // Pieces of a row-split match share a line identity — take it once.
        if seen.contains(&(m.orig_idx, m.line_idx)) {
            continue;
        }
        seen.push((m.orig_idx, m.line_idx));
        let text = placed.take_line(expansion, m.orig_idx, m.line_idx);
        if !text.trim().is_empty() {
            parts.push(text.trim().to_string());
        }
    }
    parts.join(" ")
}

/// Tracks which translations have already been placed on the page.
///
/// The invariant is simply: **every source line's translation is drawn exactly
/// once**. Without it, a match that split across several boxes handed the same
/// text to each of them and the paragraph was repeated down the page.
#[derive(Default)]
struct Placed {
    whole: Vec<usize>,
    lines: Vec<(usize, usize)>,
}

impl Placed {
    /// A unit's complete translation, the first time it is asked for.
    fn take_whole(&mut self, expansion: &Expansion, orig_idx: usize) -> String {
        if self.whole.contains(&orig_idx) {
            return String::new();
        }
        self.whole.push(orig_idx);
        expansion.full.get(orig_idx).cloned().unwrap_or_default()
    }

    /// One line's slice, unless that line — or the whole unit it belongs to —
    /// has already been placed.
    fn take_line(&mut self, expansion: &Expansion, orig_idx: usize, line_idx: usize) -> String {
        if self.whole.contains(&orig_idx) || self.lines.contains(&(orig_idx, line_idx)) {
            return String::new();
        }
        match expansion.line_text(orig_idx, line_idx) {
            Some(text) => {
                self.lines.push((orig_idx, line_idx));
                text
            }
            // No per-line slice: the unit's whole translation belongs to its
            // first line, and only there.
            None if line_idx == 0 => self.take_whole(expansion, orig_idx),
            None => String::new(),
        }
    }
}

/// The page's text column, derived from every matched line on it, plus the
/// physical page centre — which, unlike the text column, cannot be skewed by
/// an unusual page (a contents page is indented, so its column centre sits
/// well to the right of the real page centre).
struct TextColumn {
    left: f32,
    right: f32,
    page_center: f32,
    page_width: f32,
}

impl TextColumn {
    fn of(recs: &[LineRec], page_width: f32) -> Self {
        Self {
            left: recs.iter().map(|r| r.bbox.left).fold(f32::MAX, f32::min),
            right: recs.iter().map(|r| r.bbox.right).fold(f32::MIN, f32::max),
            page_center: page_width / 2.0,
            page_width,
        }
    }

    /// Infer alignment for a block. Left is the default and the fallback: on a
    /// table page the whole-page column is only a rough guide (a cell in the
    /// right-hand column looks "right-aligned" by margins alone even though its
    /// text is flush left), so centring must be positively evidenced.
    ///
    /// A multi-line block proves itself centred by having its lines share a
    /// centre axis while their left edges differ. A single line has no such
    /// evidence, so it must sit inside generous and near-equal margins.
    fn align_of(&self, members: &[&LineRec], bbox: &Bbox, size: f32) -> Align {
        if members.len() > 1 {
            let centres: Vec<f32> = members
                .iter()
                .map(|m| (m.bbox.left + m.bbox.right) / 2.0)
                .collect();
            let lefts: Vec<f32> = members.iter().map(|m| m.bbox.left).collect();
            return if spread(&centres) < 0.5 * size && spread(&lefts) > 1.5 * size {
                Align::Center
            } else {
                Align::Left
            };
        }

        // A single line proves nothing on its own, so the evidence has to be
        // physical: it sits on the PAGE's centre line with real margins on
        // both sides. Measuring against the text column instead would misjudge
        // a page whose body is indented (a contents page).
        let block_center = (bbox.left + bbox.right) / 2.0;
        let centered_on_page =
            (block_center - self.page_center).abs() <= 0.02 * self.page_width.max(1.0);
        let margins = 2.0 * size;
        if centered_on_page
            && bbox.left - self.left > margins
            && self.right - bbox.right > margins
        {
            Align::Center
        } else {
            Align::Left
        }
    }
}

/// Give every block that shared a source font size the same final size, so a
/// paragraph can never be drawn part-large and part-small. The class takes the
/// smallest size any of its members needed — shrinking a block that already fit
/// is always safe, and uniformity is what reads as "only the language changed".
fn unify_sizes(edits: &mut [ParagraphEdit], metrics: &FontMetrics) {
    // Table cells and body copy are unified SEPARATELY (the class key carries
    // the `narrow` flag). Mixing them would let one cramped cell shrink a whole
    // page of body copy; keeping cells out entirely — as an earlier version did
    // — left every cell sized on its own, which is what made table text jump
    // between sizes from line to line.
    let mut classes: Vec<(f32, bool, f32)> = Vec::new(); // (source size, narrow, min scale)
    for e in edits.iter() {
        let scale = e.size / e.source_size.max(0.01);
        match classes
            .iter_mut()
            .find(|(s, n, _)| *n == e.narrow && same_size_class(*s, e.source_size))
        {
            Some((_, _, min)) => *min = min.min(scale),
            None => classes.push((e.source_size, e.narrow, scale)),
        }
    }

    for e in edits.iter_mut() {
        let Some((_, _, scale)) = classes
            .iter()
            .find(|(s, n, _)| *n == e.narrow && same_size_class(*s, e.source_size))
        else {
            continue;
        };
        let target = (e.source_size * scale).max(MIN_FONT_PT);
        if (target - e.size).abs() > 0.01 {
            e.size = target;
            e.lines = wrap_paragraph(metrics, &e.text, e.bbox.width().max(1.0), target);
        }
    }
}

fn same_size_class(a: f32, b: f32) -> bool {
    (a - b).abs() < 0.5
}

/// Merge consecutive same-paragraph lines that are wrapped continuations.
/// Returns groups of indices into `recs`, preserving order.
fn group_wrapped_lines(recs: &[LineRec], page_right: f32, obstacles: &[Bbox]) -> Vec<Vec<usize>> {
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for i in 0..recs.len() {
        let joined = groups
            .last_mut()
            .filter(|g| {
                let prev = &recs[*g.last().expect("group is never empty")];
                is_wrapped_continuation(prev, &recs[i], page_right, obstacles)
            })
            .map(|g| g.push(i))
            .is_some();
        if !joined {
            groups.push(vec![i]);
        }
    }
    groups
}

/// True when `next` is the wrapped continuation of `prev` rather than a
/// separate block (e.g. the next row of a table, or a following heading).
///
/// The test is purely GEOMETRIC and deliberately does not require the two
/// lines to come from the same translation unit: the frontend splits every
/// multi-line block into one unit per source line, so visually-wrapped lines
/// almost always arrive as separate units. Grouping them by paragraph identity
/// would therefore never fire on a real document.
///
/// The decisive signal is that `prev` runs to the text column's right edge —
/// a line only wraps because it ran out of horizontal room. Table rows,
/// headings and paragraph-final lines all stop well short of that edge, which
/// is what keeps them from being glued to what follows.
fn is_wrapped_continuation(
    prev: &LineRec,
    next: &LineRec,
    page_right: f32,
    obstacles: &[Bbox],
) -> bool {
    if !prev.groupable || !next.groupable {
        return false;
    }
    // Anything the page DRAWS between two lines — a cell rule, a callout
    // border — ends the block, whatever the geometry suggests. Cells in the
    // rightmost column reach the page edge and so look exactly like wrapped
    // lines; only the rule between them tells the rows apart. Checking for an
    // enclosing box alone was not enough: these tables draw their grid as
    // separate segments, so no box encloses anything.
    if prev.orig_idx != next.orig_idx
        && rows_are_separated(obstacles, &prev.bbox, &next.bbox)
    {
        return false;
    }
    // A TOC row is a complete entry whose dot leader reaches the column edge.
    // Without this every row of a contents page chains into one block and the
    // whole table of contents is redrawn as a single run-on paragraph.
    if prev.is_toc || next.is_toc {
        return false;
    }
    let size = prev.size.max(1.0);
    // Comparable type size.
    let ratio = next.size / size;
    if !(0.75..=1.34).contains(&ratio) {
        return false;
    }
    // Directly on the following baseline, not further down the page.
    let gap = prev.bbox.bottom - next.bbox.top;
    if !(-0.6 * size..=1.2 * size).contains(&gap) {
        return false;
    }
    // Same left margin (tolerates a small hanging indent).
    if (next.bbox.left - prev.bbox.left).abs() > 3.0 * size {
        return false;
    }
    // `prev` filled the column — the hallmark of a wrapped line.
    prev.bbox.right >= page_right - 4.0 * size
}

/// Grow the layout box into free space so a longer translation can keep the
/// source font size: rightwards to the column edge and downwards into the gap
/// before the next block. Blocked by any other matched block in the way, and
/// clamped by any drawn box that encloses this one (a table cell's rules keep
/// the cell's translation inside the cell).
///
/// Only the ORIGINAL union is redacted (see `ParagraphEdit::redact`); this
/// larger box is used purely for text layout, so growing it can never paint
/// over content we did not verify as free.
fn grow_layout_box(
    redact: &Bbox,
    recs: &[LineRec],
    group: &[usize],
    obstacles: &[Bbox],
    column: &TextColumn,
    size: f32,
    align: Align,
    needed_width: f32,
) -> Bbox {
    let mut out = redact.clone();
    let in_group = |i: usize| group.contains(&i);

    // Drawn geometry (cell rules, callout boxes) constrains growth: a box that
    // ENCLOSES this block is its container and clamps it inside; one that sits
    // alongside blocks growth at its edge.
    let mut right_limit = column.right.max(redact.right);
    let mut bottom_floor = f32::MIN;
    let mut left_floor = f32::MIN;
    for b in obstacles {
        if encloses(b, redact) {
            right_limit = right_limit.min(b.right - 0.5 * size);
            bottom_floor = bottom_floor.max(b.bottom + 0.3 * size);
            left_floor = left_floor.max(b.left + 0.5 * size);
        } else if b.left >= redact.right && vertical_overlap(b, redact, size) {
            right_limit = right_limit.min(b.left - 0.5 * size);
        }
    }

    // Rightwards: up to the column edge, stopped by any block that shares this
    // vertical band and sits to our right.
    for (i, r) in recs.iter().enumerate() {
        if in_group(i) || r.bbox.left <= redact.right {
            continue;
        }
        if vertical_overlap(&r.bbox, redact, size) {
            right_limit = right_limit.min(r.bbox.left - 0.5 * size);
        }
    }
    out.right = out.right.max(right_limit.max(redact.right));

    // Blocks may also reclaim room on their LEFT. A centred title has free
    // margin on both sides; a heading that follows a chapter number has a gap
    // between the two. A left-aligned block does this only when it must —
    // otherwise its left edge, which carries the page's indentation structure,
    // stays exactly where the source put it.
    let needs_more_room = out.right - out.left < needed_width;
    if align != Align::Left || needs_more_room {
        let mut left_limit = column.left.min(redact.left).max(left_floor);
        for (i, r) in recs.iter().enumerate() {
            if in_group(i) || r.bbox.right >= redact.left {
                continue;
            }
            if vertical_overlap(&r.bbox, redact, size) {
                left_limit = left_limit.max(r.bbox.right + 0.5 * size);
            }
        }
        for b in obstacles {
            if !encloses(b, redact) && b.right <= redact.left && vertical_overlap(b, redact, size) {
                left_limit = left_limit.max(b.right + 0.5 * size);
            }
        }
        out.left = out.left.min(left_limit).min(redact.left);
        // Keep a centered block symmetric about its original centre so the
        // text stays visually centred after it is redrawn.
        // Keep a centred block symmetric about its original centre — but only
        // while that still leaves room for the text. A heading whose left side
        // is hemmed in by a neighbour (a chapter number, say) would otherwise
        // be squeezed into a narrow symmetric box and wrap, when there is free
        // space to its right.
        if align == Align::Center {
            let centre = (redact.left + redact.right) / 2.0;
            let reach = (centre - out.left).min(out.right - centre);
            if reach * 2.0 >= needed_width {
                out.left = centre - reach;
                out.right = centre + reach;
            }
        }
    }

    // Downwards: at most two extra lines, stopped by the next block below that
    // shares this horizontal band, and by any enclosing drawn box.
    let mut bottom_limit = (redact.bottom - 2.0 * size * LINE_FACTOR).max(bottom_floor);
    for (i, r) in recs.iter().enumerate() {
        if in_group(i) || r.bbox.top >= redact.bottom {
            continue;
        }
        if horizontal_overlap(&r.bbox, redact, out.right, size) {
            bottom_limit = bottom_limit.max(r.bbox.top + 0.3 * size);
        }
    }
    for b in obstacles {
        if !encloses(b, redact)
            && b.top <= redact.bottom
            && horizontal_overlap(b, redact, out.right, size)
        {
            bottom_limit = bottom_limit.max(b.top + 0.3 * size);
        }
    }
    out.bottom = out.bottom.min(bottom_limit.min(redact.bottom));
    out
}

/// Whether a multi-row match really covers separate places on the page, rather
/// than the wrapped lines of one block.
///
/// Splitting is now the exception: cells are separated at EXTRACTION (each cell
/// is its own translation unit), so a single unit covering several rows is
/// almost always wrapped text. Splitting it anyway gave one row the whole
/// translation and left the others with no text — and therefore no redaction,
/// so the untranslated source stayed visible underneath.
fn rows_need_splitting(obstacles: &[Bbox], rows: &[Bbox]) -> bool {
    rows.windows(2)
        .any(|pair| rows_are_separated(obstacles, &pair[0], &pair[1]))
}

/// Two stacked row boxes are separate places when they sit in different drawn
/// cells, or when a rule is drawn across the gap between them.
fn rows_are_separated(obstacles: &[Bbox], upper: &Bbox, lower: &Bbox) -> bool {
    // Wrapped lines follow each other immediately. A large vertical gap means
    // the match reached over something else — a figure, another block — and
    // unioning across it painted a redaction box over whatever sat between.
    let line_height = upper.height().max(lower.height()).max(1.0);
    if upper.bottom - lower.top > line_height * MAX_WRAP_GAP_LINES {
        return true;
    }
    match (
        smallest_container(obstacles, upper),
        smallest_container(obstacles, lower),
    ) {
        (Some(a), Some(b)) => return !same_box(a, b),
        // One inside a drawn box and the other outside it — a callout and the
        // line beneath it, say. Different places by definition.
        (Some(_), None) | (None, Some(_)) => return true,
        (None, None) => {}
    }
    obstacles.iter().any(|o| {
        o.height() <= MAX_RULE_THICKNESS
            && o.top <= upper.bottom + 1.0
            && o.bottom >= lower.top - 1.0
            && o.left <= upper.left + 1.0
            && o.right >= upper.right - 1.0
    })
}

fn same_box(a: &Bbox, b: &Bbox) -> bool {
    (a.left - b.left).abs() < 0.5
        && (a.right - b.right).abs() < 0.5
        && (a.top - b.top).abs() < 0.5
        && (a.bottom - b.bottom).abs() < 0.5
}

/// The tightest drawn box enclosing `inner`, if any.
fn smallest_container<'a>(obstacles: &'a [Bbox], inner: &Bbox) -> Option<&'a Bbox> {
    obstacles
        .iter()
        .filter(|o| encloses(o, inner))
        .min_by(|a, b| {
            (a.width() * a.height())
                .partial_cmp(&(b.width() * b.height()))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
}

/// True when `outer` surrounds `inner` — the relationship between a table
/// cell's rules (or a callout border) and the text inside it.
fn encloses(outer: &Bbox, inner: &Bbox) -> bool {
    outer.left <= inner.left + 1.0
        && outer.right >= inner.right - 1.0
        && outer.bottom <= inner.bottom + 1.0
        && outer.top >= inner.top - 1.0
        && (outer.width() > inner.width() + 1.0 || outer.height() > inner.height() + 1.0)
}

fn vertical_overlap(a: &Bbox, b: &Bbox, size: f32) -> bool {
    a.top > b.bottom + 0.2 * size && a.bottom < b.top - 0.2 * size
}

fn horizontal_overlap(a: &Bbox, b: &Bbox, right: f32, size: f32) -> bool {
    a.right > b.left + 0.2 * size && a.left < right - 0.2 * size
}

/// Bounding boxes of the page's NON-text objects (cell rules, callout
/// borders, images). They are what visually delimits a table cell, so they are
/// the only reliable way to keep a cell's translation inside its cell.
fn collect_obstacles(page: &PdfPage) -> Vec<Bbox> {
    let mut out = Vec::new();
    for object in page.objects().iter() {
        if object.object_type() == PdfPageObjectType::Text {
            continue;
        }
        let Ok(b) = object.bounds() else { continue };
        let bbox = Bbox {
            left: b.left().value,
            bottom: b.bottom().value,
            right: b.right().value,
            top: b.top().value,
        };
        // Hairlines have no area but still delimit cells; only skip degenerate
        // boxes in BOTH axes.
        if bbox.width() <= 0.5 && bbox.height() <= 0.5 {
            continue;
        }
        out.push(bbox);
    }
    out
}

/// A rendered copy of the page, used to recover what is actually painted
/// behind a block of text. Redacting with a flat white box destroys shaded
/// table headers and tinted callouts; sampling the real backdrop keeps them.
struct PageBackdrop {
    rgba: Vec<u8>,
    width: usize,
    height: usize,
    scale: f32,
    page_height: f32,
}

impl PageBackdrop {
    fn render(page: &PdfPage) -> Option<Self> {
        let scale = BACKDROP_RENDER_SCALE;
        let config = PdfRenderConfig::new().scale_page_by_factor(scale);
        let bitmap = page.render_with_config(&config).ok()?;
        Some(Self {
            width: bitmap.width() as usize,
            height: bitmap.height() as usize,
            rgba: bitmap.as_rgba_bytes(),
            scale,
            page_height: page.height().value,
        })
    }

    /// The colour painted behind `bbox`, given the colour its text is drawn in.
    ///
    /// Taking the plain mode of the region is NOT enough: dense CJK glyphs —
    /// especially bold headings and table header rows — cover more than half of
    /// their own box, so the most common colour there is the ink, and the
    /// redaction box gets painted black over the text it was meant to hide.
    /// Pixels close to the text colour are therefore discarded first, and the
    /// result is rejected unless it actually contrasts with that text.
    fn background_color(&self, bbox: &Bbox, text_color: &PdfColor) -> Option<PdfColor> {
        let x0 = ((bbox.left * self.scale).floor().max(0.0)) as usize;
        let x1 = ((bbox.right * self.scale).ceil()).clamp(0.0, self.width as f32) as usize;
        let y0 = (((self.page_height - bbox.top) * self.scale).floor().max(0.0)) as usize;
        let y1 = (((self.page_height - bbox.bottom) * self.scale).ceil())
            .clamp(0.0, self.height as f32) as usize;
        if x1 <= x0 || y1 <= y0 {
            return None;
        }

        let mut samples: Vec<(u8, u8, u8)> = Vec::new();
        for y in y0..y1.min(self.height) {
            for x in x0..x1.min(self.width) {
                let i = (y * self.width + x) * 4;
                if let Some(px) = self.rgba.get(i..i + 3) {
                    samples.push((px[0], px[1], px[2]));
                }
            }
        }
        background_from_samples(&samples, text_color)
    }
}

/// Pick the backdrop out of a block's pixels: drop everything close to the ink
/// colour (the glyphs and their anti-aliased fringes), then take the most
/// common remaining colour. Returns None when nothing usable is left or the
/// winner would be invisible against the text.
pub(crate) fn background_from_samples(
    samples: &[(u8, u8, u8)],
    text_color: &PdfColor,
) -> Option<PdfColor> {
    let ink = (text_color.red(), text_color.green(), text_color.blue());
    // Quantise to 32 levels per channel so anti-aliasing does not split one
    // background into dozens of near-identical buckets.
    let mut buckets: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();
    for px in samples {
        if color_distance(*px, ink) < INK_EXCLUSION_DISTANCE {
            continue;
        }
        let key = ((px.0 as u32 >> 3) << 10) | ((px.1 as u32 >> 3) << 5) | (px.2 as u32 >> 3);
        *buckets.entry(key).or_insert(0) += 1;
    }
    let (key, _) = buckets.into_iter().max_by_key(|(_, n)| *n)?;
    let expand = |v: u32| ((v << 3) | (v >> 2)) as u8;
    let color = (
        expand((key >> 10) & 0x1F),
        expand((key >> 5) & 0x1F),
        expand(key & 0x1F),
    );
    // The source text was legible, so its backdrop must contrast with it. A
    // low-contrast winner means the sample was dominated by ink after all.
    if color_distance(color, ink) < MIN_BACKDROP_CONTRAST {
        return None;
    }
    Some(PdfColor::new(color.0, color.1, color.2, 255))
}

fn color_distance(a: (u8, u8, u8), b: (u8, u8, u8)) -> f32 {
    let dr = a.0 as f32 - b.0 as f32;
    let dg = a.1 as f32 - b.1 as f32;
    let db = a.2 as f32 - b.2 as f32;
    (dr * dr + dg * dg + db * db).sqrt()
}

fn union_of(boxes: impl Iterator<Item = Bbox>) -> Bbox {
    let mut acc: Option<Bbox> = None;
    for b in boxes {
        acc = Some(match acc {
            None => b,
            Some(a) => Bbox {
                left: a.left.min(b.left),
                bottom: a.bottom.min(b.bottom),
                right: a.right.max(b.right),
                top: a.top.max(b.top),
            },
        });
    }
    acc.unwrap_or(Bbox {
        left: 0.0,
        bottom: 0.0,
        right: 0.0,
        top: 0.0,
    })
}

struct PageGeometry {
    matchable: Vec<MatchableChar>,
    matchable_str: String,
    super_matchable: Vec<MatchableChar>,
    super_matchable_str: String,
    cursor_matchable_idx: usize,
    cursor_super_idx: usize,
}

#[derive(Debug, Clone)]
struct MatchableChar {
    original_idx: usize,
    c: char,
}

impl PageGeometry {
    fn build(text: &PdfPageText, progress: &mut dyn FnMut(&str)) -> Result<Self, String> {
        let mut full_chars = Vec::new();
        for (i, ch) in text.chars().iter().enumerate() {
            if i > 0 && i % 800 == 0 {
                progress(&format!("Reading glyph geometry ({i} chars)…"));
            }
            full_chars.push(ch.unicode_char().unwrap_or('\u{FFFD}'));
        }

        fn is_alpha_num_cjk(c: char) -> bool {
            c.is_alphanumeric()
                || (c >= '\u{3000}' && c <= '\u{9FFF}')
                || (c >= '\u{F900}' && c <= '\u{FAFF}')
        }

        let mut matchable = Vec::new();
        let mut super_matchable = Vec::new();
        for (ci, &c) in full_chars.iter().enumerate() {
            if !c.is_whitespace() {
                if let Some(nc) = normalize_match_char(c) {
                    matchable.push(MatchableChar {
                        original_idx: ci,
                        c: nc,
                    });
                }
            }
            if is_alpha_num_cjk(c) {
                if let Some(nc) = normalize_match_char(c) {
                    super_matchable.push(MatchableChar {
                        original_idx: ci,
                        c: nc,
                    });
                }
            }
        }

        Ok(Self {
            matchable_str: matchable.iter().map(|mc| mc.c).collect(),
            super_matchable_str: super_matchable.iter().map(|mc| mc.c).collect(),
            matchable,
            super_matchable,
            cursor_matchable_idx: 0,
            cursor_super_idx: 0,
        })
    }

    fn locate(&mut self, original: &str) -> Option<(usize, usize)> {
        let original_clean: String = original.chars().filter_map(normalize_match_char).collect();
        if original_clean.is_empty() {
            return None;
        }

        let mut found_range = None;
        if let Some(rel_idx) = self.matchable_str
            [self.cursor_matchable_idx.min(self.matchable_str.len())..]
            .find(&original_clean)
        {
            let start = self.cursor_matchable_idx + rel_idx;
            let end = start + original_clean.len();
            found_range = Some((start, end));
            self.cursor_matchable_idx = end;
        } else if let Some(start) = self.matchable_str.find(&original_clean) {
            let end = start + original_clean.len();
            found_range = Some((start, end));
            self.cursor_matchable_idx = end;
        }

        let mut start_super = None;
        if found_range.is_none() {
            let original_super_clean: String = original
                .chars()
                .filter(|&c| {
                    c.is_alphanumeric()
                        || (c >= '\u{3000}' && c <= '\u{9FFF}')
                        || (c >= '\u{F900}' && c <= '\u{FAFF}')
                })
                .filter_map(normalize_match_char)
                .collect();

            if !original_super_clean.is_empty() {
                if let Some(rel_idx) = self.super_matchable_str
                    [self.cursor_super_idx.min(self.super_matchable_str.len())..]
                    .find(&original_super_clean)
                {
                    let start = self.cursor_super_idx + rel_idx;
                    let end = start + original_super_clean.len();
                    start_super = Some((start, end));
                    self.cursor_super_idx = end;
                } else if let Some(start) = self.super_matchable_str.find(&original_super_clean) {
                    let end = start + original_super_clean.len();
                    start_super = Some((start, end));
                    self.cursor_super_idx = end;
                }
            }
        }

        // `find` yields BYTE offsets while `matchable` is indexed per CHARACTER.
        // On CJK text (3 bytes per glyph) the byte offset runs far past the end
        // of the vector, so indexing it directly panicked — which killed the
        // export thread silently: no `done` event was ever emitted and the UI
        // sat on its last progress message forever.
        if let Some((start, end)) = found_range {
            original_idx_range(&self.matchable, &self.matchable_str, start, end)
        } else if let Some((start, end)) = start_super {
            original_idx_range(
                &self.super_matchable,
                &self.super_matchable_str,
                start,
                end,
            )
        } else {
            None
        }
    }
}

/// Map a `[start, end)` BYTE range inside `haystack` to the first and last
/// `original_idx` of the characters it covers. Returns None rather than
/// panicking if the range does not line up with the vector.
fn original_idx_range(
    chars: &[MatchableChar],
    haystack: &str,
    start_byte: usize,
    end_byte: usize,
) -> Option<(usize, usize)> {
    let start_char = char_count_before(haystack, start_byte);
    let end_char = char_count_before(haystack, end_byte);
    if end_char == 0 {
        return None;
    }
    let first = chars.get(start_char)?;
    let last = chars.get(end_char - 1)?;
    Some((first.original_idx, last.original_idx))
}

fn char_count_before(s: &str, byte_idx: usize) -> usize {
    let byte_idx = byte_idx.min(s.len());
    if !s.is_char_boundary(byte_idx) {
        // Should not happen (offsets come from `find`), but never panic on it.
        return s[..byte_idx.saturating_sub(1).min(s.len())]
            .chars()
            .count();
    }
    s[..byte_idx].chars().count()
}

fn char_range_for_byte_span(page_all: &str, start_byte: usize, end_byte: usize) -> (usize, usize) {
    let start_char = page_all[..start_byte.min(page_all.len())].chars().count();
    let end_char = page_all[..end_byte.min(page_all.len())]
        .chars()
        .count()
        .saturating_sub(1);
    (start_char, end_char.max(start_char))
}

/// Geometry + text attributes of one matched character range, collected in a
/// single pass over pdfium (each `loose_bounds()` call is expensive, so the
/// old separate bbox / font-size / row-split passes are folded into this one).
struct CharScan {
    positions: Vec<CharPos>,
    /// Median of the source characters' real font sizes — read from the PDF's
    /// text state via `scaled_font_size()`, NOT inferred from glyph heights.
    size: f32,
    color: PdfColor,
    bold: bool,
}

struct CharPos {
    left: f32,
    bottom: f32,
    right: f32,
    top: f32,
}

impl CharPos {
    fn y_center(&self) -> f32 {
        (self.top + self.bottom) / 2.0
    }
}

impl CharScan {
    fn bbox(&self) -> Option<Bbox> {
        if self.positions.is_empty() {
            return None;
        }
        Some(union_of(self.positions.iter().map(|p| Bbox {
            left: p.left,
            bottom: p.bottom,
            right: p.right,
            top: p.top,
        })))
    }

    /// Cluster the matched characters into visual rows. More than one row means
    /// the run spans a table-like structure rather than a single text line.
    fn split_rows(&self, bbox: &Bbox) -> Vec<Bbox> {
        split_positions_by_rows(&self.positions, bbox, self.size)
    }
}

fn scan_char_range(text: &PdfPageText, start_char: usize, end_char: usize) -> CharScan {
    let mut positions = Vec::new();
    let mut sizes: Vec<f32> = Vec::new();
    let mut bold_votes = 0usize;
    let mut total_votes = 0usize;
    let mut color: Option<PdfColor> = None;

    for (i, ch) in text.chars().iter().enumerate() {
        if i < start_char {
            continue;
        }
        if i > end_char {
            break;
        }
        if let Ok(b) = ch.loose_bounds() {
            let (l, bo, r, t) = (
                b.left().value,
                b.bottom().value,
                b.right().value,
                b.top().value,
            );
            if (r - l) > 0.0 || (t - bo) > 0.0 {
                positions.push(CharPos {
                    left: l,
                    bottom: bo,
                    right: r,
                    top: t,
                });
            }
        }
        let s = ch.scaled_font_size().value;
        if s.is_finite() && s > 0.5 {
            sizes.push(s);
        }
        // Attributes only need a sample; querying every glyph of a long
        // paragraph costs far more than it adds.
        if total_votes < STYLE_SAMPLE_CHARS {
            total_votes += 1;
            if is_bold_weight(&ch) {
                bold_votes += 1;
            }
            if color.is_none() {
                if let Ok(c) = ch.fill_color() {
                    if !is_near_white(&c) {
                        color = Some(c);
                    }
                }
            }
        }
    }

    CharScan {
        size: median_size(&mut sizes),
        color: color.unwrap_or(PdfColor::BLACK),
        bold: total_votes > 0 && bold_votes * 2 > total_votes,
        positions,
    }
}

fn median_size(sizes: &mut Vec<f32>) -> f32 {
    if sizes.is_empty() {
        return DEFAULT_FONT_PT;
    }
    sizes.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    sizes[sizes.len() / 2].clamp(MIN_FONT_PT, MAX_FONT_PT)
}

fn is_bold_weight(ch: &PdfPageTextChar) -> bool {
    use pdfium_render::prelude::PdfFontWeight::*;
    let by_weight = matches!(
        ch.font_weight(),
        Some(Weight600) | Some(Weight700Bold) | Some(Weight800) | Some(Weight900)
    ) || matches!(ch.font_weight(), Some(Custom(w)) if w >= 600);
    by_weight || ch.font_name().to_ascii_lowercase().contains("bold")
}

/// White-on-white would be invisible over the white redaction box, so such a
/// fill is ignored in favour of the black default.
fn is_near_white(c: &PdfColor) -> bool {
    c.red() > 240 && c.green() > 240 && c.blue() > 240
}

/// When a matched paragraph's bounding box spans multiple visual rows (common in
/// table cells where pdfium merges adjacent columns' text), split the bbox into
/// per-row segments by clustering matched characters' y-coordinates. Each cluster
/// becomes a separate (narrower) bbox so the translated text is drawn into the
/// correct cell instead of spanning the entire table width.
///
/// If the bbox looks like a single-row block (height ≤ ~2× the estimated font
/// size), it is returned as-is (vec![bbox]) — no overhead for normal paragraphs.
fn split_positions_by_rows(positions: &[CharPos], bbox: &Bbox, est_font_size: f32) -> Vec<Bbox> {
    // If the bbox height fits ~2 lines of text, no splitting needed.
    let row_threshold = est_font_size * LINE_FACTOR * 2.5;
    if bbox.height() <= row_threshold {
        return vec![bbox.clone()];
    }

    if positions.is_empty() {
        return vec![bbox.clone()];
    }

    // Cluster by y_center: chars whose y_center is within half a font size of
    // the cluster's running average belong to the same row.
    let merge_dist = est_font_size * 0.6;
    // Sort by y_center descending (top-of-page first in PDF coords).
    let mut positions: Vec<&CharPos> = positions.iter().collect();
    positions.sort_by(|a, b| {
        b.y_center()
            .partial_cmp(&a.y_center())
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut clusters: Vec<Vec<&CharPos>> = Vec::new();
    for pos in positions {
        let mut added = false;
        for cluster in clusters.iter_mut() {
            let cluster_y =
                cluster.iter().map(|c| c.y_center()).sum::<f32>() / cluster.len() as f32;
            if (pos.y_center() - cluster_y).abs() <= merge_dist {
                cluster.push(pos);
                added = true;
                break;
            }
        }
        if !added {
            clusters.push(vec![pos]);
        }
    }

    // If we only got one cluster, return the original bbox.
    if clusters.len() <= 1 {
        return vec![bbox.clone()];
    }

    // Build a bbox for each cluster.
    let mut result: Vec<Bbox> = Vec::new();
    for cluster in &clusters {
        let mut cl_bbox = Bbox {
            left: f32::MAX, bottom: f32::MAX, right: f32::MIN, top: f32::MIN,
        };
        for pos in cluster {
            cl_bbox.left = cl_bbox.left.min(pos.left);
            cl_bbox.bottom = cl_bbox.bottom.min(pos.bottom);
            cl_bbox.right = cl_bbox.right.max(pos.right);
            cl_bbox.top = cl_bbox.top.max(pos.top);
        }
        if cl_bbox.width() > 1.0 && cl_bbox.height() > 1.0 {
            result.push(cl_bbox);
        }
    }

    // Sort rows top-to-bottom (descending y in PDF coords = first visually).
    result.sort_by(|a, b| b.top.partial_cmp(&a.top).unwrap_or(std::cmp::Ordering::Equal));

    if result.is_empty() {
        vec![bbox.clone()]
    } else {
        result
    }
}

// Task A2 — turn one translated image-text overlay into a `ParagraphEdit`
// (white backing box + shrink-to-fit lines) anchored to the image's bounding
// box. Reuses the exact same drawing path as the text-layer redaction. Returns
// None for empty translations or degenerate boxes (image left untouched).
fn plan_overlay_edit(metrics: &FontMetrics, ov: &ImageOverlay) -> Option<ParagraphEdit> {
    let translated = ov.translated.trim();
    if translated.is_empty() {
        return None;
    }
    let clean = drawable_translation(translated, metrics);
    if clean.is_empty() {
        return None;
    }
    let bbox = Bbox {
        left: ov.left,
        bottom: ov.bottom,
        right: ov.right,
        top: ov.top,
    };
    if bbox.width() <= 1.0 || bbox.height() <= 1.0 {
        return None;
    }
    // No source glyph metrics for raster text — start from a fraction of the
    // box height and let `fit_text` shrink to fit.
    let est = (bbox.height() * 0.5).clamp(MIN_FONT_PT, MAX_FONT_PT);
    let (size, lines) = fit_text(
        metrics,
        &clean,
        bbox.width(),
        bbox.height(),
        est,
        MIN_SHRINK_RATIO_NARROW,
        false,
    );
    if lines.is_empty() {
        return None;
    }
    Some(ParagraphEdit {
        redact: bbox.clone(),
        bbox,
        size,
        lines,
        // Raster text has no queryable text state — draw it plain black on a
        // white patch, the same as before backdrops were sampled.
        color: PdfColor::BLACK,
        bold: false,
        align: Align::Left,
        source_size: size,
        text: clean,
        // Image overlays are sized on their own, never unified with body copy.
        narrow: true,
        backdrop: PdfColor::WHITE,
    })
}

// Draw each service-translated replacement image at the ORIGINAL image's
// bounding box. The service returns the same pixel dimensions as the input,
// so anchoring at (left, bottom) and scaling to the bbox reproduces the
// original placement exactly. Never fails the export: a missing/corrupt file
// or a pdfium error just leaves that original image in place.
fn apply_image_replacements(page: &mut PdfPage, replacements: &[&ImageReplacement]) {
    for r in replacements {
        let Some((left, bottom, width, height)) = replacement_rect(r) else {
            warn!(
                "Skipping degenerate image replacement on page {} ({})",
                r.page_index, r.image_path
            );
            continue;
        };
        let img = match image::open(&r.image_path) {
            Ok(i) => i,
            Err(e) => {
                warn!("Could not open translated image {}: {e}", r.image_path);
                continue;
            }
        };
        if let Err(e) = page.objects_mut().create_image_object(
            PdfPoints::new(left),
            PdfPoints::new(bottom),
            &img,
            Some(PdfPoints::new(width)),
            Some(PdfPoints::new(height)),
        ) {
            warn!(
                "Placing translated image {} failed: {e} — original kept",
                r.image_path
            );
        }
    }
}

/// Apply the source run's colour and weight to a freshly drawn line. Bold is
/// emulated by stroking the glyph outline — the bundled DejaVu face ships no
/// bold variant and adding one would grow the binary for a cosmetic detail.
/// Style failures are non-fatal: the line is already drawn and readable.
fn apply_text_style(object: &mut PdfPageObject, edit: &ParagraphEdit) {
    if object.set_fill_color(edit.color).is_err() {
        warn!("Could not apply source text colour to a translated line");
    }
    if !edit.bold {
        return;
    }
    let _ = object.set_stroke_color(edit.color);
    let _ = object.set_stroke_width(PdfPoints::new(edit.size * BOLD_STROKE_RATIO));
    if let Some(text_object) = object.as_text_object_mut() {
        let _ = text_object.set_render_mode(PdfPageTextRenderMode::FilledThenStroked);
    }
}

/// Validate one replacement's geometry: returns (left, bottom, width, height)
/// or None for degenerate boxes / empty paths (replacement skipped).
fn replacement_rect(r: &ImageReplacement) -> Option<(f32, f32, f32, f32)> {
    let width = r.right - r.left;
    let height = r.top - r.bottom;
    if !width.is_finite() || !height.is_finite() || width <= 1.0 || height <= 1.0 {
        return None;
    }
    if r.image_path.trim().is_empty() {
        return None;
    }
    Some((r.left, r.bottom, width, height))
}

/// Where one drawn line starts, honouring the source block's alignment.
fn line_start_x(edit: &ParagraphEdit, line: &str, metrics: &FontMetrics) -> f32 {
    match edit.align {
        Align::Left => edit.bbox.left,
        Align::Center => {
            let w = metrics.text_w(line, edit.size);
            (edit.bbox.left + edit.bbox.right - w) / 2.0
        }
    }
}

fn apply_page_edits(
    page: &mut PdfPage,
    font: PdfFontToken,
    metrics: &FontMetrics,
    edits: &[ParagraphEdit],
) -> Result<(), String> {
    {
        let objects = page.objects_mut();
        // Pass 1 — redaction boxes over the ORIGINAL text only (all first so
        // translated text always paints on top). The fill is the colour
        // actually painted behind that text, so shaded table headers and
        // tinted callouts survive instead of turning into white patches.
        for e in edits {
            let rect = PdfRect::new_from_values(
                e.redact.bottom - REDACT_PAD,
                e.redact.left - REDACT_PAD,
                e.redact.top + REDACT_PAD,
                e.redact.right + REDACT_PAD,
            );
            objects
                .create_path_object_rect(rect, None, None, Some(e.backdrop))
                .map_err(|err| format!("redaction rect failed: {err}"))?;
        }
        // Pass 2 — translated lines in the source's size, colour and weight,
        // clipped to the layout box so table cells don't overflow into
        // adjacent rows.
        for e in edits {
            let line_h = e.size * LINE_FACTOR;
            for (k, line) in e.lines.iter().enumerate() {
                if line.trim().is_empty() {
                    continue;
                }
                let baseline_y = e.bbox.top - e.size - (k as f32) * line_h;
                let x = line_start_x(e, line, metrics);
                let mut object = objects
                    .create_text_object(
                        PdfPoints::new(x),
                        PdfPoints::new(baseline_y),
                        line,
                        font,
                        PdfPoints::new(e.size),
                    )
                    .map_err(|err| format!("text object failed: {err}"))?;
                apply_text_style(&mut object, e);
            }
        }
    }
    // Flatten the new objects into the page's content stream before save.
    page.regenerate_content()
        .map_err(|e| format!("regenerate content failed: {e}"))?;
    Ok(())
}

// ── Pure helpers (unit-tested) ───────────────────────────────────────────────

/// Strip PDF private-use bullets and full-width ASCII before case-folding for matching.
fn normalize_match_char(c: char) -> Option<char> {
    if c.is_whitespace() || c == '\u{3000}' {
        return None;
    }
    if ('\u{E000}'..='\u{F8FF}').contains(&c) {
        return None;
    }
    let c = if ('\u{FF01}'..='\u{FF5E}').contains(&c) {
        char::from_u32(c as u32 - 0xFEE0)?
    } else {
        c
    };
    Some(c.to_lowercase().next().unwrap_or(c))
}

/// One source line to locate on the page, tagged with the paragraph it came
/// from so wrapped continuations can be regrouped after matching.
struct ExpandedLine {
    original: String,
    orig_idx: usize,
    line_idx: usize,
}

/// Per-line view of the requested paragraphs, keeping the whole-paragraph
/// texts alongside so a regrouped block can be redrawn as one flowing
/// translation instead of per-line fragments.
struct Expansion {
    lines: Vec<ExpandedLine>,
    full: Vec<String>,
    full_source: Vec<String>,
    /// Per-line translation slices, present only when the translation split
    /// into exactly as many lines as the source did.
    per_line: Vec<Option<Vec<String>>>,
}

impl Expansion {
    fn line_text(&self, orig_idx: usize, line_idx: usize) -> Option<String> {
        self.per_line
            .get(orig_idx)?
            .as_ref()?
            .get(line_idx)
            .cloned()
            .filter(|s| !s.trim().is_empty())
    }
}

/// TOC / list pages often arrive as one giant paragraph — split into lines so
/// each row can be matched, redacted, and redrawn independently. The paragraph
/// identity is preserved so `group_wrapped_lines` can undo the split for lines
/// that are merely wrapped continuations.
fn expand_layout_paragraphs(paragraphs: &[LayoutParagraph]) -> Expansion {
    let mut lines = Vec::new();
    let mut full = Vec::new();
    let mut full_source = Vec::new();
    let mut per_line = Vec::new();

    for para in paragraphs {
        let original = para.original.trim();
        let translated = para.translated.trim();
        let orig_idx = full.len();
        full.push(translated.to_string());
        full_source.push(original.to_string());

        if original.is_empty() {
            per_line.push(None);
            continue;
        }

        let orig_lines: Vec<String> = original
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect();
        let trans_lines: Vec<String> = translated
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect();

        per_line.push(if trans_lines.len() == orig_lines.len() {
            Some(trans_lines)
        } else {
            None
        });

        // Always split multi-line blocks into per-line entries. This is
        // critical for table cells: pdfium merges adjacent cells' text into
        // a single string, and per-line splitting lets each cell/row line
        // be matched and bbox'd independently (prevents cross-cell unions).
        if orig_lines.len() > 1 {
            for (line_idx, orig_line) in orig_lines.into_iter().enumerate() {
                lines.push(ExpandedLine {
                    original: orig_line,
                    orig_idx,
                    line_idx,
                });
            }
        } else {
            lines.push(ExpandedLine {
                original: original.to_string(),
                orig_idx,
                line_idx: 0,
            });
        }
    }

    Expansion {
        lines,
        full,
        full_source,
        per_line,
    }
}

/// Keep only glyphs the embedded font can draw; drop unmappable codepoints instead
/// of stripping whole paragraphs when the model leaves a few source characters.
fn drawable_translation(text: &str, metrics: &FontMetrics) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if ('\u{E000}'..='\u{F8FF}').contains(&c) {
            out.push('•');
        } else if metrics.has_glyph(c) {
            out.push(c);
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ").trim().to_string()
}

#[cfg(test)]
fn union_bbox_range(boxes: &[Bbox], start_char_idx: usize, end_char_idx: usize) -> Option<Bbox> {
    let mut acc: Option<Bbox> = None;
    for ci in start_char_idx..=end_char_idx {
        if let Some(b) = boxes.get(ci) {
            if b.width() <= 0.0 && b.height() <= 0.0 {
                continue;
            }
            acc = Some(match acc {
                None => b.clone(),
                Some(a) => Bbox {
                    left: a.left.min(b.left),
                    bottom: a.bottom.min(b.bottom),
                    right: a.right.max(b.right),
                    top: a.top.max(b.top),
                },
            });
        }
    }
    acc
}

#[cfg(test)]
fn estimate_font_size_range(boxes: &[Bbox], start_char_idx: usize, end_char_idx: usize) -> f32 {
    let mut heights: Vec<f32> = Vec::new();
    for ci in start_char_idx..=end_char_idx {
        if let Some(b) = boxes.get(ci) {
            let h = b.height();
            if h > 0.5 {
                heights.push(h);
            }
        }
    }
    if heights.is_empty() {
        return 11.0;
    }
    heights.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let med = heights[heights.len() / 2];
    med.clamp(MIN_FONT_PT, MAX_FONT_PT)
}

/// Find `needle` (already trimmed) inside `haystack` at or after byte offset
/// `from`. Returns the `[start, end)` byte range, or None.
fn find_run(haystack: &str, needle: &str, from: usize) -> Option<(usize, usize)> {
    if needle.is_empty() {
        return None;
    }
    let from = from.min(haystack.len());
    let slice = &haystack[from..];
    let rel = slice.find(needle)?;
    let start = from + rel;
    Some((start, start + needle.len()))
}

/// Union of the bounding boxes of every char whose byte offset lies in
/// `[start_byte, end_byte)`.
#[cfg(test)]
fn union_bbox(full: &str, boxes: &[Bbox], start_byte: usize, end_byte: usize) -> Option<Bbox> {
    let mut acc: Option<Bbox> = None;
    for (ci, (byte_idx, _)) in full.char_indices().enumerate() {
        if byte_idx < start_byte {
            continue;
        }
        if byte_idx >= end_byte {
            break;
        }
        let Some(b) = boxes.get(ci) else { break };
        // Skip whitespace boxes which pdfium sometimes reports as zero-area.
        if b.width() <= 0.0 && b.height() <= 0.0 {
            continue;
        }
        acc = Some(match acc {
            None => b.clone(),
            Some(a) => Bbox {
                left: a.left.min(b.left),
                bottom: a.bottom.min(b.bottom),
                right: a.right.max(b.right),
                top: a.top.max(b.top),
            },
        });
    }
    acc
}


// Lay the translation out at the SOURCE font size, shrinking only if it still
// does not fit the (already grown) box — and never below `MIN_SHRINK_RATIO` of
// the source size, so a block cannot collapse to unreadable text while its
// neighbours keep their size. Never drops text: at the floor it returns
// whatever wraps, which may overflow slightly (best-effort, per the brief).
fn fit_text(
    m: &FontMetrics,
    text: &str,
    box_w: f32,
    box_h: f32,
    source_size: f32,
    floor_ratio: f32,
    keep_on_one_line: bool,
) -> (f32, Vec<String>) {
    let source_size = source_size.clamp(MIN_FONT_PT, MAX_FONT_PT);
    let floor = (source_size * floor_ratio).max(MIN_FONT_PT);
    let mut size = source_size;
    let w = box_w.max(1.0);
    loop {
        let lines = wrap_paragraph(m, text, w, size);
        let h = lines.len() as f32 * size * LINE_FACTOR;
        // A heading that was ONE line in the source should stay one line: a
        // slightly smaller heading reads far better than one broken in two.
        let acceptable = h <= box_h && (!keep_on_one_line || lines.len() <= 1);
        if acceptable || size <= floor {
            return (size, lines);
        }
        size = (size - 0.25).max(floor);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metrics() -> FontMetrics<'static> {
        FontMetrics::new(LATIN_FONT).unwrap()
    }

    #[test]
    fn find_run_locates_substring_after_cursor() {
        let hay = "alpha beta gamma beta delta";
        // first beta
        let (s1, e1) = find_run(hay, "beta", 0).unwrap();
        assert_eq!(&hay[s1..e1], "beta");
        // second beta is found only when the cursor advances past the first
        let (s2, e2) = find_run(hay, "beta", e1).unwrap();
        assert!(s2 > s1);
        assert_eq!(&hay[s2..e2], "beta");
    }

    #[test]
    fn find_run_missing_returns_none() {
        assert!(find_run("hello world", "xyz", 0).is_none());
        assert!(find_run("hello", "", 0).is_none());
        // cursor past end must not panic
        assert!(find_run("hello", "hello", 999).is_none());
    }

    #[test]
    fn union_bbox_spans_matched_chars() {
        let full = "ab";
        let boxes = vec![
            Bbox { left: 0.0, bottom: 0.0, right: 10.0, top: 12.0 },
            Bbox { left: 10.0, bottom: 0.0, right: 22.0, top: 12.0 },
        ];
        let b = union_bbox(full, &boxes, 0, full.len()).unwrap();
        assert_eq!(b.left, 0.0);
        assert_eq!(b.right, 22.0);
        assert_eq!(b.top, 12.0);
        assert!(b.width() > 0.0 && b.height() > 0.0);
    }

    #[test]
    fn fit_text_shrinks_to_fit_height() {
        let m = metrics();
        let text = "word ".repeat(60);
        // Tall box: keeps the starting size.
        let (big, _) = fit_text(&m, text.trim(), 200.0, 2000.0, 12.0, MIN_SHRINK_RATIO, false);
        // Short box: must shrink below the start.
        let (small, lines) = fit_text(&m, text.trim(), 200.0, 40.0, 12.0, MIN_SHRINK_RATIO, false);
        assert!(small <= big);
        assert!(small >= MIN_FONT_PT);
        assert!(!lines.is_empty(), "must never drop the text entirely");
    }

    #[test]
    fn fit_text_floor_never_drops_text() {
        let m = metrics();
        let source = 12.0;
        let text = "verylongunbreakabletoken".repeat(20);
        let (size, lines) = fit_text(&m, &text, 30.0, 5.0, source, MIN_SHRINK_RATIO, false);
        // Bottoms out at the source-relative floor (NOT the absolute 4pt
        // minimum): unreadably small text is worse than a slight overflow.
        assert!((size - source * MIN_SHRINK_RATIO).abs() < 0.001);
        assert!(!lines.is_empty());
    }

    #[test]
    fn wrap_breaks_at_connectors_without_inventing_spaces() {
        let m = metrics();
        // A dense cell value with no spaces to break at: it must wrap at the
        // slash rather than shrink or hard-break mid-token…
        let lines = wrap_paragraph(&m, "220Vac/20A/tek-fazli", 40.0, 8.0);
        assert!(lines.len() > 1, "should wrap at the connectors");
        // …and rejoining must not alter the text: no space may appear that was
        // not in the source.
        let rejoined: String = lines.concat();
        assert_eq!(rejoined, "220Vac/20A/tek-fazli");
    }

    #[test]
    fn wrap_keeps_real_spaces_between_words() {
        let m = metrics();
        let lines = wrap_paragraph(&m, "Giris tek fazli uc telli", 400.0, 8.0);
        assert_eq!(lines, vec!["Giris tek fazli uc telli"]);
    }

    #[test]
    fn wrap_respects_width() {
        let m = metrics();
        let lines = wrap_paragraph(&m, &"word ".repeat(40), 150.0, 10.0);
        assert!(lines.len() > 1);
        for l in &lines {
            assert!(m.text_w(l, 10.0) <= 150.0 + 0.01);
        }
    }

    fn overlay(translated: &str, l: f32, b: f32, r: f32, t: f32) -> ImageOverlay {
        ImageOverlay {
            page_index: 0,
            left: l,
            bottom: b,
            right: r,
            top: t,
            translated: translated.to_string(),
        }
    }

    #[test]
    fn plan_overlay_edit_fits_translation_into_image_box() {
        let m = metrics();
        let edit = plan_overlay_edit(&m, &overlay("Merhaba dunya", 100.0, 100.0, 300.0, 160.0))
            .expect("a non-empty overlay in a valid box must produce an edit");
        assert_eq!(edit.bbox.left, 100.0);
        assert_eq!(edit.bbox.top, 160.0);
        assert!(edit.size >= MIN_FONT_PT && edit.size <= MAX_FONT_PT);
        assert!(!edit.lines.is_empty());
    }

    fn replacement(path: &str, l: f32, b: f32, r: f32, t: f32) -> ImageReplacement {
        ImageReplacement {
            page_index: 0,
            left: l,
            bottom: b,
            right: r,
            top: t,
            image_path: path.to_string(),
        }
    }

    #[test]
    fn replacement_rect_returns_anchor_and_size() {
        let (l, b, w, h) =
            replacement_rect(&replacement("out.png", 50.0, 100.0, 350.0, 300.0)).unwrap();
        assert_eq!(l, 50.0);
        assert_eq!(b, 100.0);
        assert_eq!(w, 300.0);
        assert_eq!(h, 200.0);
    }

    #[test]
    fn replacement_rect_skips_degenerate_or_pathless() {
        // Zero/negative area boxes are skipped, never drawn.
        assert!(replacement_rect(&replacement("out.png", 10.0, 10.0, 10.5, 10.5)).is_none());
        assert!(replacement_rect(&replacement("out.png", 100.0, 100.0, 50.0, 50.0)).is_none());
        // Empty path (nothing to draw) is skipped.
        assert!(replacement_rect(&replacement("   ", 0.0, 0.0, 200.0, 100.0)).is_none());
    }

    #[test]
    fn layout_request_deserializes_without_image_replacements() {
        // Older frontends (and the default OCR-overlay path) never send the
        // field — serde(default) must keep the request valid and the path inert.
        let json = r#"{
            "runId": 1,
            "sourcePdfPath": "in.pdf",
            "outputPath": "out.pdf",
            "pages": []
        }"#;
        let req: LayoutRequest = serde_json::from_str(json).unwrap();
        assert!(req.image_replacements.is_empty());
        assert!(req.image_overlays.is_empty());
    }

    #[test]
    fn layout_request_deserializes_with_image_replacements() {
        let json = r#"{
            "runId": 2,
            "sourcePdfPath": "in.pdf",
            "outputPath": "out.pdf",
            "pages": [],
            "imageReplacements": [
                { "pageIndex": 1, "left": 5.0, "bottom": 6.0, "right": 105.0, "top": 86.0, "imagePath": "p1_i0.png" }
            ]
        }"#;
        let req: LayoutRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.image_replacements.len(), 1);
        assert_eq!(req.image_replacements[0].page_index, 1);
        assert_eq!(req.image_replacements[0].image_path, "p1_i0.png");
    }

    #[test]
    fn plan_overlay_edit_skips_empty_or_degenerate() {
        let m = metrics();
        // Empty/whitespace translation → nothing drawn (image left untouched).
        assert!(plan_overlay_edit(&m, &overlay("   ", 0.0, 0.0, 200.0, 100.0)).is_none());
        // Degenerate (zero-area) box → skipped.
        assert!(plan_overlay_edit(&m, &overlay("text", 10.0, 10.0, 10.5, 10.5)).is_none());
    }

    #[test]
    fn test_union_bbox_range() {
        let boxes = vec![
            Bbox { left: 0.0, bottom: 0.0, right: 10.0, top: 12.0 },
            Bbox { left: 10.0, bottom: 0.0, right: 22.0, top: 12.0 },
        ];
        let b = union_bbox_range(&boxes, 0, 1).unwrap();
        assert_eq!(b.left, 0.0);
        assert_eq!(b.right, 22.0);
        assert_eq!(b.top, 12.0);
    }

    #[test]
    fn test_estimate_font_size_range() {
        let boxes = vec![
            Bbox { left: 0.0, bottom: 0.0, right: 10.0, top: 12.0 },
            Bbox { left: 10.0, bottom: 0.0, right: 22.0, top: 12.0 },
        ];
        let sz = estimate_font_size_range(&boxes, 0, 1);
        assert!(sz >= MIN_FONT_PT && sz <= MAX_FONT_PT);
    }

    #[test]
    fn normalize_match_char_strips_private_use_and_fullwidth() {
        assert_eq!(normalize_match_char('\u{f0b7}'), None);
        assert_eq!(normalize_match_char('\u{ff41}'), Some('a'));
        assert_eq!(normalize_match_char('A'), Some('a'));
    }

    #[test]
    fn expand_layout_paragraphs_splits_toc_like_blocks() {
        let paras = vec![LayoutParagraph {
            original: "1.1 foo\n1.2 bar\n1.3 baz".to_string(),
            translated: "a\nb\nc".to_string(),
        }];
        let out = expand_layout_paragraphs(&paras);
        assert_eq!(out.lines.len(), 3);
        assert_eq!(out.lines[0].original, "1.1 foo");
        assert_eq!(out.line_text(0, 0).unwrap(), "a");
        assert_eq!(out.line_text(0, 2).unwrap(), "c");
        // All three lines belong to the same source paragraph.
        assert!(out.lines.iter().all(|l| l.orig_idx == 0));
    }

    #[test]
    fn drawable_translation_keeps_latin_drops_unmappable() {
        let m = metrics();
        let s = drawable_translation("Merhaba 你好 dünya", &m);
        assert!(s.contains("Merhaba"));
        assert!(s.contains("dünya"));
        assert!(!s.contains('你'));
    }

    #[test]
    fn expand_layout_paragraphs_splits_two_line_blocks() {
        // Previously 2-line short blocks were NOT split. Now they are — critical
        // for table cells where pdfium merges two adjacent cells into one paragraph.
        let paras = vec![LayoutParagraph {
            original: "Cell A\nCell B".to_string(),
            translated: "Hücre A\nHücre B".to_string(),
        }];
        let out = expand_layout_paragraphs(&paras);
        assert_eq!(
            out.lines.len(),
            2,
            "2-line block must be split into 2 entries"
        );
        assert_eq!(out.lines[0].original, "Cell A");
        assert_eq!(out.line_text(0, 0).unwrap(), "Hücre A");
        assert_eq!(out.lines[1].original, "Cell B");
        assert_eq!(out.line_text(0, 1).unwrap(), "Hücre B");
    }

    #[test]
    fn expand_layout_paragraphs_keeps_single_line_intact() {
        let paras = vec![LayoutParagraph {
            original: "Single line paragraph".to_string(),
            translated: "Tek satırlık paragraf".to_string(),
        }];
        let out = expand_layout_paragraphs(&paras);
        assert_eq!(out.lines.len(), 1);
        assert_eq!(out.lines[0].original, "Single line paragraph");
        assert_eq!(out.full[0], "Tek satırlık paragraf");
    }

    fn line_rec(orig_idx: usize, line_idx: usize, l: f32, r: f32, top: f32, size: f32) -> LineRec {
        LineRec {
            orig_idx,
            line_idx,
            groupable: true,
            drawable: true,
            is_toc: false,
            bbox: Bbox {
                left: l,
                bottom: top - size,
                right: r,
                top,
            },
            size,
            color: PdfColor::BLACK,
            bold: false,
        }
    }

    #[test]
    fn wrapped_continuation_lines_are_regrouped() {
        // The real-world failure: a bullet whose first line runs to the column
        // edge and whose short tail wrapped onto the next line. Split apart,
        // the tail's narrow box shrank its translation to unreadable text.
        let size = 10.0;
        let page_right = 500.0;
        let recs = vec![
            line_rec(0, 0, 100.0, 498.0, 400.0, size),
            line_rec(0, 1, 100.0, 160.0, 400.0 - size * LINE_FACTOR, size),
        ];
        let groups = group_wrapped_lines(&recs, page_right, &[]);
        assert_eq!(groups.len(), 1, "wrapped tail must rejoin its first line");
        assert_eq!(groups[0], vec![0, 1]);
    }

    #[test]
    fn wrapped_lines_regroup_across_translation_units() {
        // The frontend splits every multi-line block into ONE UNIT PER LINE, so
        // visually-wrapped lines arrive with different orig_idx. Grouping must
        // be geometric, or it never fires on a real document.
        let size = 12.0;
        let page_right = 539.0;
        let recs = vec![
            line_rec(0, 0, 56.7, 538.5, 733.2, size),
            line_rec(1, 0, 56.7, 538.9, 716.3, size),
            line_rec(2, 0, 56.7, 538.3, 699.3, size),
            line_rec(3, 0, 56.7, 176.7, 682.5, size),
        ];
        let groups = group_wrapped_lines(&recs, page_right, &[]);
        assert_eq!(
            groups.len(),
            1,
            "all four wrapped lines of one paragraph must form a single block"
        );
        assert_eq!(groups[0], vec![0, 1, 2, 3]);
    }

    #[test]
    fn heading_is_not_absorbed_into_the_paragraph_below() {
        // A heading stops far short of the column edge, so the line under it is
        // a new block — never a wrapped continuation.
        let size = 12.0;
        let page_right = 539.0;
        let recs = vec![
            line_rec(0, 0, 56.7, 120.5, 644.4, 12.96),
            line_rec(1, 0, 56.7, 538.3, 626.0, size),
        ];
        assert_eq!(group_wrapped_lines(&recs, page_right, &[]).len(), 2);
    }

    #[test]
    fn table_rows_are_not_regrouped() {
        // Same paragraph, stacked lines — but neither reaches the column edge,
        // so they are separate cells/rows and must stay independent.
        let size = 10.0;
        let page_right = 500.0;
        let recs = vec![
            line_rec(0, 0, 100.0, 180.0, 400.0, size),
            line_rec(0, 1, 100.0, 175.0, 400.0 - size * LINE_FACTOR, size),
        ];
        let groups = group_wrapped_lines(&recs, page_right, &[]);
        assert_eq!(groups.len(), 2, "table rows must not be merged");
    }

    #[test]
    fn distant_or_foreign_lines_are_not_regrouped() {
        let size = 10.0;
        let page_right = 500.0;
        // Full-width line followed by a line far down the page.
        let far = vec![
            line_rec(0, 0, 100.0, 498.0, 400.0, size),
            line_rec(0, 1, 100.0, 160.0, 200.0, size),
        ];
        assert_eq!(group_wrapped_lines(&far, page_right, &[]).len(), 2);

        // Full-width line followed by one indented far past a hanging indent.
        let indented = vec![
            line_rec(0, 0, 100.0, 498.0, 400.0, size),
            line_rec(1, 0, 260.0, 400.0, 400.0 - size * LINE_FACTOR, size),
        ];
        assert_eq!(group_wrapped_lines(&indented, page_right, &[]).len(), 2);

        // Table-split pieces are never groupable, even when they look wrapped.
        let mut ungroupable = vec![
            line_rec(0, 0, 100.0, 498.0, 400.0, size),
            line_rec(0, 1, 100.0, 160.0, 400.0 - size * LINE_FACTOR, size),
        ];
        ungroupable[0].groupable = false;
        assert_eq!(group_wrapped_lines(&ungroupable, page_right, &[]).len(), 2);
    }

    #[test]
    fn a_one_line_heading_shrinks_rather_than_wrapping() {
        let m = metrics();
        // Measured: the "其他配置" heading is 22.1 pt in a box that can grow to
        // ~235 pt. Its translation needs slightly more, so it wrapped into two
        // lines and drifted to the right of the page.
        let text = "Diğer yapılandırmalar";
        let (wrapped_size, wrapped) = fit_text(&m, text, 235.0, 60.0, 22.1, MIN_SHRINK_RATIO, false);
        let (kept_size, kept) = fit_text(&m, text, 235.0, 60.0, 22.1, MIN_SHRINK_RATIO, true);
        assert_eq!(kept.len(), 1, "a one-line heading must stay on one line");
        assert!(kept_size <= wrapped_size);
        assert!(
            kept_size >= 22.1 * MIN_SHRINK_RATIO - 0.001,
            "and must not shrink past the floor to do it"
        );
        let _ = wrapped;
    }

    #[test]
    fn fit_text_keeps_source_size_when_it_fits() {
        let m = metrics();
        // A translation that comfortably fits must be drawn at the SOURCE size,
        // not at some size inferred from glyph heights.
        let (size, lines) = fit_text(&m, "Kurulum Konumu", 400.0, 60.0, 20.0, MIN_SHRINK_RATIO, false);
        assert_eq!(size, 20.0);
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn fit_text_never_shrinks_below_the_source_ratio() {
        let m = metrics();
        let source = 12.0;
        let text = "word ".repeat(120);
        let (size, lines) = fit_text(&m, text.trim(), 120.0, 20.0, source, MIN_SHRINK_RATIO, false);
        assert!(
            size >= source * MIN_SHRINK_RATIO - 0.001,
            "shrank to {size}, below the {MIN_SHRINK_RATIO} floor of {source}"
        );
        assert!(!lines.is_empty(), "must never drop the text entirely");
    }

    fn column(left: f32, right: f32) -> TextColumn {
        TextColumn {
            left,
            right,
            page_center: 595.4 / 2.0,
            page_width: 595.4,
        }
    }

    #[test]
    fn grow_layout_box_expands_into_free_space_only() {
        let size = 10.0;
        let redact = Bbox {
            left: 100.0,
            bottom: 390.0,
            right: 200.0,
            top: 400.0,
        };
        // Nothing else on the page: grows right to the column edge and down.
        let alone: Vec<LineRec> = vec![line_rec(0, 0, 100.0, 200.0, 400.0, size)];
        let grown = grow_layout_box(&redact, &alone, &[0], &[], &column(100.0, 480.0), size, Align::Left, 0.0);
        assert!(grown.right >= 480.0, "should reach the column edge");
        assert!(grown.bottom < redact.bottom, "should gain vertical room");

        // A neighbour to the right on the same band blocks the expansion.
        let blocked = vec![
            line_rec(0, 0, 100.0, 200.0, 400.0, size),
            line_rec(1, 0, 300.0, 470.0, 400.0, size),
        ];
        let grown =
            grow_layout_box(&redact, &blocked, &[0], &[], &column(100.0, 480.0), size, Align::Left, 0.0);
        assert!(grown.right < 300.0, "must stop before the neighbour");
    }

    #[test]
    fn centered_block_grows_symmetrically() {
        // The page title: centred in the column with wide free margins. It must
        // reclaim BOTH margins, or a longer translation wraps instead of
        // staying on one line (the header defect the user reported).
        let size = 15.12;
        let redact = Bbox {
            left: 222.6,
            bottom: 788.0,
            right: 372.8,
            top: 803.3,
        };
        let recs = vec![line_rec(0, 0, 222.6, 372.8, 803.3, size)];
        let col = column(56.7, 539.0);
        assert_eq!(col.align_of(&[&recs[0]], &redact, size), Align::Center);

        let grown = grow_layout_box(&redact, &recs, &[0], &[], &col, size, Align::Center, 0.0);
        assert!(grown.left < redact.left, "must reclaim the left margin");
        assert!(grown.right > redact.right, "must reclaim the right margin");
        let before = (redact.left + redact.right) / 2.0;
        let after = (grown.left + grown.right) / 2.0;
        assert!(
            (before - after).abs() < 0.01,
            "the centre must not drift: {before} vs {after}"
        );
        assert!(grown.width() > 400.0, "should get most of the column width");
    }

    #[test]
    fn a_cramped_left_heading_reclaims_the_gap_before_it() {
        // "第12章 校准配置": the translated title only replaces the second run, so
        // it grows right from x=303.5 — not quite enough, and it wrapped. The
        // gap between the chapter number and the title is free, so a block that
        // still does not fit may reclaim it.
        let size = 22.1;
        let redact = Bbox { left: 303.5, bottom: 731.0, right: 391.8, top: 753.8 };
        let chapter_number = line_rec(1, 0, 203.6, 250.0, 753.8, size);
        let recs = vec![line_rec(0, 0, 303.5, 391.8, 753.8, size), chapter_number];
        let col = column(56.7, 539.0);

        // Fits already → the left edge must not move.
        let unchanged =
            grow_layout_box(&redact, &recs, &[0], &[], &col, size, Align::Left, 100.0);
        assert_eq!(unchanged.left, redact.left);

        // Needs more than the right side can give → reclaims the gap, but stops
        // at the chapter number.
        let grown = grow_layout_box(&redact, &recs, &[0], &[], &col, size, Align::Left, 300.0);
        assert!(grown.left < redact.left, "should reclaim the gap");
        assert!(grown.left > 250.0, "but must not run into the chapter number");
    }

    #[test]
    fn a_hemmed_in_centred_heading_uses_the_free_side() {
        // A chapter heading sits right of its (untranslated) chapter number, so
        // symmetric growth is capped by that neighbour and the translation was
        // squeezed into a narrow box and wrapped — even though the rest of the
        // line was empty. When symmetry cannot hold the text, the box may grow
        // asymmetrically instead.
        let size = 18.0;
        let redact = Bbox { left: 250.0, bottom: 700.0, right: 330.0, top: 720.0 };
        let chapter_number = line_rec(1, 0, 180.0, 245.0, 720.0, size);
        let recs = vec![line_rec(0, 0, 250.0, 330.0, 720.0, size), chapter_number];
        let col = column(56.7, 539.0);

        // Symmetric reach here is only ~(290-245)=45 either side, i.e. ~90 pt.
        let narrow = grow_layout_box(
            &redact, &recs, &[0], &[], &col, size, Align::Center, 60.0,
        );
        assert!(narrow.width() <= 100.0, "symmetry holds when the text fits");

        let wide = grow_layout_box(
            &redact, &recs, &[0], &[], &col, size, Align::Center, 240.0,
        );
        assert!(
            wide.width() >= 240.0,
            "a heading needing 240pt must get it from the free right side, got {}",
            wide.width()
        );
    }

    #[test]
    fn alignment_is_inferred_from_column_margins() {
        let col = column(56.7, 539.0);
        let size = 12.0;
        let one = |b: &Bbox| line_rec(0, 0, b.left, b.right, b.top, size);

        let flush_left = Bbox { left: 56.7, bottom: 700.0, right: 300.0, top: 712.0 };
        assert_eq!(col.align_of(&[&one(&flush_left)], &flush_left, size), Align::Left);
        // Balanced, generous margins → centred (the page footer "i").
        let centre = Bbox { left: 296.0, bottom: 700.0, right: 299.4, top: 712.0 };
        assert_eq!(col.align_of(&[&one(&centre)], &centre, size), Align::Center);
        // Lopsided margins are not centred.
        let lopsided = Bbox { left: 120.0, bottom: 700.0, right: 300.0, top: 712.0 };
        assert_eq!(col.align_of(&[&one(&lopsided)], &lopsided, size), Align::Left);
    }

    #[test]
    fn table_cell_in_the_right_column_stays_left_aligned() {
        // Regression: a cell in a table's right-hand column has a huge left
        // margin and none on the right, so a margin-only rule called it
        // right-aligned and shoved its translation to the cell's right edge.
        // Its lines share a LEFT edge, which is the evidence that decides it.
        let col = column(143.9, 534.6);
        let size = 12.0;
        let cell = vec![
            line_rec(0, 0, 300.0, 534.0, 500.0, size),
            line_rec(1, 0, 300.0, 480.0, 500.0 - size * LINE_FACTOR, size),
        ];
        let members: Vec<&LineRec> = cell.iter().collect();
        let bbox = Bbox { left: 300.0, bottom: 480.0, right: 534.0, top: 500.0 };
        assert_eq!(col.align_of(&members, &bbox, size), Align::Left);

        // A single right-hand cell line is left-aligned too (no Right at all).
        let single = line_rec(0, 0, 400.0, 534.0, 500.0, size);
        let sbox = Bbox { left: 400.0, bottom: 488.0, right: 534.0, top: 500.0 };
        assert_eq!(col.align_of(&[&single], &sbox, size), Align::Left);
    }

    #[test]
    fn multi_line_block_is_centred_only_when_lines_share_a_centre() {
        let col = column(56.7, 539.0);
        let size = 12.0;
        // A centred title: lines differ in width but share a centre axis.
        let centred = vec![
            line_rec(0, 0, 200.0, 400.0, 700.0, size),
            line_rec(1, 0, 240.0, 360.0, 700.0 - size * LINE_FACTOR, size),
        ];
        let members: Vec<&LineRec> = centred.iter().collect();
        let bbox = Bbox { left: 200.0, bottom: 685.0, right: 400.0, top: 700.0 };
        assert_eq!(col.align_of(&members, &bbox, size), Align::Center);

        // A normal paragraph shares a LEFT edge instead.
        let para = vec![
            line_rec(0, 0, 56.7, 538.0, 700.0, size),
            line_rec(1, 0, 56.7, 300.0, 700.0 - size * LINE_FACTOR, size),
        ];
        let members: Vec<&LineRec> = para.iter().collect();
        let bbox = Bbox { left: 56.7, bottom: 685.0, right: 538.0, top: 700.0 };
        assert_eq!(col.align_of(&members, &bbox, size), Align::Left);
    }

    fn edit_for_unify(source_size: f32, fitted: f32, text: &str) -> ParagraphEdit {
        let bbox = Bbox { left: 0.0, bottom: 0.0, right: 400.0, top: 20.0 };
        ParagraphEdit {
            redact: bbox.clone(),
            bbox,
            size: fitted,
            lines: vec![text.to_string()],
            color: PdfColor::BLACK,
            bold: false,
            align: Align::Left,
            source_size,
            text: text.to_string(),
            narrow: false,
            backdrop: PdfColor::WHITE,
        }
    }

    fn geometry_from(text: &str) -> PageGeometry {
        let matchable: Vec<MatchableChar> = text
            .chars()
            .enumerate()
            .filter_map(|(i, c)| {
                normalize_match_char(c).map(|nc| MatchableChar {
                    original_idx: i,
                    c: nc,
                })
            })
            .collect();
        PageGeometry {
            matchable_str: matchable.iter().map(|m| m.c).collect(),
            super_matchable_str: String::new(),
            matchable,
            super_matchable: Vec::new(),
            cursor_matchable_idx: 0,
            cursor_super_idx: 0,
        }
    }

    fn expansion_of(source: &str, translation: &str) -> Expansion {
        expand_layout_paragraphs(&[LayoutParagraph {
            original: source.to_string(),
            translated: translation.to_string(),
        }])
    }

    #[test]
    fn wrapped_lines_of_one_cell_are_not_split_into_rows() {
        // A tall cell's wrapped lines stay ONE block. Splitting them gave the
        // first row the whole translation and the others nothing — and a row
        // with no text gets no redaction, so the untranslated Chinese stayed
        // visible under it (the user's navigation-parameter table).
        let cell = Bbox { left: 90.0, bottom: 300.0, right: 400.0, top: 400.0 };
        let rows = vec![
            Bbox { left: 100.0, bottom: 380.0, right: 390.0, top: 395.0 },
            Bbox { left: 100.0, bottom: 360.0, right: 390.0, top: 375.0 },
            Bbox { left: 100.0, bottom: 340.0, right: 200.0, top: 355.0 },
        ];
        assert!(!rows_need_splitting(&[cell], &rows));
        // Cells are separated at extraction, so with no drawn geometry at all
        // a multi-row match is wrapped text and must NOT be split.
        assert!(!rows_need_splitting(&[], &rows));
    }

    #[test]
    fn rows_in_different_cells_are_still_split() {
        let upper_cell = Bbox { left: 90.0, bottom: 370.0, right: 400.0, top: 400.0 };
        let lower_cell = Bbox { left: 90.0, bottom: 330.0, right: 400.0, top: 365.0 };
        let rows = vec![
            Bbox { left: 100.0, bottom: 380.0, right: 200.0, top: 395.0 },
            Bbox { left: 100.0, bottom: 340.0, right: 200.0, top: 355.0 },
        ];
        assert!(rows_need_splitting(&[upper_cell, lower_cell], &rows));
    }

    #[test]
    fn cells_of_different_rows_are_never_glued_together() {
        // The rightmost column's cells reach the page edge, so they looked like
        // wrapped lines and the row below was glued onto the row above. Two
        // DIFFERENT units inside drawn cells can never be a continuation.
        let size = 10.0;
        let page_right = 500.0;
        let upper = line_rec(0, 0, 300.0, 498.0, 400.0, size);
        let lower = line_rec(1, 0, 300.0, 460.0, 400.0 - size * LINE_FACTOR, size);
        let cells = vec![
            Bbox { left: 295.0, bottom: 389.0, right: 500.0, top: 404.0 },
            Bbox { left: 295.0, bottom: 374.0, right: 500.0, top: 389.0 },
        ];
        let recs = vec![upper, lower];
        assert_eq!(group_wrapped_lines(&recs, page_right, &cells).len(), 2);
        // With no drawn cells (ordinary prose) the geometric rule still applies.
        assert_eq!(group_wrapped_lines(&recs, page_right, &[]).len(), 1);
    }

    #[test]
    fn rows_far_apart_are_never_one_block() {
        // A match reaching over a figure must not union across it — the
        // redaction box would paint over the figure and erase it.
        let upper = Bbox { left: 100.0, bottom: 700.0, right: 400.0, top: 712.0 };
        let far_below = Bbox { left: 100.0, bottom: 500.0, right: 400.0, top: 512.0 };
        assert!(rows_need_splitting(&[], &[upper.clone(), far_below]));
        // Consecutive lines stay one block.
        let next_line = Bbox { left: 100.0, bottom: 686.0, right: 400.0, top: 698.0 };
        assert!(!rows_need_splitting(&[], &[upper, next_line]));
    }

    #[test]
    fn a_rule_drawn_between_rows_keeps_them_split() {
        // No enclosing cells, but a table rule crosses the gap: separate rows.
        let rule = Bbox { left: 90.0, bottom: 366.0, right: 400.0, top: 367.0 };
        let rows = vec![
            Bbox { left: 100.0, bottom: 380.0, right: 390.0, top: 395.0 },
            Bbox { left: 100.0, bottom: 350.0, right: 390.0, top: 365.0 },
        ];
        assert!(rows_need_splitting(&[rule], &rows));
    }

    #[test]
    fn a_units_translation_is_drawn_only_once() {
        // The defect this encodes: one unit whose match split across several
        // row boxes handed its ENTIRE translation to every box, so the same
        // sentence was redrawn dozens of times down the page.
        let expansion = expansion_of("源文本", "Çevrilmiş metin");
        let rows: Vec<LineRec> = (0..4)
            .map(|i| {
                let mut r = line_rec(0, 0, 100.0, 300.0, 400.0 - i as f32 * 14.0, 12.0);
                r.groupable = false; // pieces of a multi-row match
                r
            })
            .collect();
        let mut placed = Placed::default();

        let first = group_text(&expansion, &[&rows[0]], &rows, &mut placed);
        assert_eq!(first, "Çevrilmiş metin");
        for rest in &rows[1..] {
            assert_eq!(
                group_text(&expansion, &[rest], &rows, &mut placed),
                "",
                "the same translation must not be placed twice"
            );
        }
    }

    #[test]
    fn per_line_slices_are_still_placed_on_every_line() {
        // Dedup must only affect the WHOLE-unit fallback: a multi-line unit
        // whose translation lines up per line still fills each line.
        let expansion = expansion_of("bir\niki", "one\ntwo");
        let recs = vec![
            line_rec(0, 0, 100.0, 300.0, 400.0, 12.0),
            line_rec(0, 1, 100.0, 300.0, 386.0, 12.0),
        ];
        let mut placed = Placed::default();
        assert_eq!(group_text(&expansion, &[&recs[0]], &recs, &mut placed), "one");
        assert_eq!(group_text(&expansion, &[&recs[1]], &recs, &mut placed), "two");
    }

    #[test]
    fn locate_maps_byte_offsets_to_character_indices() {
        // The defect this encodes: `find` returns BYTE offsets, but the
        // character vector is indexed per CHARACTER. With CJK text (3 bytes a
        // glyph) the byte offset ran past the end of the vector and panicked,
        // which killed the export thread — the window then sat on its last
        // progress message forever with no error.
        let page = "前言充电柜功率配置表结束";
        let mut geometry = geometry_from(page);
        let needle: String = page.chars().skip(5).take(3).collect(); // 柜功率

        let (start, end) = geometry.locate(&needle).expect("must locate the run");
        assert_eq!(start, 5, "start must be a CHARACTER index");
        assert_eq!(end, 7, "end must be the last matched CHARACTER index");
    }

    #[test]
    fn locate_returns_none_instead_of_panicking_on_a_miss() {
        let mut geometry = geometry_from("充电柜功率配置表");
        assert!(geometry.locate("完全不同的文本").is_none());
        assert!(geometry.locate("").is_none());
    }

    #[test]
    fn locate_advances_its_cursor_across_repeats() {
        let page = "输入功率输入功率";
        let mut geometry = geometry_from(page);
        let first = geometry.locate("输入功率").expect("first match");
        let second = geometry.locate("输入功率").expect("second match");
        assert_eq!(first.0, 0);
        assert_eq!(second.0, 4, "the cursor must move past the first match");
    }

    #[test]
    fn toc_rows_never_chain_into_one_block() {
        // Every contents row ends in a dot leader that reaches the column edge,
        // which looks exactly like a wrapped line. Without the leader check the
        // whole table of contents is redrawn as one run-on paragraph.
        let size = 12.0;
        let page_right = 538.3;
        let mut rows: Vec<LineRec> = (0..4)
            .map(|i| line_rec(i, 0, 92.7, 538.0, 700.0 - i as f32 * size * LINE_FACTOR, size))
            .collect();
        for r in rows.iter_mut() {
            r.is_toc = true;
        }
        assert_eq!(group_wrapped_lines(&rows, page_right, &[]).len(), 4);
    }

    #[test]
    fn toc_lines_are_detected_by_their_leader() {
        assert!(looks_like_toc_line("概览.......................... 1"));
        assert!(looks_like_toc_line("1.1 Overview . . . . . . . 4"));
        assert!(looks_like_toc_line("Bölüm 2 ……… 12"));
        // Ordinary prose, including an ellipsis mid-sentence, is not a TOC row.
        assert!(!looks_like_toc_line("Bu bir normal paragraf satırıdır."));
        assert!(!looks_like_toc_line("Sonuç olarak.. devam eder"));
    }

    #[test]
    fn untranslated_lines_still_block_a_neighbour_from_growing() {
        // A pass-through cell (a number the model left alone) is not drawn, but
        // it occupies its cell: the cell beside it must not grow over it.
        let size = 10.0;
        let redact = Bbox { left: 100.0, bottom: 390.0, right: 200.0, top: 400.0 };
        let mut untranslated = line_rec(1, 0, 260.0, 400.0, 400.0, size);
        untranslated.drawable = false;
        untranslated.groupable = false;
        let recs = vec![line_rec(0, 0, 100.0, 200.0, 400.0, size), untranslated];

        let grown = grow_layout_box(
            &redact,
            &recs,
            &[0],
            &[],
            &column(100.0, 480.0),
            size,
            Align::Left,
            0.0,
        );
        assert!(
            grown.right < 260.0,
            "growth must stop at the untranslated neighbour, got {}",
            grown.right
        );
    }

    #[test]
    fn enclosing_box_clamps_growth_inside_a_table_cell() {
        let size = 10.0;
        // Cell text with the cell's rules drawn around it.
        let redact = Bbox { left: 100.0, bottom: 390.0, right: 200.0, top: 400.0 };
        let cell = Bbox { left: 95.0, bottom: 385.0, right: 240.0, top: 405.0 };
        let recs = vec![line_rec(0, 0, 100.0, 200.0, 400.0, size)];

        let grown = grow_layout_box(
            &redact,
            &recs,
            &[0],
            &[cell.clone()],
            &column(95.0, 520.0),
            size,
            Align::Left,
            0.0,
        );
        assert!(
            grown.right <= cell.right,
            "text must stay inside its cell: {} > {}",
            grown.right,
            cell.right
        );
        assert!(grown.bottom >= cell.bottom, "must not drop below the cell");
    }

    #[test]
    fn a_box_below_blocks_downward_growth() {
        // The callout box under a paragraph: growing into it is what put body
        // text on top of the note box.
        let size = 10.0;
        let redact = Bbox { left: 100.0, bottom: 390.0, right: 400.0, top: 400.0 };
        let note = Bbox { left: 90.0, bottom: 330.0, right: 500.0, top: 385.0 };
        let recs = vec![line_rec(0, 0, 100.0, 400.0, 400.0, size)];

        let grown = grow_layout_box(
            &redact,
            &recs,
            &[0],
            &[note.clone()],
            &column(90.0, 520.0),
            size,
            Align::Left,
            0.0,
        );
        assert!(
            grown.bottom >= note.top,
            "must not grow into the box below: {} < {}",
            grown.bottom,
            note.top
        );
    }

    #[test]
    fn encloses_distinguishes_container_from_neighbour() {
        let inner = Bbox { left: 100.0, bottom: 100.0, right: 200.0, top: 120.0 };
        let container = Bbox { left: 90.0, bottom: 90.0, right: 260.0, top: 130.0 };
        let neighbour = Bbox { left: 260.0, bottom: 100.0, right: 400.0, top: 120.0 };
        assert!(encloses(&container, &inner));
        assert!(!encloses(&neighbour, &inner));
        // A box identical to the text is not a container (it IS the text run).
        assert!(!encloses(&inner, &inner));
    }

    #[test]
    fn cells_and_body_copy_are_unified_separately() {
        let m = metrics();
        let narrow = |fitted: f32| {
            let mut e = edit_for_unify(12.0, fitted, "hucre metni");
            e.narrow = true;
            e
        };
        let mut edits = vec![
            edit_for_unify(12.0, 12.0, "govde metni"),
            narrow(6.5),
            edit_for_unify(12.0, 11.0, "diger govde metni"),
            narrow(12.0),
        ];
        unify_sizes(&mut edits, &m);
        // Body copy unifies to the smallest BODY size — the cramped cell must
        // not drag it down…
        assert!((edits[0].size - 11.0).abs() < 0.01);
        assert!((edits[2].size - 11.0).abs() < 0.01);
        // …while the cells unify among THEMSELVES, so table text does not jump
        // between sizes from one line to the next.
        assert!((edits[1].size - 6.5).abs() < 0.01);
        assert!(
            (edits[3].size - 6.5).abs() < 0.01,
            "cells must share one size, got {}",
            edits[3].size
        );
    }

    #[test]
    fn backdrop_ignores_dense_ink_and_finds_the_real_background() {
        // The defect this encodes: a bold CJK heading covers MORE than half of
        // its own box, so a plain mode returned black and the redaction box was
        // painted black straight over the text it was meant to hide.
        let ink = PdfColor::BLACK;
        let mut samples = vec![(0u8, 0u8, 0u8); 70]; // 70% glyph coverage
        samples.extend(std::iter::repeat((255, 255, 255)).take(30));
        let bg = background_from_samples(&samples, &ink).expect("must find the white backdrop");
        assert!(bg.red() > 240 && bg.green() > 240 && bg.blue() > 240, "expected white");
    }

    #[test]
    fn backdrop_keeps_a_shaded_table_header() {
        // Grey header cell: the fill must survive, not be flattened to white.
        let ink = PdfColor::BLACK;
        let mut samples = vec![(0u8, 0u8, 0u8); 60];
        samples.extend(std::iter::repeat((208, 208, 208)).take(40));
        let bg = background_from_samples(&samples, &ink).expect("must find the grey backdrop");
        assert!(
            (bg.red() as i32 - 208).abs() <= 8,
            "expected ~grey, got {}",
            bg.red()
        );
    }

    #[test]
    fn backdrop_rejects_a_sample_with_no_contrast() {
        // All ink and nothing else: better to fall back to white than to paint
        // a box in the same colour as the text.
        let ink = PdfColor::BLACK;
        let samples = vec![(0u8, 0u8, 0u8); 50];
        assert!(background_from_samples(&samples, &ink).is_none());

        // Light text on a dark panel still resolves to the dark panel.
        let light = PdfColor::new(250, 250, 250, 255);
        let mut dark = vec![(250u8, 250u8, 250u8); 55];
        dark.extend(std::iter::repeat((20, 20, 30)).take(45));
        let bg = background_from_samples(&dark, &light).expect("dark panel must be found");
        assert!(bg.red() < 40 && bg.blue() < 60);
    }

    #[test]
    fn unify_sizes_makes_one_source_size_render_at_one_size() {
        let m = metrics();
        // Two blocks that were 12pt in the source: one fitted at full size, one
        // had to shrink. Rendering them at different sizes is exactly the
        // "part-large, part-small paragraph" defect.
        let mut edits = vec![
            edit_for_unify(12.0, 12.0, "kisa satir"),
            edit_for_unify(12.0, 9.0, "cok daha uzun bir satir"),
            // A heading class must keep its own (larger) size.
            edit_for_unify(20.0, 20.0, "Baslik"),
        ];
        unify_sizes(&mut edits, &m);
        assert_eq!(edits[0].size, edits[1].size, "same source size ⇒ same size");
        assert!((edits[0].size - 9.0).abs() < 0.01, "class takes the min");
        assert!((edits[2].size - 20.0).abs() < 0.01, "other classes untouched");
    }

    #[test]
    fn baseline_clipping_prevents_overflow() {
        // Simulate: a small bbox (height=14, font=12) should only fit ~1 line.
        let m = metrics();
        let text = "This is a very long text that should wrap to multiple lines if the box is narrow enough";
        let (size, lines) = fit_text(&m, text, 60.0, 14.0, 12.0, MIN_SHRINK_RATIO, false);
        assert!(size >= MIN_FONT_PT);
        assert!(!lines.is_empty());
        // Verify that at the fitted size, at least the first line fits in the box.
        let first_line_h = size * LINE_FACTOR;
        assert!(first_line_h <= 14.0 + 0.01, "First line must fit in the box");
    }

    #[test]
    fn inspect_pdf_text() {
        let dir = "C:\\Users\\enesk\\OneDrive\\Masaüstü\\";
        let mut pdf_path = None;
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.contains("Parameter Configuration") && name.ends_with(".pdf") {
                    if name.contains("Parameter Configuration") && !name.contains("_layout") {
                        pdf_path = Some(entry.path());
                        break;
                    }
                }
            }
        }

        let path = match pdf_path {
            Some(p) => p,
            None => {
                println!("PDF file not found dynamically on desktop.");
                return;
            }
        };
        let lib_path = "C:\\Users\\enesk\\AppData\\Roaming\\com.pot-app.desktop\\pdfium.dll";
        let pdfium = Pdfium::new(
            Pdfium::bind_to_library(lib_path).unwrap()
        );
        let _doc = pdfium.load_pdf_from_file(path.to_str().unwrap(), None).unwrap();

        let lib_path_buf = std::path::PathBuf::from(lib_path);
        let temp_dir = std::env::temp_dir();
        let images = crate::document::image_text::extract_page_images(
            &lib_path_buf,
            path.to_str().unwrap(),
            1,
            &temp_dir
        ).unwrap();
        println!("--- PAGE 2 IMAGES (len={}) ---", images.len());
        for (ii, img) in images.iter().enumerate() {
            println!("Image {}: L:{:.1} B:{:.1} R:{:.1} T:{:.1} Path: {}",
                ii, img.left, img.bottom, img.right, img.top, img.cache_path);
        }
    }
}

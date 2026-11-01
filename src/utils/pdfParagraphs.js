// PDF page text → translation units (shared by Document extraction + tests).

// TOC entry shape: "<title> <dot leader> <page number>". Leader chars may be
// ASCII dots, middle dots or ellipsis, optionally spaced (". . . .").
const TOC_LINE_RE = /^(.*?)\s*((?:[.·⋅…・]\s*){3,})(\d{1,4})$/s;

export function translateShortCaption(text, targetLang) {
    const match = /^([图表])\s*(\d+\s*[-.]\s*\d+)$/u.exec(text.trim());
    if (!match) return null;
    const labels = { tr: ['Şekil', 'Tablo'], en: ['Figure', 'Table'], de: ['Abbildung', 'Tabelle'], fr: ['Figure', 'Tableau'], es: ['Figura', 'Tabla'] };
    const pair = labels[String(targetLang).split(/[_-]/)[0]];
    return pair ? `${pair[match[1] === '图' ? 0 : 1]} ${match[2].replace(/\s/g, '')}` : null;
}

/** Parse a TOC-style line into { title, leader, page }, or null. */
export function parseTocLine(text) {
    const m = TOC_LINE_RE.exec((text || '').trim());
    if (!m) return null;
    const title = m[1].trim();
    if (!title) return null;
    return { title, leader: m[2].trim(), page: m[3] };
}

// Rough glyph-width units for leader sizing: CJK ≈ 2 Latin chars, a dot ≈ 0.5.
function approxWidth(str) {
    let w = 0;
    for (const c of str) {
        w += /[⺀-鿿豈-﫿！-｠　-〿]/.test(c) ? 2 : 1;
    }
    return w;
}

/**
 * Post-process the translation of a TOC line so the dot leader and the page
 * number survive whatever the model did to them (models routinely collapse
 * "........." or drop the trailing number). Non-TOC originals pass through
 * unchanged. The leader is re-emitted with a length that compensates for the
 * translated title being wider/narrower than the source, so the line still
 * fits its original bounding box in the layout-preserving export.
 */
export function normalizeTocTranslation(original, translated) {
    const toc = parseTocLine(original);
    if (!toc) return translated;
    let t = (translated || '').trim();
    if (!t) return translated;
    // Drop an echoed leader (with or without the page number after it)…
    t = t.replace(/(?:[.·⋅…・]\s*){2,}\d{0,4}\s*$/, '').trim();
    // …and a bare echoed page number left after the model dropped the dots.
    const barePage = new RegExp(`\\s${toc.page}$`);
    if (barePage.test(t)) t = t.replace(barePage, '').trim();
    if (t.length === 0) t = toc.title;

    const leaderChar = toc.leader.replace(/\s/g, '')[0] || '.';
    const dotCount = toc.leader.replace(/\s/g, '').length;
    const widthBudget = approxWidth(toc.title) + dotCount * 0.5;
    const newDots = Math.max(3, Math.round((widthBudget - approxWidth(t)) / 0.5));
    return `${t} ${leaderChar.repeat(newDots)} ${toc.page}`;
}

export function splitPdfSourceParagraphs(text) {
    const blocks = text.split(/\n\s*\n/).map((p) => p.trim()).filter((p) => p.length > 0);
    const out = [];
    for (const block of blocks) {
        const lines = block.split(/\n/).map((l) => l.trim()).filter((l) => l.length > 0);
        // Always split multi-line blocks into per-line units. The column-split
        // extractor separates table cells with blank lines but keeps the ROW
        // break inside a block, so a 2-line block can glue two unrelated cells
        // together ("变更内容\n2024/9"). Such units silently drop in the
        // layout export when the model merges them into one translated line
        // (the Rust side requires translated/original line counts to match).
        // Per-line units are what the rest of the pipeline is built around.
        if (lines.length > 1 || block.length > 600) {
            out.push(...lines);
        } else {
            out.push(block);
        }
    }
    return out.length > 0 ? out : [text.trim()].filter(Boolean);
}

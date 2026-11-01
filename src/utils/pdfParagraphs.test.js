import { describe, expect, it } from 'vitest';
import { splitPdfSourceParagraphs, parseTocLine, normalizeTocTranslation, translateShortCaption } from './pdfParagraphs';

it('translates standalone figure/table labels without sending their numbers to an LLM', () => {
    expect(translateShortCaption('图 2-1', 'tr')).toBe('Şekil 2-1');
    expect(translateShortCaption('表2.1', 'en')).toBe('Table 2.1');
    expect(translateShortCaption('图 2-1', 'ja')).toBe(null);
});

describe('splitPdfSourceParagraphs', () => {
    it('splits TOC-like blocks into lines', () => {
        const text = 'Title\n1.1 foo\n1.2 bar\n1.3 baz';
        const paras = splitPdfSourceParagraphs(text);
        expect(paras).toHaveLength(4);
        expect(paras[1]).toBe('1.1 foo');
    });

    it('splits 2-line blocks per line (column-split table cells)', () => {
        // Regression: the column-split extractor glues a table header cell to
        // the next row's first cell ("变更内容\n2024/9"). Kept as one 2-line
        // unit, the layout export silently dropped it when the model returned
        // a single translated line. Per-line units avoid that entirely.
        const text = 'Line one\nLine two';
        expect(splitPdfSourceParagraphs(text)).toEqual(['Line one', 'Line two']);
    });

    it('keeps single-line blocks intact', () => {
        expect(splitPdfSourceParagraphs('Only line')).toEqual(['Only line']);
    });

    it('splits the real revision-table page into per-cell units', () => {
        // Reconstructed from the FMR Pallet Recognition manual's last page
        // (blank lines are the column-split markers the Rust extractor emits).
        const text =
            '变更日期 \n\n变更版本 \n\n变更人 \n\n变更内容\r\n2024/9 \n\nV1.0 \n\n马世杰 \n\n新建\r\n2025/1 \n\n' +
            'V1.1 \n\n马世杰 \n\n随 V4.2.10 更新放货自主识别基\r\n准高度说明';
        const paras = splitPdfSourceParagraphs(text);
        expect(paras).toContain('变更内容');
        expect(paras).toContain('2024/9');
        expect(paras).toContain('新建');
        expect(paras).toContain('2025/1');
        expect(paras).toContain('随 V4.2.10 更新放货自主识别基');
        expect(paras).toContain('准高度说明');
        // No unit may still contain an internal line break.
        for (const p of paras) expect(p).not.toMatch(/\n/);
    });
});

describe('parseTocLine', () => {
    it('parses a dot-leader TOC entry', () => {
        const p = parseTocLine('1.1 内容摘要 ................................. 1');
        expect(p).not.toBeNull();
        expect(p.title).toBe('1.1 内容摘要');
        expect(p.page).toBe('1');
        expect(p.leader.startsWith('.')).toBe(true);
    });

    it('rejects normal prose and leaderless lines', () => {
        expect(parseTocLine('This is a normal sentence.')).toBeNull();
        expect(parseTocLine('3.2.1 Introduction')).toBeNull();
        expect(parseTocLine('............ 12')).toBeNull(); // no title
    });
});

describe('normalizeTocTranslation', () => {
    const original = '1.1 内容摘要 ....................... 1';

    it('returns non-TOC translations unchanged', () => {
        expect(normalizeTocTranslation('Normal paragraph.', 'Normal paragraf.')).toBe('Normal paragraf.');
    });

    it('restores leader + page when the model dropped them', () => {
        const out = normalizeTocTranslation(original, '1.1 İçerik Özeti');
        expect(out).toMatch(/^1\.1 İçerik Özeti \.{3,} 1$/);
    });

    it('deduplicates an echoed leader and page number', () => {
        const out = normalizeTocTranslation(original, '1.1 İçerik Özeti ...... 1');
        expect(out).toMatch(/^1\.1 İçerik Özeti \.{3,} 1$/);
    });

    it('strips a bare echoed page number', () => {
        const out = normalizeTocTranslation(original, '1.1 İçerik Özeti 1');
        expect(out).toMatch(/^1\.1 İçerik Özeti \.{3,} 1$/);
    });

    it('does not eat a number that is part of the title', () => {
        const orig = 'IPv4 配置 .............. 4';
        const out = normalizeTocTranslation(orig, 'IPv4 Yapılandırması');
        expect(out).toMatch(/^IPv4 Yapılandırması \.{3,} 4$/);
    });

    it('shrinks the leader when the translated title is wider', () => {
        const orig = '概览 ' + '.'.repeat(60) + ' 1';
        const out = normalizeTocTranslation(orig, 'Genel Bakış');
        const dots = (out.match(/\./g) || []).length;
        expect(dots).toBeLessThan(60);
        expect(dots).toBeGreaterThanOrEqual(3);
        expect(out.endsWith(' 1')).toBe(true);
    });
});

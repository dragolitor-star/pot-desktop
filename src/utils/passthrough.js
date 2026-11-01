// Strong LaTeX/code signal (rare in extracted PDF text, but decisive when present).
// Two or more CJK ideographs — enough to be a sentence in Chinese/Japanese.
const CJK_IDEOGRAPHS = /(?:[㐀-鿿豈-﫿]){2,}/;

export const FORMULA_SIGNAL =
    /(\\(?:frac|sum|int|sqrt|alpha|beta|gamma|theta|left|right|begin|end|cdot|times|partial|nabla)\b|\$\$|\\\[|\\\()/;

/**
 * Decide whether a paragraph is a formula / code / numeric-symbol block that should
 * be passed through UNTRANSLATED (Phase-2 spec). Skipping these saves API calls and
 * keeps math/code intact. `\p{L}` counts letters of ANY script, so CJK ideographs
 * and Turkish letters are letters too — prose (whose letter ratio is high) is never
 * misclassified; only digit/operator/symbol-dominated chunks fall through.
 *
 * @param {string} text — a single source paragraph
 * @returns {boolean} true → keep verbatim (do not translate)
 */
export function shouldPassThrough(text) {
    const s = (text || '').trim();
    if (s.length < 3) return false;
    // A single Han character can carry a caption's entire meaning (图 / 表).
    if (/[\u3400-\u9fff]/u.test(s)) return false;
    if (/^[\x00-\x7f]+$/.test(s)) {
        const words = s.split(/\s+/);
        const command = words[0].includes('_') || /^(helm|git|python|pip|npm|sudo|curl)$/.test(words[0]);
        const file = words.length === 1 && /\.(txt|bin|xml|yuv|json|log|dll|exe|cfg)$/i.test(s);
        if (file || (command && (words.length === 1 || words.slice(1).some((word) => /^[-+]?\d+(?:\.\d+)?$/.test(word))))) return true;
    }
    // TOC dot-leaders ("Overview ......... 12") would otherwise swamp the letter
    // ratio and misclassify a translatable heading as a numeric block — drop
    // leader runs (ASCII/middle-dot/ellipsis, optionally spaced) before counting.
    const noLeaders = s.replace(/(?:[.·⋅…・]\s*){3,}/g, ' ');
    const nonSpace = noLeaders.replace(/\s/g, '');
    if (nonSpace.length === 0) return false;
    const letters = (nonSpace.match(/\p{L}/gu) || []).length;
    const letterRatio = letters / nonSpace.length;
    if (FORMULA_SIGNAL.test(s) && letterRatio < 0.6) return true;
    // A value line like "取值：默认 2000，范围：0~5000，单位：°/1000。" is real prose
    // carrying numbers, but its letter ratio is only ~0.27 and it was being
    // skipped as a numeric block. Chinese is dense — a handful of ideographs is
    // a sentence — and code/terminal listings in these manuals are pure ASCII,
    // so the presence of CJK is reliable evidence that there is text to
    // translate.
    if (CJK_IDEOGRAPHS.test(nonSpace)) return false;
    return letterRatio < 0.3;
}

// External image-translation service (layout-image-translator / PaddleOCR-VL)
// — pure helpers shared by the Document window. The service is consumed over
// HTTP only (never modified or bundled); these helpers translate Pot's
// language codes into the service's language names and normalize the
// user-configured base URL. Kept pure so they are unit-testable (vitest).

export const DEFAULT_IMAGE_SERVICE_URL = 'http://127.0.0.1:8765';

// Pot language code -> the service's language name (its API accepts plain
// English language names, plus "auto" for source auto-detection). Keys mirror
// the Document window's LANG_OPTIONS.
const SERVICE_LANG_NAMES = {
    auto: 'auto',
    en: 'English',
    zh_cn: 'Chinese',
    zh_tw: 'Traditional Chinese',
    ja: 'Japanese',
    ko: 'Korean',
    fr: 'French',
    es: 'Spanish',
    ru: 'Russian',
    de: 'German',
    it: 'Italian',
    tr: 'Turkish',
    pt_pt: 'Portuguese',
    pt_br: 'Brazilian Portuguese',
    vi: 'Vietnamese',
    ar: 'Arabic',
    hi: 'Hindi',
    th: 'Thai',
    id: 'Indonesian',
    nl: 'Dutch',
    pl: 'Polish',
    uk: 'Ukrainian',
    fa: 'Persian',
    sv: 'Swedish',
    km: 'Khmer',
    fi: 'Finnish',
    cs: 'Czech',
    da: 'Danish',
    hu: 'Hungarian',
    nb_no: 'Norwegian',
    nn_no: 'Norwegian Nynorsk',
    mn_cy: 'Mongolian',
    he: 'Hebrew',
};

/**
 * Map a Pot language code to the service's language name. Unknown codes fall
 * through unchanged — the service treats unrecognized names as free-form
 * target language hints, which is still better than failing the whole export.
 */
export function serviceLangName(code) {
    if (!code) {
        return 'auto';
    }
    return SERVICE_LANG_NAMES[code] || code;
}

/**
 * Normalize the user-configured service base URL: trim whitespace, strip
 * trailing slashes, default the scheme to http://, and fall back to the
 * default localhost address when empty/unset.
 */
export function normalizeServiceUrl(url) {
    const trimmed = (url || '').trim().replace(/\/+$/, '');
    if (!trimmed) {
        return DEFAULT_IMAGE_SERVICE_URL;
    }
    if (/^https?:\/\//i.test(trimmed)) {
        return trimmed;
    }
    return `http://${trimmed}`;
}

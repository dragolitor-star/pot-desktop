// A JSON schema guarantees shape, not translation quality or unique IDs.
export function hasResidualCjk(text, targetLang) {
    const target = String(targetLang || '').toLowerCase();
    if (/^(zh|ja|ko)(?:[_-]|$)|chinese|japanese|korean|中文|日本|한국/.test(target)) return false;
    return /[\u3400-\u9fff\uf900-\ufaff]/u.test(text || '');
}

// Catch a valid JSON item paired with a neighboring command/filename. Match
// literal substrings so PDF splits such as `alg_b ias_left` remain compatible
// with the intact `alg_bias_left` in the target. Units may be localized.
export function hasProtectedTokenLoss(source, target) {
    const tokens = String(source || '').match(/[A-Za-z][A-Za-z0-9_.-]*\.(?:txt|bin|json|xml|ini|cfg|log|csv|dll|exe)|[A-Za-z][A-Za-z0-9_]*_[A-Za-z0-9_]+/g) || [];
    const counts = new Map();
    for (const token of tokens) counts.set(token, (counts.get(token) || 0) + 1);
    return [...counts].some(([token, count]) => String(target || '').split(token).length - 1 < count);
}

export function mapTranslationItems(items, count) {
    const out = Array(count).fill(null);
    const seen = new Set();
    const duplicates = new Set();
    for (const item of items) {
        const id = item?.id;
        if (!Number.isInteger(id) || id < 1 || id > count) continue;
        if (seen.has(id)) duplicates.add(id);
        seen.add(id);
        if (typeof item.translation === 'string' && item.translation.trim()) out[id - 1] = item.translation.trim();
    }
    for (const id of duplicates) out[id - 1] = null;
    return out;
}

// Salvage only complete JSON objects; braces and escaped quotes inside a
// translation must never turn into an invented item ID after token truncation.
export function parseStructuredArray(raw) {
    try {
        const value = JSON.parse(raw);
        if (Array.isArray(value)) return value;
    } catch { /* Partial response; salvage complete objects below. */ }
    const start = raw.indexOf('[');
    if (start < 0) return [];
    const objects = [];
    let depth = 0, objectStart = -1, quoted = false, escaped = false;
    for (let i = start + 1; i < raw.length; i++) {
        const c = raw[i];
        if (quoted) {
            if (escaped) escaped = false;
            else if (c === '\\') escaped = true;
            else if (c === '"') quoted = false;
            continue;
        }
        if (c === '"') quoted = true;
        else if (c === '{') { if (depth++ === 0) objectStart = i; }
        else if (c === '}' && depth > 0 && --depth === 0) {
            try { objects.push(JSON.parse(raw.slice(objectStart, i + 1))); } catch { /* Incomplete object. */ }
        } else if (c === ']' && depth === 0) break;
    }
    return objects;
}

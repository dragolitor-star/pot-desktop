import { invoke } from '@tauri-apps/api/tauri';

// Shared glossary application helpers — Phase 1.
//
// LLM engines (Gemini, OpenAI, Claude, ChatGLM, Ollama, …) call
// applyGlossaryToPrompt() before sending the request. The helper
// prepends a JSON mapping block with a directive sentence so the
// model honors source→target term mappings.
//
// Classical engines (Google, DeepL, Bing, Yandex, Baidu, …) call
// applyGlossaryPostTranslate() on the response. The helper runs
// Unicode-aware word-boundary regex substitutions on the translated
// text, respecting the per-entry case_sensitive flag.
//
// Glossary entries arrive from the Rust-side `get_active_glossary`
// Tauri command (see src-tauri/src/glossary.rs). Entries are already
// sorted by priority (exact lang > wildcard, scope-matched > NULL,
// recent updated_at first), so the LLM mapping picks the
// highest-priority replacement for any duplicate source_term, and
// the post-translate pass applies highest-priority substitutions
// first.
//
// Out of scope for Phase 1: external .potext plugins do NOT call
// these helpers (see TODO comment in TargetArea + docs/glossary.md).

const WORD_CHAR_CLASS = '[\\p{L}\\p{N}_]';
const NON_WORD_CHAR_CLASS = '[^\\p{L}\\p{N}_]';
const REGEX_META_CHARS = /[.*+?^${}()|[\]\\]/g;

/**
 * Build the glossary instruction block and prepend it to an LLM
 * prompt template.
 *
 * The original prompt is returned unchanged when there are no
 * entries — engines can call this unconditionally and the no-op
 * path is free of side effects.
 *
 * Variables in the source prompt ($text, $from, $to, $detect) are
 * untouched; engine-level substitution runs after this transform.
 *
 * @param {string} promptText — engine prompt template
 * @param {Array<{source_term: string, target_term: string}>} entries
 *        — ordered highest-priority first
 * @returns {string} — prompt with glossary block prepended
 */
export function applyGlossaryToPrompt(promptText, entries) {
    if (!entries || entries.length === 0) return promptText;

    // First seen wins so the highest-priority entry sticks for any
    // duplicate source_term value.
    const mapping = {};
    for (const entry of entries) {
        if (!entry || !entry.source_term) continue;
        if (!(entry.source_term in mapping)) {
            mapping[entry.source_term] = entry.target_term;
        }
    }
    if (Object.keys(mapping).length === 0) return promptText;

    const block =
        '[Glossary — use these mappings strictly. Do not deviate. ' +
        'Apply exactly as written, preserving case where the source ' +
        'matches case-sensitively. Mappings are JSON: source_term ' +
        '→ target_term.]\n' +
        JSON.stringify(mapping, null, 2) +
        '\n\n';
    return block + (promptText ?? '');
}

/**
 * Run word-boundary substitutions over the output of a classical
 * (non-LLM) translation engine.
 *
 * Uses Unicode-aware boundaries (`\p{L}\p{N}_`) so accented Latin,
 * Cyrillic, Greek, Turkish "ş ğ ı İ Ş Ğ" etc. match correctly.
 *
 * Entries are applied in the order received (highest priority
 * first). Later entries CAN rewrite earlier substitutions if their
 * source_term matches an earlier replacement — accepted as an edge
 * case; docs/glossary.md flags it for users.
 *
 * @param {string} text — translated text from the engine
 * @param {Array<{
 *   source_term: string,
 *   target_term: string,
 *   case_sensitive?: boolean | 0 | 1,
 * }>} entries — ordered highest-priority first
 * @returns {string} — text with glossary substitutions applied
 */
export function applyGlossaryPostTranslate(text, entries) {
    if (typeof text !== 'string' || text.length === 0) return text;
    if (!entries || entries.length === 0) return text;

    let result = text;
    for (const entry of entries) {
        if (!entry || !entry.source_term) continue;
        const escaped = entry.source_term.replace(REGEX_META_CHARS, '\\$&');
        const caseFlag = entry.case_sensitive ? '' : 'i';
        const flags = `${caseFlag}gu`;
        // Capture the leading boundary char (or empty for start-of-string)
        // and put it back via the replace callback. This avoids lookbehind
        // assertions whose Unicode behavior varies across JS engines.
        const pattern = `(^|${NON_WORD_CHAR_CLASS})${escaped}(?=$|${NON_WORD_CHAR_CLASS})`;
        let re;
        try {
            re = new RegExp(pattern, flags);
        } catch (_e) {
            // Bad source_term that escaped sanitization — skip without crashing
            // the whole pipeline.
            continue;
        }
        result = result.replace(re, (_match, leading) => leading + (entry.target_term ?? ''));
    }
    return result;
}

// Built-in translation engines that operate on a prompt (LLM-style).
// Engines NOT in this list are treated as classical (output post-processed
// at the dispatcher with applyGlossaryPostTranslate). External `.potext`
// plugins are out of scope for Phase 1 (directive #5).
export const BUILTIN_LLM_ENGINES = ['geminipro', 'openai', 'chatglm', 'ollama'];

/**
 * Fetch the currently-applicable glossary entries from the Rust side.
 * Never throws — returns an empty array on any failure so translation
 * is never blocked by a glossary problem (per "fail-soft" design).
 *
 * @param {string} sourceLang — short ISO-ish code or '*' for wildcard
 * @param {string} targetLang
 * @param {string | null} [scope] — optional domain (e.g. 'tech')
 * @returns {Promise<Array<object>>}
 */
export async function fetchActiveGlossary(sourceLang, targetLang, scope = null) {
    try {
        const entries = await invoke('get_active_glossary', { sourceLang, targetLang, scope });
        return Array.isArray(entries) ? entries : [];
    } catch (e) {
        // eslint-disable-next-line no-console
        console.warn('Glossary fetch failed:', e);
        return [];
    }
}

// Internal exports for tests in Commit #7.
export const __internals = {
    WORD_CHAR_CLASS,
    NON_WORD_CHAR_CLASS,
    REGEX_META_CHARS,
};

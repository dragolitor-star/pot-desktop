import { fetch, Body } from '@tauri-apps/api/http';
import { Language } from './info';
import { GEMINI_MODEL_PRESETS, GEMINI_DEFAULT_PRESET, GEMINI_API_BASE } from './Config';
import { applyGlossaryToPrompt } from '../../../utils/glossary';

const KNOWN_LEGACY_MODEL_SEGMENTS = [
    'gemini-pro',
    'gemini-1.0-pro',
    'gemini-1.5-pro',
    'gemini-1.5-flash',
    'gemini-2.0-flash',
    'gemini-2.0-pro',
    'gemini-2.5-flash',
    'gemini-2.5-pro',
    'gemini-2.5-flash-lite',
];

// Resolves the effective model name and API base URL from the user's serviceConfig.
// Tolerates legacy configs whose requestPath was the full URL ending in a known model name.
function resolveModelAndBase(config) {
    // Effective model name.
    let model;
    if (config.useCustomModel && config.customModel) {
        model = config.customModel;
    } else if (typeof config.presetKey === 'string' && config.presetKey) {
        model = config.presetKey;
    } else {
        // Try to recover from a legacy requestPath; otherwise fall back to default.
        const path = (config.requestPath || '').replace(/\/+$/, '');
        const trailing = path.slice(path.lastIndexOf('/') + 1);
        if (KNOWN_LEGACY_MODEL_SEGMENTS.includes(trailing)) {
            model = GEMINI_DEFAULT_PRESET;
        } else if (GEMINI_MODEL_PRESETS.some((p) => p.key === trailing)) {
            model = trailing;
        } else {
            model = GEMINI_DEFAULT_PRESET;
        }
    }

    // API base URL (must end with `/`).
    let base = config.requestPath || GEMINI_API_BASE;
    if (!/^https?:\/\//.test(base)) {
        base = 'https://' + base;
    }
    const trimmed = base.replace(/\/+$/, '');
    const trailing = trimmed.slice(trimmed.lastIndexOf('/') + 1);
    if (KNOWN_LEGACY_MODEL_SEGMENTS.includes(trailing) || GEMINI_MODEL_PRESETS.some((p) => p.key === trailing)) {
        // requestPath ended with a model name — strip it to get the base.
        base = trimmed.slice(0, trimmed.lastIndexOf('/') + 1);
    } else {
        base = trimmed + '/';
    }

    return { model, base };
}

export async function translate(text, from, to, options = {}) {
    const { config, setResult, detect } = options;
    const { apiKey, stream } = config;
    let { promptList } = config;

    // Glossary injection (Phase 1) — runs before variable substitution so
    // $text/$from/$to in the original prompt remain intact for the next pass.
    // Gemini's prompt shape is { role, parts: [{ text }] }; inject into the
    // first user message (Gemini uses user/model role pairs, no system role).
    const glossaryEntries = options.glossaryEntries ?? [];
    if (glossaryEntries.length > 0) {
        const injIdx = promptList.findIndex((m) => m.role === 'user');
        if (injIdx !== -1) {
            promptList = promptList.map((m, i) =>
                i === injIdx
                    ? { ...m, parts: [{ text: applyGlossaryToPrompt(m.parts[0].text, glossaryEntries) }] }
                    : m
            );
        }
    }

    const { model, base } = resolveModelAndBase(config);
    const url = stream
        ? `${base}${model}:streamGenerateContent?alt=sse&key=${apiKey}`
        : `${base}${model}:generateContent?key=${apiKey}`;

    promptList = promptList.map((item) => {
        return {
            ...item,
            parts: [
                {
                    text: item.parts[0].text
                        .replaceAll('$text', text)
                        .replaceAll('$from', from)
                        .replaceAll('$to', to)
                        .replaceAll('$detect', Language[detect]),
                },
            ],
        };
    });

    const headers = {
        'Content-Type': 'application/json',
    };
    const body = {
        contents: promptList,
        safetySettings: [
            { category: 'HARM_CATEGORY_HARASSMENT', threshold: 'BLOCK_NONE' },
            { category: 'HARM_CATEGORY_HATE_SPEECH', threshold: 'BLOCK_NONE' },
            { category: 'HARM_CATEGORY_SEXUALLY_EXPLICIT', threshold: 'BLOCK_NONE' },
            { category: 'HARM_CATEGORY_DANGEROUS_CONTENT', threshold: 'BLOCK_NONE' },
        ],
        // Raise the output ceiling so large document-translation batches don't get
        // truncated mid-response (the default cap is much lower). It's only a ceiling,
        // so short selection/input translations are unaffected. All Gemini 3.x Flash
        // models support this; a custom legacy model that rejects it can be pointed
        // at a lower value via a future config field.
        generationConfig: { maxOutputTokens: 32768 },
    };

    if (stream) {
        const res = await window.fetch(url, {
            method: 'POST',
            headers: headers,
            body: JSON.stringify(body),
        });
        if (!res.ok) {
            const errBody = await res.text().catch(() => '');
            throw `Http Request Error\nHttp Status: ${res.status}\n${errBody}`;
        }
        return await readSseStream(res, setResult);
    } else {
        const res = await fetch(url, {
            method: 'POST',
            headers: headers,
            body: Body.json(body),
        });

        if (!res.ok) {
            throw `Http Request Error\nHttp Status: ${res.status}\n${JSON.stringify(res.data)}`;
        }
        const { candidates } = res.data;
        if (!candidates) {
            throw JSON.stringify(res.data);
        }
        let target = candidates[0]?.content?.parts?.[0]?.text;
        if (!target) {
            throw JSON.stringify(candidates);
        }
        target = target.trim();
        if (target.startsWith('"')) target = target.slice(1);
        if (target.endsWith('"')) target = target.slice(0, -1);
        return target.trim();
    }
}

// Shared safety settings (mirror the per-engine translate() call so structured
// document batches aren't blocked by overzealous filters on benign source text).
const GEMINI_SAFETY_SETTINGS = [
    { category: 'HARM_CATEGORY_HARASSMENT', threshold: 'BLOCK_NONE' },
    { category: 'HARM_CATEGORY_HATE_SPEECH', threshold: 'BLOCK_NONE' },
    { category: 'HARM_CATEGORY_SEXUALLY_EXPLICIT', threshold: 'BLOCK_NONE' },
    { category: 'HARM_CATEGORY_DANGEROUS_CONTENT', threshold: 'BLOCK_NONE' },
];

// Decide the thinkingConfig for a given model. Only Gemini 2.5+/3.x are thinking
// models; older models would reject/ignore the field, so we omit it for them.
// Flash / Flash-Lite can fully disable thinking (budget 0); Pro cannot go below 128.
function thinkingConfigFor(model) {
    const m = (model || '').toLowerCase();
    const isThinkingModel = /gemini-(2\.5|3)/.test(m);
    if (!isThinkingModel) return undefined;
    if (m.includes('pro')) return { thinkingBudget: 128 };
    return { thinkingBudget: 0 };
}

/**
 * Structured-output batch translation for document mode (HANDOFF §8 #1).
 *
 * Instead of joining paragraphs with fragile `@@n@@` markers and parsing them
 * back out of free text, we ask Gemini for schema-constrained JSON: an array of
 * `{ id, translation }`. Because the schema FORCES a `translation` field per id,
 * this eliminates the two weak-model failure modes the marker approach suffered:
 *   1. marker drift / dropped markers → whole-batch "Paragraph mismatch", and
 *   2. the model echoing the (e.g. Chinese) SOURCE instead of translating.
 * It also lets batches be large again on flash-lite (fewer API calls).
 *
 * Returns an array aligned to `texts` (length === texts.length). Any id the model
 * fails to return is left `null`, so the caller can mark just that paragraph for
 * a cheap re-run while every other paragraph is kept + cached.
 *
 * Gemini-only. Non-Gemini engines keep using the numbered-marker fallback.
 *
 * @param {string[]} texts — source paragraphs in order
 * @param {string} from — Gemini language label (e.g. 'English', 'Auto')
 * @param {string} to — Gemini language label (e.g. 'Turkish')
 * @param {{ config: object, glossaryEntries?: Array<object> }} options
 * @returns {Promise<Array<string|null>>}
 */
export async function translateBatchStructured(texts, from, to, options = {}) {
    const { config } = options;
    const { apiKey } = config;
    const glossaryEntries = options.glossaryEntries ?? [];

    const { model, base } = resolveModelAndBase(config);
    // Structured output uses the non-streaming endpoint (we need the whole JSON
    // document to parse it; there's no value in token streaming here).
    const url = `${base}${model}:generateContent?key=${apiKey}`;

    const fromLabel = from && from !== 'Auto' ? from : 'the source language';
    const items = texts.map((t, i) => ({ id: i + 1, source: t }));

    // Instruction only — per Google's guidance we do NOT restate the output
    // schema in the prompt (responseSchema already constrains it; duplicating it
    // degrades quality). Glossary mappings are injected via the shared helper.
    let instruction =
        `You are a professional translator. Translate the "source" field of every ` +
        `item in the JSON array below from ${fromLabel} into ${to}. ` +
        `Return one result per item, keeping the same "id". Translate every item — ` +
        `never copy the source untranslated and never omit, merge, reorder, or add ` +
        `items. Preserve numbers, inline punctuation and meaning; output natural, ` +
        `fluent ${to}. Do not add notes or explanations.`;
    instruction = applyGlossaryToPrompt(instruction, glossaryEntries);

    const userText = `${instruction}\n\nInput items:\n${JSON.stringify(items)}`;

    const generationConfig = {
        maxOutputTokens: 32768,
        responseMimeType: 'application/json',
        responseSchema: {
            type: 'ARRAY',
            items: {
                type: 'OBJECT',
                properties: {
                    id: { type: 'INTEGER' },
                    translation: { type: 'STRING' },
                },
                required: ['id', 'translation'],
                propertyOrdering: ['id', 'translation'],
            },
        },
    };

    // Gemini 2.5+/3.x Flash models default to *dynamic thinking*, and thinking
    // tokens are billed against maxOutputTokens. On a dense batch the reasoning can
    // consume the whole ceiling, truncating the JSON (finishReason=MAX_TOKENS) so the
    // trailing items come back empty — which showed up as untranslated final pages of
    // a long document. Translation needs no chain-of-thought, so disable it. (Pro
    // models can't fully disable thinking — the API floor is 128 — so cap there.)
    const thinkingConfig = thinkingConfigFor(model);
    if (thinkingConfig) generationConfig.thinkingConfig = thinkingConfig;

    const body = {
        contents: [{ role: 'user', parts: [{ text: userText }] }],
        safetySettings: GEMINI_SAFETY_SETTINGS,
        generationConfig,
    };

    const res = await fetch(url, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: Body.json(body),
    });

    if (!res.ok) {
        throw `Http Request Error\nHttp Status: ${res.status}\n${JSON.stringify(res.data)}`;
    }
    const { candidates } = res.data;
    if (!candidates || candidates.length === 0) {
        throw JSON.stringify(res.data);
    }
    const cand = candidates[0];
    const finish = cand?.finishReason;
    if (finish && finish !== 'STOP' && finish !== 'MAX_TOKENS') {
        // Safety/recitation block etc. — surface it instead of returning empties.
        throw `Gemini stopped: finishReason=${finish}`;
    }
    const raw = cand?.content?.parts?.[0]?.text;
    if (!raw) {
        throw JSON.stringify(candidates);
    }

    const parsed = parseStructuredArray(raw);
    const out = new Array(texts.length).fill(null);
    for (const el of parsed) {
        const id = parseInt(el?.id, 10);
        if (!Number.isNaN(id) && id >= 1 && id <= texts.length && typeof el.translation === 'string') {
            const val = el.translation.trim();
            if (val.length > 0) out[id - 1] = val;
        }
    }
    return out;
}

// Parse the model's JSON array. With responseMimeType=application/json the body
// is normally clean JSON, but stay defensive: a MAX_TOKENS truncation can clip
// the trailing `]`, so fall back to slicing the outermost array and, if needed,
// salvaging complete `{...}` objects so a partial batch still yields its done items.
function parseStructuredArray(raw) {
    try {
        const v = JSON.parse(raw);
        return Array.isArray(v) ? v : [];
    } catch (_) {
        // fall through to salvage
    }
    const start = raw.indexOf('[');
    if (start !== -1) {
        const end = raw.lastIndexOf(']');
        if (end > start) {
            try {
                const v = JSON.parse(raw.slice(start, end + 1));
                if (Array.isArray(v)) return v;
            } catch (_) {
                // fall through to object-by-object salvage
            }
        }
    }
    const objects = [];
    const objRe = /\{[^{}]*\}/g;
    let m;
    while ((m = objRe.exec(raw)) !== null) {
        try {
            objects.push(JSON.parse(m[0]));
        } catch (_) {
            // skip a malformed fragment
        }
    }
    return objects;
}

// Proper SSE chunk reader for Gemini's `streamGenerateContent?alt=sse` endpoint.
// Each event is "\n\n"-delimited; only lines starting with "data:" carry payload.
async function readSseStream(res, setResult) {
    if (!res.body) {
        throw 'Streaming response has no body';
    }
    const reader = res.body.getReader();
    const decoder = new TextDecoder('utf-8', { fatal: false });
    let buffer = '';
    let target = '';

    try {
        while (true) {
            const { done, value } = await reader.read();
            if (done) break;
            buffer += decoder.decode(value, { stream: true });
            let sep;
            while ((sep = buffer.indexOf('\n\n')) !== -1) {
                const eventBlock = buffer.slice(0, sep);
                buffer = buffer.slice(sep + 2);
                target = consumeSseEvent(eventBlock, target, setResult);
            }
        }
        // Flush trailing bytes (rare for proper SSE, defensive).
        buffer += decoder.decode();
        if (buffer.length > 0) {
            target = consumeSseEvent(buffer, target, setResult);
        }
    } finally {
        reader.releaseLock();
    }

    target = target.trim();
    if (target.startsWith('"')) target = target.slice(1);
    if (target.endsWith('"')) target = target.slice(0, -1);
    target = target.trim();
    if (setResult) setResult(target);
    return target;
}

// One SSE event = one or more lines. We only care about "data:" lines.
function consumeSseEvent(eventBlock, target, setResult) {
    for (const rawLine of eventBlock.split('\n')) {
        const line = rawLine.replace(/\r$/, '');
        if (!line.startsWith('data:')) continue;
        const jsonStr = line.slice(5).trim();
        if (!jsonStr || jsonStr === '[DONE]') continue;
        let payload;
        try {
            payload = JSON.parse(jsonStr);
        } catch (_) {
            // A complete \n\n-delimited block should always contain complete JSON;
            // skip malformed lines defensively rather than abort the stream.
            continue;
        }
        if (payload.error) {
            throw `Gemini API error: ${JSON.stringify(payload.error)}`;
        }
        const cand = payload?.candidates?.[0];
        const chunk = cand?.content?.parts?.[0]?.text;
        if (chunk) {
            target += chunk;
            if (setResult) setResult(target + '_');
        }
        // Surface a clear error for safety/recitation blocks instead of returning empty.
        const finish = cand?.finishReason;
        if (finish && finish !== 'STOP' && finish !== 'MAX_TOKENS') {
            throw `Gemini stopped: finishReason=${finish}`;
        }
    }
    return target;
}

export * from './Config';
export * from './info';

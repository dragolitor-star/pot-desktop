import { fetch, Body } from '@tauri-apps/api/http';
import { Language } from './info';
import { GEMINI_MODEL_PRESETS, GEMINI_DEFAULT_PRESET, GEMINI_API_BASE } from './Config';

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

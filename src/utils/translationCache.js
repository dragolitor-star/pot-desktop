// Shared translation-cache + pacing helpers (Phase 4).
//
// These mirror the private helpers inside `window/Document/index.jsx`. They were
// extracted so the new Subtitle window can reuse the exact same SQLite-backed
// translation cache, stable model-id keying and sliding-window RPM limiter without
// duplicating the logic. (Document still has its own copies; it can be switched to
// this util in a later, separately-verified change — left untouched here on purpose
// so the in-flight PDF/EPUB verification isn't disturbed.)

import { invoke } from '@tauri-apps/api/tauri';
import { hasResidualCjk, hasProtectedTokenLoss } from './translationQuality';

export async function sha256Hex(str) {
    const buffer = new TextEncoder().encode(str);
    const hash = await crypto.subtle.digest('SHA-256', buffer);
    return Array.from(new Uint8Array(hash))
        .map((b) => b.toString(16).padStart(2, '0'))
        .join('');
}

// Stable per-engine model identifier for cache keys. Changing the model invalidates
// the key for NEW requests; existing entries persist until cleared via cache_clear.
export function getEngineModelId(engineName, instanceConfig) {
    if (engineName === 'geminipro') {
        if (instanceConfig?.useCustomModel && instanceConfig?.customModel) {
            return `gemini:${instanceConfig.customModel}`;
        }
        return `gemini:${instanceConfig?.presetKey || 'unknown'}`;
    }
    if (instanceConfig?.model) {
        return `${engineName}:${instanceConfig.model}`;
    }
    return engineName;
}

export async function getCachedTranslation(modelId, srcLang, tgtLang, text) {
    try {
        const hash = await sha256Hex(`${modelId}\n${srcLang}\n${tgtLang}\n${text}`);
        const cached = await invoke('cache_get_translation', { hash });
        return hasResidualCjk(cached, tgtLang) || hasProtectedTokenLoss(text, cached) ? null : (cached ?? null);
    } catch (e) {
        console.warn('cache_get_translation failed:', e);
        return null;
    }
}

export async function setCachedTranslation(modelId, srcLang, tgtLang, sourceText, translatedText) {
    if (hasResidualCjk(translatedText, tgtLang) || hasProtectedTokenLoss(sourceText, translatedText)) return;
    try {
        const hash = await sha256Hex(`${modelId}\n${srcLang}\n${tgtLang}\n${sourceText}`);
        await invoke('cache_set_translation', {
            hash,
            model: modelId,
            srcLang,
            tgtLang,
            sourceText,
            translatedText,
        });
    } catch (e) {
        console.warn('cache_set_translation failed:', e);
    }
}

// Sliding-window RPM limiter. acquire() resolves when a request slot is free, so
// API calls stay under the provider's requests-per-minute ceiling.
export class RpmLimiter {
    constructor(rpm) {
        this.rpm = Math.max(1, rpm | 0);
        this.windowMs = 60_000;
        this.timestamps = [];
    }
    async acquire() {
        for (;;) {
            const now = Date.now();
            this.timestamps = this.timestamps.filter((t) => now - t < this.windowMs);
            if (this.timestamps.length < this.rpm) {
                this.timestamps.push(now);
                return;
            }
            const oldest = this.timestamps[0];
            const waitMs = oldest + this.windowMs - now + 50;
            await new Promise((r) => setTimeout(r, Math.max(50, waitMs)));
        }
    }
}

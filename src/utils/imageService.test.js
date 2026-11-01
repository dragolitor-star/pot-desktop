import { describe, expect, it } from 'vitest';

import { DEFAULT_IMAGE_SERVICE_URL, normalizeServiceUrl, serviceLangName } from './imageService';

describe('serviceLangName', () => {
    it('maps Pot language codes to service language names', () => {
        expect(serviceLangName('tr')).toBe('Turkish');
        expect(serviceLangName('en')).toBe('English');
        expect(serviceLangName('zh_cn')).toBe('Chinese');
        expect(serviceLangName('ja')).toBe('Japanese');
    });

    it('passes auto through for source auto-detection', () => {
        expect(serviceLangName('auto')).toBe('auto');
    });

    it('defaults empty input to auto and passes unknown codes through', () => {
        expect(serviceLangName('')).toBe('auto');
        expect(serviceLangName(undefined)).toBe('auto');
        expect(serviceLangName('xx_unknown')).toBe('xx_unknown');
    });
});

describe('normalizeServiceUrl', () => {
    it('keeps a clean http/https URL unchanged', () => {
        expect(normalizeServiceUrl('http://127.0.0.1:8765')).toBe('http://127.0.0.1:8765');
        expect(normalizeServiceUrl('https://translator.example.com')).toBe('https://translator.example.com');
    });

    it('strips trailing slashes and trims whitespace', () => {
        expect(normalizeServiceUrl(' http://127.0.0.1:8765/ ')).toBe('http://127.0.0.1:8765');
        expect(normalizeServiceUrl('http://host:8765///')).toBe('http://host:8765');
    });

    it('adds a default http scheme when missing', () => {
        expect(normalizeServiceUrl('localhost:8765')).toBe('http://localhost:8765');
        expect(normalizeServiceUrl('192.168.1.20:8765')).toBe('http://192.168.1.20:8765');
    });

    it('falls back to the default localhost URL when empty', () => {
        expect(normalizeServiceUrl('')).toBe(DEFAULT_IMAGE_SERVICE_URL);
        expect(normalizeServiceUrl(null)).toBe(DEFAULT_IMAGE_SERVICE_URL);
        expect(normalizeServiceUrl('   ')).toBe(DEFAULT_IMAGE_SERVICE_URL);
    });
});

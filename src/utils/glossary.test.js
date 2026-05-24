import { describe, it, expect } from 'vitest';
import { applyGlossaryToPrompt, applyGlossaryPostTranslate } from './glossary';

describe('applyGlossaryToPrompt', () => {
    it('returns original prompt when entries is null, undefined, or empty', () => {
        const prompt = 'Translate this: hello';
        expect(applyGlossaryToPrompt(prompt, null)).toBe(prompt);
        expect(applyGlossaryToPrompt(prompt, undefined)).toBe(prompt);
        expect(applyGlossaryToPrompt(prompt, [])).toBe(prompt);
    });

    it('prepends a JSON mapping block for a single entry', () => {
        const prompt = 'Translate: hello';
        const entries = [{ source_term: 'hello', target_term: 'merhaba kanka' }];
        const result = applyGlossaryToPrompt(prompt, entries);
        
        expect(result).toContain('[Glossary — use these mappings strictly. Do not deviate.');
        expect(result).toContain('"hello": "merhaba kanka"');
        expect(result).toContain('\n\nTranslate: hello');
    });

    it('preserves the highest priority entry when duplicate source_term exists (first wins)', () => {
        const prompt = 'Translate: hello';
        const entries = [
            { source_term: 'hello', target_term: 'merhaba kanka' }, // highest priority
            { source_term: 'hello', target_term: 'merhaba dünya' },
        ];
        const result = applyGlossaryToPrompt(prompt, entries);
        
        expect(result).toContain('"hello": "merhaba kanka"');
        expect(result).not.toContain('"hello": "merhaba dünya"');
    });
});

describe('applyGlossaryPostTranslate', () => {
    it('returns original text when entries or text is empty/missing', () => {
        expect(applyGlossaryPostTranslate('', [])).toBe('');
        expect(applyGlossaryPostTranslate(null, [])).toBe(null);
        expect(applyGlossaryPostTranslate('hello', null)).toBe('hello');
        expect(applyGlossaryPostTranslate('hello', [])).toBe('hello');
    });

    it('substitutes a single term respecting case-insensitivity by default', () => {
        const entries = [{ source_term: 'hello', target_term: 'merhaba', case_sensitive: false }];
        expect(applyGlossaryPostTranslate('Hello, how are you?', entries)).toBe('merhaba, how are you?');
        expect(applyGlossaryPostTranslate('hello, how are you?', entries)).toBe('merhaba, how are you?');
    });

    it('respects case-sensitivity when flag is true', () => {
        const entries = [{ source_term: 'hello', target_term: 'merhaba', case_sensitive: true }];
        expect(applyGlossaryPostTranslate('Hello, how are you?', entries)).toBe('Hello, how are you?');
        expect(applyGlossaryPostTranslate('hello, how are you?', entries)).toBe('merhaba, how are you?');
    });

    it('correctly handles Turkish accented characters on word boundaries', () => {
        // Turkish "şirket" -> "firma"
        const entries = [{ source_term: 'şirket', target_term: 'firma', case_sensitive: false }];
        
        // boundary match (surrounded by spaces/punctuation)
        expect(applyGlossaryPostTranslate('Bu şirket çok büyük.', entries)).toBe('Bu firma çok büyük.');
        expect(applyGlossaryPostTranslate('şirket!', entries)).toBe('firma!');
        
        // substring match within larger word (should NOT substitute because of word boundaries)
        expect(applyGlossaryPostTranslate('şirketler grubu', entries)).toBe('şirketler grubu');
        expect(applyGlossaryPostTranslate('başşirket', entries)).toBe('başşirket');
    });

    it('correctly escapes regex meta characters in the source_term', () => {
        const entries = [{ source_term: 'C++', target_term: 'CPP', case_sensitive: false }];
        expect(applyGlossaryPostTranslate('I love C++', entries)).toBe('I love CPP');
        expect(applyGlossaryPostTranslate('C++ is great', entries)).toBe('CPP is great');
        expect(applyGlossaryPostTranslate('C+++ is not C++', entries)).toBe('CPP+ is not CPP');
    });

    it('does no-op on no match', () => {
        const entries = [{ source_term: 'hello', target_term: 'merhaba' }];
        expect(applyGlossaryPostTranslate('world is big', entries)).toBe('world is big');
    });
});

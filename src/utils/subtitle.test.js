import { describe, it, expect } from 'vitest';
import { formatSrtTime, formatVttTime, buildSrt, buildVtt } from './subtitle';

describe('timestamp formatting', () => {
    it('formats SRT timestamps with a comma decimal separator', () => {
        expect(formatSrtTime(0)).toBe('00:00:00,000');
        expect(formatSrtTime(62.5)).toBe('00:01:02,500');
        expect(formatSrtTime(3661.25)).toBe('01:01:01,250');
    });

    it('formats VTT timestamps with a dot decimal separator', () => {
        expect(formatVttTime(0)).toBe('00:00:00.000');
        expect(formatVttTime(62.5)).toBe('00:01:02.500');
    });

    it('clamps negative / non-finite values to zero', () => {
        expect(formatSrtTime(-5)).toBe('00:00:00,000');
        expect(formatSrtTime(NaN)).toBe('00:00:00,000');
        expect(formatVttTime(undefined)).toBe('00:00:00.000');
    });
});

const cues = [
    { start: 1, end: 3, original: 'Hello', translated: 'Merhaba' },
    { start: 3, end: 5.5, original: 'world', translated: '' },
];

describe('buildSrt', () => {
    it('numbers entries and uses the original text by default', () => {
        const srt = buildSrt(cues, 'original');
        expect(srt).toBe(
            '1\n00:00:01,000 --> 00:00:03,000\nHello\n\n' +
            '2\n00:00:03,000 --> 00:00:05,500\nworld\n'
        );
    });

    it('falls back to original text when a cue has no translation', () => {
        const srt = buildSrt(cues, 'translated');
        expect(srt).toContain('Merhaba');
        expect(srt).toContain('world'); // second cue untranslated → original kept
    });

    it('stacks original + translation in bilingual mode', () => {
        const srt = buildSrt(cues, 'bilingual');
        expect(srt).toContain('Hello\nMerhaba');
    });

    it('skips wholly empty cues so no blank entry is emitted', () => {
        const srt = buildSrt([{ start: 0, end: 1, original: '   ', translated: '' }], 'original');
        expect(srt).toBe('');
    });
});

describe('buildVtt', () => {
    it('prepends the WEBVTT header and omits numeric ids', () => {
        const vtt = buildVtt(cues, 'original');
        expect(vtt.startsWith('WEBVTT\n\n')).toBe(true);
        expect(vtt).toContain('00:00:01.000 --> 00:00:03.000\nHello');
        expect(vtt).not.toMatch(/\n1\n/); // no SRT-style sequence numbers
    });
});

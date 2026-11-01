import { describe, it, expect } from 'vitest';
import { shouldPassThrough } from './passthrough';

it('keeps commands intact while translating one-character figure labels', () => {
    expect(shouldPassThrough('castor_set_helm_the_calib 0 300 300')).toBe(true);
    expect(shouldPassThrough('helm set 0 1018')).toBe(true);
    expect(shouldPassThrough('图 2-1')).toBe(false);
});

describe('shouldPassThrough', () => {
    it('never skips natural-language prose (any script)', () => {
        // The critical safety property: prose must always be translated.
        expect(shouldPassThrough('近年来，中国的科技发展速度非常快，尤其是在人工智能。')).toBe(false);
        expect(shouldPassThrough("Son yıllarda Çin'in teknolojik gelişme hızı oldukça yüksektir.")).toBe(false);
        expect(shouldPassThrough('In recent years China has developed rapidly.')).toBe(false);
        expect(shouldPassThrough('3.2.1 Introduction')).toBe(false);
    });

    it('does not pass through TOC lines with dot leaders', () => {
        expect(shouldPassThrough('概览 ............................................................ 1')).toBe(false);
        expect(shouldPassThrough('1.1 内容摘要 ................................................... 1')).toBe(false);
        expect(shouldPassThrough('2.6.1 载具放置地面 ......................................... 12')).toBe(false);
        expect(shouldPassThrough('Introduction ................................................... 3')).toBe(false);
        // spaced leader variant
        expect(shouldPassThrough('概览 . . . . . . . . . . . . . . . . . . . . . . . . . . . . 1')).toBe(false);
    });

    it('still passes through numeric blocks even with the leader fix', () => {
        expect(shouldPassThrough('1.2.3 4.5.6 7.8.9')).toBe(true);
    });

    it('skips formula / math / numeric-symbol blocks', () => {
        expect(shouldPassThrough('x = (-b ± √(b² - 4ac)) / (2a)')).toBe(true);
        expect(shouldPassThrough('12 / 63')).toBe(true);
        expect(shouldPassThrough('= 1234.56 + 7.8% − 9 × 10')).toBe(true);
    });

    it('skips LaTeX-signalled content even with some letters', () => {
        expect(shouldPassThrough('\\frac{a}{b} + \\sum_{i=0}^{n} x_i')).toBe(true);
        expect(shouldPassThrough('$$ E = mc^2 $$')).toBe(true);
    });

    it('does not skip LaTeX-signalled text that is mostly prose', () => {
        // A sentence that merely mentions a command should still be translated.
        expect(shouldPassThrough('The \\sum operator denotes a summation over the index range.')).toBe(false);
    });

    it('keeps short / empty / whitespace input as translatable (conservative)', () => {
        expect(shouldPassThrough('')).toBe(false);
        expect(shouldPassThrough(null)).toBe(false);
        expect(shouldPassThrough(undefined)).toBe(false);
        expect(shouldPassThrough('  ')).toBe(false);
        expect(shouldPassThrough('ab')).toBe(false);
    });
});

describe('CJK value lines', () => {
    it('translates parameter value lines despite their many digits', () => {
        // Measured letter ratio ≈ 0.27, which the numeric-block rule skipped.
        expect(shouldPassThrough('取值：默认 2000，范围： 0~5000，单位：°/1000。')).toBe(false);
        expect(shouldPassThrough('取值：默认 1000，单位：1/1000 度。范围： -3000~5000。')).toBe(false);
        expect(shouldPassThrough('XML 中对应字段：acur_normal_move_t。')).toBe(false);
    });

    it('still passes through numeric blocks that carry no CJK', () => {
        // The CJK exemption must not weaken the numeric-block rule elsewhere.
        expect(shouldPassThrough('-3000~5000, 0~5000, 1/1000')).toBe(true);
        expect(shouldPassThrough('0: 1, 1: 2, 2: 3, 3: 4')).toBe(true);
    });
});

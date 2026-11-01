import { describe, expect, it } from 'vitest';
import { hasResidualCjk, hasProtectedTokenLoss, mapTranslationItems, parseStructuredArray } from './translationQuality';

describe('structured translation validation', () => {
    it('rejects a neighboring command or missing filename even with a valid item ID', () => {
        expect(hasProtectedTokenLoss('查看 castor_fork_calib_set read_o3x', 'castor_fork_calib_set write_o3x')).toBe(true);
        expect(hasProtectedTokenLoss('下载 rcu_hw_cfg_out.json', 'Dosyayı indirin')).toBe(true);
        expect(hasProtectedTokenLoss('prm.txt prm.txt', 'prm.txt')).toBe(true);
    });
    it('accepts restored PDF-split identifiers and localized units', () => {
        expect(hasProtectedTokenLoss('有 alg_b ias_left 和 delt_x 1', 'alg_bias_left ve delt_x1')).toBe(false);
        expect(hasProtectedTokenLoss('0.3m/s 的速度', '0,3 m/s hız')).toBe(false);
        expect(hasProtectedTokenLoss('下载 rcu_hw_cfg_out.json', 'rcu_hw_cfg_out.json dosyasını indirin')).toBe(false);
    });
    it('maps shuffled IDs without shifting a missing item', () => {
        expect(mapTranslationItems([{ id: 3, translation: 'üç' }, { id: 1, translation: 'bir' }], 3)).toEqual(['bir', null, 'üç']);
    });
    it('rejects ambiguous duplicate IDs and non-integer IDs', () => {
        expect(mapTranslationItems([{ id: 1, translation: 'a' }, { id: 1, translation: 'b' }, { id: '2', translation: 'c' }], 2)).toEqual([null, null]);
    });
    it('rejects residual Han only for non-CJK targets', () => {
        expect(hasResidualCjk('Adım 步骤', 'tr')).toBe(true);
        expect(hasResidualCjk('步骤', 'Chinese (Simplified)')).toBe(false);
        expect(hasResidualCjk('步骤', 'ja_JP')).toBe(false);
        expect(hasResidualCjk('Adım 1', 'Turkish')).toBe(false);
    });
    it('salvages a truncated array without treating quoted braces as new items', () => {
        const first = { id: 1, translation: 'f(x) = {a}; "id": 2' };
        const raw = `[${JSON.stringify(first)},{"id":2,"translation":"unfinished`;
        expect(parseStructuredArray(raw)).toEqual([first]);
    });
});

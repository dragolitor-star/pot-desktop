import { describe, it, expect, vi, beforeEach } from 'vitest';
const { load, db } = vi.hoisted(() => {
    const db = { execute: vi.fn(async () => ({})), close: vi.fn() };
    return { db, load: vi.fn(async () => db) };
});
vi.mock('tauri-plugin-sql-api', () => ({ default: { load } }));
beforeEach(() => { vi.resetModules(); load.mockClear(); db.execute.mockClear(); db.close.mockClear(); });
describe('shared translation history', () => {
    it('creates the schema once and retains the connection across concurrent engine results', async () => {
        const { addHistory, openHistory } = await import('./history');
        await Promise.all([addHistory('hello', 'en', 'tr', 'google', 'merhaba'), addHistory('hello', 'en', 'tr', 'bing', 'selam'), openHistory()]);
        expect(load).toHaveBeenCalledTimes(1);
        expect(db.execute).toHaveBeenCalledTimes(3);
        expect(db.execute.mock.calls[0][0]).toContain('CREATE TABLE IF NOT EXISTS');
        expect(db.close).not.toHaveBeenCalled();
    });
    it('allows retry after a database initialization failure', async () => {
        load.mockRejectedValueOnce(new Error('temporarily unavailable'));
        const { openHistory } = await import('./history');
        await expect(openHistory()).rejects.toThrow('temporarily unavailable');
        await expect(openHistory()).resolves.toBe(db);
        expect(load).toHaveBeenCalledTimes(2);
    });
});

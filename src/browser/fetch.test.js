import { describe, it, expect, vi } from 'vitest';
import { createNativeFetch } from './fetch';

function fixture() {
    let handler;
    const unlisten = vi.fn();
    const invoke = vi.fn(async () => undefined);
    const nativeFetch = vi.fn(async () => new Response('local'));
    const fetch = createNativeFetch({ nativeFetch, invoke, listen: async (_, cb) => { handler = cb; return unlisten; }, origin: 'http://127.0.0.1:9876' });
    return { fetch, invoke, nativeFetch, unlisten, send: payload => handler({ payload }) };
}
describe('browser streaming network adapter', () => {
    it('keeps local requests on the authenticated browser transport', async () => {
        const test = fixture();
        expect(await (await test.fetch('http://127.0.0.1:9876/api/status')).text()).toBe('local');
        expect(test.invoke).not.toHaveBeenCalled();
    });
    it('delivers headers and chunks before the upstream response finishes', async () => {
        const test = fixture();
        const response = test.fetch('https://provider.invalid/stream', { method: 'POST', body: 'hello' });
        await vi.waitFor(() => expect(test.invoke).toHaveBeenCalledWith('web_fetch_start', expect.anything()));
        test.send({ type: 'headers', status: 200, headers: { 'content-type': 'text/event-stream' } });
        const reader = (await response).body.getReader();
        test.send({ type: 'chunk', data: [...new TextEncoder().encode('data: one\n\n')] });
        expect(new TextDecoder().decode((await reader.read()).value)).toBe('data: one\n\n');
        test.send({ type: 'chunk', data: [...new TextEncoder().encode('data: two\n\n')] });
        expect(new TextDecoder().decode((await reader.read()).value)).toBe('data: two\n\n');
        test.send({ type: 'done' });
        expect((await reader.read()).done).toBe(true);
        await vi.waitFor(() => expect(test.unlisten).toHaveBeenCalledTimes(1));
    });
    it('aborts the native request and errors an already open stream', async () => {
        const test = fixture(), abort = new AbortController();
        const response = test.fetch('https://provider.invalid/stream', { signal: abort.signal });
        await vi.waitFor(() => expect(test.invoke).toHaveBeenCalled());
        test.send({ type: 'headers', status: 200, headers: {} });
        const reader = (await response).body.getReader();
        abort.abort();
        await expect(reader.read()).rejects.toMatchObject({ name: 'AbortError' });
        await vi.waitFor(() => expect(test.invoke).toHaveBeenCalledWith('web_fetch_cancel', expect.anything()));
        expect(test.unlisten).toHaveBeenCalledTimes(1);
    });
    it('preserves HTTP errors for provider-specific error handling', async () => {
        const test = fixture(), response = test.fetch('https://provider.invalid/quota');
        await vi.waitFor(() => expect(test.invoke).toHaveBeenCalled());
        test.send({ type: 'headers', status: 429, headers: { 'retry-after': '30' } });
        test.send({ type: 'chunk', data: [...new TextEncoder().encode('{"error":"quota"}')] });
        test.send({ type: 'done' });
        const result = await response;
        expect(result.status).toBe(429);
        expect(result.headers.get('retry-after')).toBe('30');
        expect(await result.json()).toEqual({ error: 'quota' });
    });
    it('handles aborts before the native start acknowledgement', async () => {
        const test = fixture(), abort = new AbortController();
        let started;
        test.invoke.mockImplementation(cmd => cmd === 'web_fetch_start' ? new Promise(resolve => { started = resolve; }) : Promise.resolve());
        const response = test.fetch('https://provider.invalid/slow', { signal: abort.signal });
        const rejected = expect(response).rejects.toMatchObject({ name: 'AbortError' });
        await vi.waitFor(() => expect(started).toBeTypeOf('function'));
        abort.abort();
        await new Promise(resolve => setTimeout(resolve, 0));
        started();
        await rejected;
        expect(test.unlisten).toHaveBeenCalledTimes(1);
    });
});

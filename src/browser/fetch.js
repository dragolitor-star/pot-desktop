/** Adapt native response events to a real Fetch stream, including aborts. */
export function createNativeFetch({ nativeFetch, invoke, listen, origin }) {
    return async (input, init = {}) => {
        const req = new Request(input, init);
        const url = new URL(req.url);
        if (url.origin === origin || !['http:', 'https:'].includes(url.protocol)) return nativeFetch(input, init);
        if (req.signal.aborted) throw new DOMException('Aborted', 'AbortError');
        const id = crypto.randomUUID();
        let controller, unlisten, settled = false, ended = false, started = false;
        let resolveResponse, rejectResponse;
        const response = new Promise((resolve, reject) => { resolveResponse = resolve; rejectResponse = reject; });
        // Aborts may arrive while the native start command is still pending.
        // Mark the rejection handled now; the returned promise still rejects.
        void response.catch(() => {});
        const cleanup = async () => {
            req.signal.removeEventListener('abort', abort);
            if (unlisten) { const stop = unlisten; unlisten = null; await stop(); }
            if (started) await invoke('web_fetch_cancel', { id });
        };
        const fail = error => {
            if (ended) return;
            ended = true;
            if (settled) controller.error(error); else rejectResponse(error);
            void cleanup().catch(console.error);
        };
        const abort = () => fail(new DOMException('Aborted', 'AbortError'));
        const body = new ReadableStream({ start(value) { controller = value; }, cancel() { ended = true; return cleanup(); } });
        try {
            unlisten = await listen(`web_fetch://${id}`, ({ payload }) => {
                if (ended) return;
                if (payload.type === 'headers') {
                    const headers = new Headers(payload.headers);
                    headers.delete('content-encoding'); headers.delete('content-length');
                    settled = true;
                    resolveResponse(new Response([204, 205, 304].includes(payload.status) ? null : body, { status: payload.status, headers }));
                } else if (payload.type === 'chunk') controller.enqueue(new Uint8Array(payload.data));
                else if (payload.type === 'done') {
                    ended = true; controller.close(); void cleanup().catch(console.error);
                } else if (payload.type === 'error') fail(new Error(payload.error));
            });
            req.signal.addEventListener('abort', abort, { once: true });
            if (req.signal.aborted) { abort(); return response; }
            const bytes = ['GET', 'HEAD'].includes(req.method) ? null : Array.from(new Uint8Array(await req.arrayBuffer()));
            if (req.signal.aborted) { abort(); return response; }
            started = true;
            await invoke('web_fetch_start', { request: { id, url: req.url, method: req.method, headers: Object.fromEntries(req.headers), body: bytes } });
            if (ended) await invoke('web_fetch_cancel', { id });
        } catch (error) { fail(error); }
        return response;
    };
}

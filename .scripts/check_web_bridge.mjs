// Integration tests against the actual hidden Tauri worker, not a mock transport.
// Usage: node .scripts/check_web_bridge.mjs <debug-url-file>
import fs from 'node:fs/promises';
import assert from 'node:assert/strict';
import path from 'node:path';
const url = new URL((await fs.readFile(process.argv[2], 'utf8')).trim());
const token = url.hash.slice(1), base = url.origin;
let client = '', cursor = 0, next = 100;
const callbacks = new Map();
let running = true;
async function request(path, body, extra = {}) {
    return fetch(base + path, { method: body === undefined ? 'GET' : 'POST', headers: { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json', 'X-Bridge-Client': client, 'X-Bridge-Cursor': String(cursor), ...extra }, body: body === undefined ? undefined : JSON.stringify(body) });
}
async function call(cmd, args = {}) {
    const callback = ++next, error = ++next;
    return new Promise((resolve, reject) => {
        const timer = setTimeout(() => { callbacks.delete(callback); callbacks.delete(error); reject(new Error(`Timeout: ${cmd}`)); }, 30000);
        const done = fn => value => { clearTimeout(timer); callbacks.delete(callback); callbacks.delete(error); fn(value); };
        callbacks.set(callback, done(resolve)); callbacks.set(error, done(value => reject(new Error(String(value)))));
        request('/api/bridge', { cmd, callback, error, ...args }).then(async response => { if (!response.ok) callbacks.get(error)?.((await response.json()).error); }).catch(reason => callbacks.get(error)?.(reason.message));
    });
}
const native = (module, message) => call('tauri', { __tauriModule: module, message });
for (let attempt = 0; ; attempt++) {
    const response = await request('/api/bridge/connect', {});
    if (response.ok) { ({ client } = await response.json()); break; }
    if (attempt > 60) throw new Error('Worker did not become ready');
    await new Promise(resolve => setTimeout(resolve, 500));
}
const polling = (async () => {
    while (running) {
        const response = await request('/api/bridge/events');
        const data = await response.json();
        if (!response.ok) throw new Error(data.error);
        for (const item of data.events) { cursor = item.id; callbacks.get(item.event.callback)?.(item.event.payload); }
        await new Promise(resolve => setTimeout(resolve, 50));
    }
})();
try {
    assert.equal((await fetch(base + '/api/bridge/events')).status, 401);
    assert.equal((await request('/api/bridge/events', undefined, { Origin: 'https://untrusted.invalid' })).status, 401);
    assert.equal((await request('/api/bridge', { cmd: 'web_bridge_ready', callback: 1, error: 2 })).status, 400);
    console.log('PASS: authentication, origin checks and command allowlist');
    const html = await (await fetch(base + '/')).text();
    assert.match(html, /workspace-/);
    for (const [, asset] of html.matchAll(/(?:src|href)="(\/assets\/[^"]+)"/g)) assert.equal((await fetch(base + asset)).status, 200);
    console.log('PASS: packaged browser entry and assets');
    assert.match(await native('App', { cmd: 'getAppVersion' }), /^\d+\./);
    const configDir = await native('Path', { cmd: 'resolvePath', path: '', directory: 21 });
    assert.equal(typeof configDir, 'string');
    console.log('PASS: native app and path APIs');
    // Paths use the same native API as the shipped Store constructor.
    const storePath = await native('Path', { cmd: 'resolve', paths: [configDir, 'config.json'] });
    const key = '__web_transport_test_' + Date.now();
    try {
        await call('plugin:store|set', { path: storePath, key, value: { unicode: 'Türkçe 中文', value: 42 } });
        assert.deepEqual(await call('plugin:store|get', { path: storePath, key }), { unicode: 'Türkçe 中文', value: 42 });
    } finally { await call('plugin:store|delete', { path: storePath, key }); }
    console.log('PASS: shared settings read/write and Unicode round trip');
    const eventName = 'web-test-' + Date.now(), handler = ++next;
    const delivered = new Promise(resolve => callbacks.set(handler, resolve));
    const eventId = await native('Event', { cmd: 'listen', event: eventName, windowLabel: null, handler });
    await native('Event', { cmd: 'emit', event: eventName, payload: { text: 'İlerleme ✓' } });
    assert.equal((await Promise.race([delivered, new Promise((_, reject) => setTimeout(() => reject(new Error('Event missing')), 3000))])).payload.text, 'İlerleme ✓');
    await native('Event', { cmd: 'unlisten', event: eventName, eventId });
    callbacks.delete(handler);
    console.log('PASS: native events delivered and listeners removed');
    assert.equal(typeof (await call('cache_stats')).entry_count, 'number');
    assert.ok(Array.isArray(await call('list_glossaries', { filter: null })));
    console.log('PASS: cache and glossary database commands');
    const bad = await call('extract_pdf_layout', { path: 'C:/__missing_web_test__.pdf' }).then(() => false, () => true);
    assert.ok(bad);
    console.log('PASS: native errors reach the browser');
    const fixture = path.resolve('tmp/web-fixtures/sample.pdf');
    if (await fs.stat(fixture).then(() => true, () => false)) {
        const layout = await call('extract_pdf_layout', { path: fixture });
        assert.equal(layout.length, 1);
        assert.match(JSON.stringify(layout), /Hello world/);
        const rendered = await call('render_pdf_page', { path: fixture, pageIndex: 0 });
        assert.equal(typeof rendered, 'string');
        for (const mode of ['translated', 'bilingual', 'sidebyside']) {
            const output = path.resolve(`tmp/web-fixtures/${mode}.pdf`);
            await call('export_translated_pdf', { sourcePdfPath: fixture, outputPath: output, mode, targetLang: 'tr', pages: [{ paragraphs: [{ translated: 'Merhaba dünya. Türkçe çeviri doğrulandı.', error: null }] }] });
            assert.equal((await fs.readFile(output)).subarray(0, 5).toString(), '%PDF-');
            const extracted = await call('extract_pdf_pages', { path: output });
            assert.match(extracted.join('\n'), /Merhaba/);
        }
        console.log('PASS: actual PDF extraction, rendering and all three native PDF exports');
        const epub = path.resolve('tmp/web-fixtures/sample.epub');
        const chapters = await call('extract_epub', { path: epub });
        assert.match(chapters.join('\n'), /Hello world/);
        for (const bilingual of [false, true]) {
            const output = path.resolve(`tmp/web-fixtures/translated-${bilingual}.epub`);
            await call('assemble_epub', { path: epub, outputPath: output, translations: { 'Hello world': 'Merhaba dünya', 'Keep every word.': 'Her sözcüğü koru.' }, bilingual });
            const result = (await call('extract_epub', { path: output })).join('\n');
            assert.match(result, /Merhaba dünya/);
            assert.equal(result.includes('Hello world'), bilingual);
        }
        console.log('PASS: EPUB extraction, translated and bilingual EPUB round trips');
    }
    console.log('All web bridge integration checks passed.');
} finally { running = false; await polling; }

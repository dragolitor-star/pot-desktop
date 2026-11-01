// Install the browser transport BEFORE importing any desktop module. Several
// modules subscribe to Tauri events at module scope.
const nativeFetch = window.fetch.bind(window);
const token = location.hash.slice(1) || sessionStorage.getItem('turktranslate-session');
if (token) sessionStorage.setItem('turktranslate-session', token);
history.replaceState(null, '', location.pathname + location.search);
window.__TT_BROWSER__ = true;
window.__TAURI_METADATA__ = { __currentWindow: { label: 'browser' }, __windows: [{ label: 'browser' }, { label: 'live_caption' }] };
let client = '', cursor = 0, stopped = false;
const pending = new Map();
const listeners = new Map();
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
async function request(path, body) {
    const response = await nativeFetch(path, {
        method: body === undefined ? 'GET' : 'POST',
        headers: { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json', 'X-Bridge-Client': client, 'X-Bridge-Cursor': String(cursor) },
        body: body === undefined ? undefined : JSON.stringify(body),
    });
    const result = await response.json();
    if (!response.ok) throw new Error(result.error || `HTTP ${response.status}`);
    return result;
}
function callback(id, value) {
    const entry = pending.get(id);
    if (entry) { clearTimeout(entry.timer); pending.delete(entry.callback); pending.delete(entry.error); }
    window[`_${id}`]?.(value);
}
function browserWindow(message) {
    const operation = message.data?.cmd?.type || message.cmd;
    const monitor = { name: 'Browser', size: { width: screen.width, height: screen.height }, position: { x: 0, y: 0 }, scaleFactor: devicePixelRatio };
    if (operation === 'currentMonitor' || operation === 'primaryMonitor') return monitor;
    if (operation === 'availableMonitors') return [monitor];
    if (operation === 'scaleFactor') return devicePixelRatio;
    if (operation.endsWith('Position')) return { x: 0, y: 0 };
    if (operation.endsWith('Size')) return { width: innerWidth, height: innerHeight };
    if (operation === 'isVisible' || operation === 'isDecorated' || operation === 'isResizable') return true;
    if (operation.startsWith('is')) return false;
    if (operation === 'close') window.dispatchEvent(new CustomEvent('tt:navigate', { detail: { page: 'text' } }));
    // Browser chrome owns resize, focus, pinning and decorations.
    return null;
}
window.__TAURI__ = {
    convertFileSrc(path) { return `/api/asset?path=${encodeURIComponent(path)}`; },
};
window.__TAURI_IPC__ = message => {
    if (stopped) { callback(message.error, 'Bağlantı kesildi. Sayfayı yenileyin.'); return; }
    const pages = { document_window: 'document', subtitle_window: 'subtitle', updater_window: 'updater' };
    if (pages[message.cmd]) {
        window.dispatchEvent(new CustomEvent('tt:navigate', { detail: { page: pages[message.cmd] } }));
        callback(message.callback, null); return;
    }
    if (message.__tauriModule === 'Window' && message.message?.data?.label !== 'live_caption') {
        callback(message.callback, browserWindow(message.message)); return;
    }
    const entry = { callback: message.callback, error: message.error };
    entry.timer = setTimeout(() => callback(message.error, 'İşlem zaman aşımına uğradı.'), 30 * 60 * 1000);
    pending.set(message.callback, entry); pending.set(message.error, entry);
    const event = message.__tauriModule === 'Event' ? message.message : null;
    if (event?.cmd === 'listen') {
        const page = window.__TT_ACTIVE_VIEW__;
        const original = window[`_${message.callback}`];
        Object.defineProperty(window, `_${message.callback}`, { configurable: true, value: id => { listeners.set(id, { handler: event.handler, event: event.event, page }); original(id); } });
    }
    if (event?.cmd === 'unlisten') {
        const handler = listeners.get(event.eventId)?.handler;
        if (handler) { delete window[`_${handler}`]; listeners.delete(event.eventId); }
    }
    request('/api/bridge', message).catch(error => callback(message.error, error.message));
};
window.__TT_UPLOAD__ = async file => {
    if (file.size > 2 * 1024 * 1024 * 1024) throw new Error('Dosya en fazla 2 GB olabilir.');
    const response = await nativeFetch('/api/bridge/upload', { method: 'POST', headers: { Authorization: `Bearer ${token}`, 'X-Bridge-Client': client, 'X-File-Name': encodeURIComponent(file.name), 'Content-Type': 'application/octet-stream' }, body: file });
    const result = await response.json();
    if (!response.ok) throw new Error(result.error);
    return result.path;
};
window.__TT_FILE_DROP__ = path => {
    for (const [id, entry] of listeners) {
        if (entry.event === 'tauri://file-drop' && entry.page === window.__TT_ACTIVE_VIEW__) window[`_${entry.handler}`]?.({ id, event: entry.event, windowLabel: 'browser', payload: [path] });
    }
};
async function poll() {
    let failures = 0;
    while (!stopped) {
        try {
            const result = await request('/api/bridge/events');
            for (const item of result.events) {
                if (item.id <= cursor) continue;
                cursor = item.id;
                try { callback(item.event.callback, item.event.payload); } catch (error) { console.error(error); }
            }
            failures = 0;
        } catch (error) {
            if (++failures >= 10) {
                stopped = true;
                for (const entry of new Set(pending.values())) callback(entry.error, error.message);
                window.dispatchEvent(new CustomEvent('tt:disconnected', { detail: error.message }));
            }
        }
        await sleep(document.hidden ? 1000 : 100);
    }
}
async function start() {
    if (!token) throw new Error('Oturum bulunamadı. Sistem tepsisinden “Web arayüzünü aç” seçeneğini kullanın.');
    for (let attempt = 0; ; attempt++) {
        try { ({ client } = await request('/api/bridge/connect', {})); break; }
        catch (error) { if (attempt >= 30) throw error; await sleep(500); }
    }
    await request('/api/session', {});
    void poll();
    const { invoke } = await import('@tauri-apps/api/tauri');
    const { listen } = await import('@tauri-apps/api/event');
    const { createNativeFetch } = await import('./fetch.js');
    window.fetch = createNativeFetch({ nativeFetch, invoke, listen, origin: location.origin });
    await import('../main.jsx');
}
start().catch(error => {
    const root = document.getElementById('root');
    root.textContent = error.message;
    root.style.cssText = 'padding:40px;font:16px system-ui;color:#a00';
});

import { invoke, transformCallback } from '@tauri-apps/api/tauri';
import { listen } from '@tauri-apps/api/event';

const clients = new Map();
const active = new Map();
const jobs = { translate_document: ['document', 'cancel_document_translation'], transcribe_media: ['media', 'cancel_transcription'], export_layout_pdf: ['layout', 'cancel_layout_export'], start_live_caption: ['live', 'stop_live_caption'] };
const cancellations = Object.fromEntries(Object.values(jobs).map(([group, cancel]) => [cancel, group]));
function clientState(id) {
    if (!clients.has(id)) clients.set(id, { listeners: new Map(), watchers: new Set(), live: false });
    return clients.get(id);
}
async function publish(client, callback, payload, completed = false) {
    // `callback` and `error` are reserved IPC fields in Tauri 1.
    await invoke('web_bridge_publish', { client, callbackId: callback, payload: payload ?? null, completed });
}
async function dispose(client) {
    const state = clients.get(client);
    if (!state) return;
    clients.delete(client);
    for (const [group, job] of active) {
        if (job.client === client) { await invoke(job.cancel); active.delete(group); }
    }
    for (const [eventId, { event, handler }] of state.listeners) {
        await invoke('tauri', { __tauriModule: 'Event', message: { cmd: 'unlisten', event, eventId } });
        delete window[`_${handler}`];
    }
    for (const id of state.watchers) await invoke('plugin:fs-watch|unwatch', { id });
}
async function start() {
for (const [event, group] of [['document://done', 'document'], ['media://done', 'media'], ['layout_export://done', 'layout'], ['live_caption://state', 'live']]) {
    await listen(event, ({ payload }) => {
        if (group === 'live' && !['stopped', 'error'].includes(payload.state)) return;
        if (active.get(group)?.runId === payload.runId) active.delete(group);
    });
}
await listen('web_bridge://disconnect', ({ payload }) => { dispose(payload).catch(console.error); });
await listen('web_bridge://request', async ({ payload: { client, message } }) => {
    const state = clientState(client);
    const { cmd, callback, error, ...args } = message;
    if (cmd === 'web_fetch_start' || cmd === 'web_fetch_cancel') args.client = client;
    let handler;
    let reserved;
    try {
        if (jobs[cmd]) {
            const [group, cancel] = jobs[cmd];
            if (active.has(group)) throw new Error('Bu işlem başka bir ekranda sürüyor. Önce tamamlanmasını bekleyin veya durdurun.');
            reserved = group;
            active.set(group, { client, cancel, runId: args.request?.runId });
        }
        if (cancellations[cmd] && active.has(cancellations[cmd]) && active.get(cancellations[cmd]).client !== client) throw new Error('İşlemi başlatan sekmeden durdurun.');
        const event = args.__tauriModule === 'Event' ? args.message : null;
        if (event?.cmd === 'listen') {
            const browserHandler = event.handler;
            handler = transformCallback(value => { publish(client, browserHandler, value).catch(console.error); });
            event.handler = handler;
            // The native worker owns window-scoped filesystem watchers.
            if (event.windowLabel) event.windowLabel = 'web_bridge';
        }
        if (event?.cmd === 'unlisten' && !state.listeners.has(event.eventId)) throw new Error('Listener belongs to another session');
        if (cmd === 'plugin:fs-watch|unwatch' && !state.watchers.has(args.id)) throw new Error('Watcher belongs to another session');
        const result = await invoke(cmd, args);
        if (event?.cmd === 'listen') state.listeners.set(result, { event: event.event, handler });
        if (event?.cmd === 'unlisten') {
            delete window[`_${state.listeners.get(event.eventId).handler}`];
            state.listeners.delete(event.eventId);
        }
        if (cmd === 'plugin:fs-watch|watch') state.watchers.add(args.id);
        if (cmd === 'plugin:fs-watch|unwatch') state.watchers.delete(args.id);
        if (cmd === 'start_live_caption') state.live = true;
        if (cmd === 'stop_live_caption') state.live = false;
        await publish(client, callback, result, true);
    } catch (reason) {
        if (reserved) active.delete(reserved);
        if (handler) delete window[`_${handler}`];
        await publish(client, error, String(reason?.message ?? reason), true).catch(console.error);
    }
});
await invoke('web_bridge_ready');
}
start().catch(console.error);

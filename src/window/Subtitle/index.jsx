import { Button, Card, CardBody, CardHeader, Dropdown, DropdownTrigger, DropdownMenu, DropdownItem, Input, Progress, Select, SelectItem, Switch, Tooltip } from '@nextui-org/react';
import React, { useState, useEffect, useRef } from 'react';
import { save, open } from '@tauri-apps/api/dialog';
import { writeTextFile } from '@tauri-apps/api/fs';
import { invoke } from '@tauri-apps/api/tauri';
import toast, { Toaster } from 'react-hot-toast';
import { useTranslation } from 'react-i18next';
import { MdMovie, MdTranslate, MdCancel, MdFileDownload, MdArrowBack, MdGraphicEq, MdHearing, MdStop } from 'react-icons/md';
import { listen, emit } from '@tauri-apps/api/event';
import { appWindow, WebviewWindow } from '@tauri-apps/api/window';

import { useToastStyle, useConfig } from '../../hooks';
import { store } from '../../utils/store';
import * as builtinServices from '../../services/translate';
import { fetchActiveGlossary, applyGlossaryPostTranslate, BUILTIN_LLM_ENGINES } from '../../utils/glossary';
import { getServiceName, whetherPluginService } from '../../utils/service_instance';
import { getEngineModelId, getCachedTranslation, setCachedTranslation, RpmLimiter } from '../../utils/translationCache';
import { buildSrt, buildVtt } from '../../utils/subtitle';

const LANG_OPTIONS = [
    { value: 'auto', label: 'Auto Detect' },
    { value: 'en', label: 'English' },
    { value: 'zh_cn', label: '简体中文' },
    { value: 'zh_tw', label: '繁體中文' },
    { value: 'ja', label: '日本語' },
    { value: 'ko', label: '한국어' },
    { value: 'fr', label: 'Français' },
    { value: 'es', label: 'Español' },
    { value: 'ru', label: 'Русский' },
    { value: 'de', label: 'Deutsch' },
    { value: 'it', label: 'Italiano' },
    { value: 'tr', label: 'Türkçe' },
    { value: 'pt_pt', label: 'Português' },
    { value: 'vi', label: 'Tiếng Việt' },
];

const AUDIO_EXT = ['mp3', 'wav', 'm4a', 'aac', 'ogg', 'oga', 'flac', 'aiff', 'aif', 'opus'];
const VIDEO_EXT = ['mp4', 'm4v', 'mov', 'mkv', 'webm', 'avi', 'mpeg', 'mpg', 'wmv', 'flv', '3gp'];
const MEDIA_EXT = [...AUDIO_EXT, ...VIDEO_EXT];

function mediaKindOf(path) {
    const ext = String(path).toLowerCase().split('.').pop();
    if (VIDEO_EXT.includes(ext)) return 'video';
    if (AUDIO_EXT.includes(ext)) return 'audio';
    return '';
}

// HH:MM:SS for the on-screen cue list (compact, no milliseconds).
function clockOf(seconds) {
    const total = Math.max(0, Math.floor(Number(seconds) || 0));
    const h = Math.floor(total / 3600);
    const m = Math.floor((total % 3600) / 60);
    const s = total % 60;
    const pad = (n) => String(n).padStart(2, '0');
    return h > 0 ? `${pad(h)}:${pad(m)}:${pad(s)}` : `${pad(m)}:${pad(s)}`;
}

export default function Subtitle() {
    const { t } = useTranslation();
    const toastStyle = useToastStyle();

    useEffect(() => {
        if (appWindow.label === 'subtitle') {
            appWindow.show();
        }
    }, []);

    const [translateServiceInstanceList] = useConfig('translate_service_list', ['google']);

    const [filePath, setFilePath] = useState('');
    const [mediaKind, setMediaKind] = useState(''); // 'audio' | 'video'
    const [status, setStatus] = useState('idle'); // idle | uploading | transcribing | translating | done
    const [progressPercent, setProgressPercent] = useState(0);
    const [progressText, setProgressText] = useState('');
    const [cues, setCues] = useState([]); // { start, end, original, translated, translating, error }
    const [sourceLang, setSourceLang] = useState('auto');
    const [targetLang, setTargetLang] = useState('tr');
    const [selectedEngine, setSelectedEngine] = useState('');
    const [translateEngine, setTranslateEngine] = useState(''); // cue translation engine (any builtin; Gemini-only applies to transcription)
    const [charsPerRequest, setCharsPerRequest] = useState(6000);
    const [rpmLimit, setRpmLimit] = useState(10);
    const [cacheHits, setCacheHits] = useState(0);

    // Live caption (Task B) — separate, additive flow. Transcription is Gemini-only
    // (same constraint as file transcription), captured from system-audio loopback.
    const [liveActive, setLiveActive] = useState(false);
    const [liveState, setLiveState] = useState('idle');
    const [liveMessage, setLiveMessage] = useState('');
    const [liveChunkSeconds, setLiveChunkSeconds] = useState(5);
    const [liveClickThrough, setLiveClickThrough] = useState(false);
    const liveRunIdRef = useRef(0);
    const liveUnlistenRef = useRef(null);

    const cancelRef = useRef(false);

    // Tidy the live-caption status listener if the window is closed mid-session.
    useEffect(() => () => {
        if (liveUnlistenRef.current) liveUnlistenRef.current();
    }, []);

    useEffect(() => {
        if (translateServiceInstanceList && translateServiceInstanceList.length > 0) {
            if (!selectedEngine) {
                // Prefer a Gemini instance — transcription is Gemini-only.
                const gemini = translateServiceInstanceList.find((e) => getServiceName(e) === 'geminipro');
                setSelectedEngine(gemini || translateServiceInstanceList[0]);
            }
            // Cue translation may use any engine; default to the first configured instance.
            if (!translateEngine) {
                setTranslateEngine(translateServiceInstanceList[0]);
            }
        }
    }, [translateServiceInstanceList, selectedEngine, translateEngine]);

    useEffect(() => {
        const unlistenDrop = listen('tauri://file-drop', (event) => {
            const files = event.payload;
            if (Array.isArray(files) && files.length > 0) {
                const first = files[0];
                const kind = mediaKindOf(first);
                if (kind) {
                    setFilePath(first);
                    setMediaKind(kind);
                    toast.success(kind === 'video' ? 'Video file loaded' : 'Audio file loaded');
                } else {
                    toast.error('Only audio/video files are supported');
                }
            }
        });
        return () => {
            unlistenDrop.then((f) => f());
        };
    }, []);

    const handleSelectFile = async () => {
        try {
            const selected = await open({
                filters: [{ name: 'Media', extensions: MEDIA_EXT }],
                multiple: false,
            });
            if (selected) {
                const kind = mediaKindOf(selected) || 'audio';
                setFilePath(selected);
                setMediaKind(kind);
                toast.success(kind === 'video' ? 'Video file loaded' : 'Audio file loaded');
            }
        } catch (e) {
            toast.error(String(e), { style: toastStyle });
        }
    };

    // Transcription is Gemini-only (audio/video understanding). Validate the engine
    // up front and surface a clear message instead of failing mid-upload.
    const resolveGeminiConfig = async () => {
        const engineName = getServiceName(selectedEngine);
        if (whetherPluginService(selectedEngine) || engineName !== 'geminipro') {
            throw new Error('Subtitle transcription requires a Gemini (geminipro) service. Pick a Gemini instance.');
        }
        if (typeof builtinServices.geminipro?.resolveModelAndBase !== 'function') {
            throw new Error('Gemini service is unavailable.');
        }
        const instanceConfig = (await store.get(selectedEngine)) ?? {};
        if (!instanceConfig.apiKey) {
            throw new Error('The selected Gemini service has no API key configured.');
        }
        const { model, base } = builtinServices.geminipro.resolveModelAndBase(instanceConfig);
        return { engineName, instanceConfig, model, base };
    };

    // The translation step accepts ANY builtin engine (unlike transcription). Gemini
    // routes through the Rust translate_document pipeline; every other engine is
    // translated per-cue in the renderer. Plugins are not supported.
    const resolveTranslateConfig = async (engineOverride) => {
        const engine = engineOverride || translateEngine || selectedEngine;
        if (!engine) {
            throw new Error('No translation engine is configured.');
        }
        if (whetherPluginService(engine)) {
            throw new Error('.potext plugins are not supported for subtitle translation. Pick a builtin engine.');
        }
        const engineName = getServiceName(engine);
        const service = builtinServices[engineName];
        if (!service || typeof service.translate !== 'function' || !service.Language) {
            throw new Error(`Engine "${engineName}" cannot translate subtitles.`);
        }
        const instanceConfig = (await store.get(engine)) ?? {};
        const isGemini = engineName === 'geminipro' && typeof service.resolveModelAndBase === 'function';
        let model = '';
        let base = '';
        if (isGemini) {
            if (!instanceConfig.apiKey) {
                throw new Error('The selected Gemini service has no API key configured.');
            }
            ({ model, base } = service.resolveModelAndBase(instanceConfig));
        }
        return { engineName, instanceConfig, isGemini, model, base };
    };

    const handleTranscribe = async ({ chain = false } = {}) => {
        if (!filePath) {
            toast.error('Please load an audio or video file first');
            return;
        }
        cancelRef.current = false;
        setCues([]);
        setCacheHits(0);
        setStatus('uploading');
        setProgressPercent(5);
        setProgressText('Preparing transcription…');

        let unlistenProgress;
        let unlistenCues;
        let unlistenDone;
        let latestCues = [];
        let completed = false;
        try {
            const { model, base, instanceConfig } = await resolveGeminiConfig();
            const LanguageEnum = builtinServices.geminipro.Language;
            const runId = Date.now();

            let resolveDone;
            const donePromise = new Promise((r) => { resolveDone = r; });
            let receivedCueCount = 0; // avoids reading stale `cues` state in the closure below

            const onProgress = (event) => {
                const p = event.payload;
                if (!p || p.runId !== runId) return;
                if (typeof p.percent === 'number') setProgressPercent(Math.max(5, Math.min(99, p.percent)));
                if (p.text) setProgressText(p.text);
                setStatus('transcribing');
            };
            const onCues = (event) => {
                const p = event.payload;
                if (!p || p.runId !== runId || !Array.isArray(p.cues)) return;
                receivedCueCount = p.cues.length;
                latestCues = p.cues.map((c) => ({
                    start: c.start,
                    end: c.end,
                    original: c.text,
                    translated: '',
                    translating: false,
                }));
                setCues(latestCues);
            };
            const onDone = (event) => {
                const p = event.payload;
                if (!p || p.runId !== runId) return;
                resolveDone(p);
            };

            unlistenProgress = await listen('media://progress', onProgress);
            unlistenCues = await listen('media://cues', onCues);
            unlistenDone = await listen('media://done', onDone);

            invoke('transcribe_media', {
                request: {
                    runId,
                    path: filePath,
                    apiKey: instanceConfig.apiKey,
                    apiBase: base,
                    model,
                    languageHint: sourceLang === 'auto' ? '' : LanguageEnum[sourceLang],
                },
            }).catch((err) => resolveDone({ status: 'error', message: String(err) }));

            const outcome = await donePromise;

            if (outcome.status === 'completed') {
                completed = true;
                setStatus('done');
                setProgressPercent(100);
                setProgressText(`Transcribed ${outcome.count} subtitle segments.`);
                if (!chain) toast.success(`Transcription complete — ${outcome.count} segments.`);
            } else if (outcome.status === 'rate_limited') {
                setStatus(receivedCueCount > 0 ? 'done' : 'idle');
                setProgressText('Stopped — provider rate limit / daily quota reached.');
                toast.error('Stopped: Gemini returned 429 (rate limit / daily quota). Try again after it resets.', { style: toastStyle, duration: 10000 });
            } else if (outcome.status === 'cancelled') {
                setStatus('idle');
                toast.error('Transcription cancelled.');
            } else {
                setStatus('idle');
                toast.error(String(outcome.message || 'Transcription failed.'), { style: toastStyle, duration: 10000 });
            }
        } catch (e) {
            setStatus('idle');
            toast.error(String(e), { style: toastStyle, duration: 8000 });
        } finally {
            if (unlistenProgress) unlistenProgress();
            if (unlistenCues) unlistenCues();
            if (unlistenDone) unlistenDone();
        }

        // One-click flow: chain straight into translation with the same validated
        // Gemini instance so a single action yields a translated subtitle file.
        if (chain && completed && !cancelRef.current && latestCues.length > 0) {
            await handleTranslate(latestCues, selectedEngine);
        }
    };

    // Translate the transcribed cue text, reusing the Rust `translate_document`
    // pipeline (dedup + structured Gemini batches) the document flow uses. Unique
    // cue texts are translated once and fanned out to every matching cue.
    const handleTranslate = async (cuesArg, engineOverride) => {
        const sourceCues = Array.isArray(cuesArg) ? cuesArg : cues;
        if (sourceCues.length === 0) return;
        cancelRef.current = false;
        setStatus('translating');
        setProgressPercent(10);
        setProgressText('Starting subtitle translation…');

        let unlistenProgress;
        let unlistenBatch;
        let unlistenDone;
        try {
            const { engineName, instanceConfig, isGemini, model, base } = await resolveTranslateConfig(engineOverride);
            const LanguageEnum = builtinServices[engineName].Language;
            const modelId = getEngineModelId(engineName, instanceConfig);

            const glossarySrc = sourceLang === 'auto' ? 'en' : sourceLang;
            const glossaryEntries = await fetchActiveGlossary(glossarySrc, targetLang, 'document');
            const glossaryPayload = (glossaryEntries || []).map((e) => ({
                sourceTerm: e.source_term,
                targetTerm: e.target_term,
            }));

            // DEDUP unique cue texts; resolve cache hits, queue the rest.
            const translationByText = new Map();
            const uniqueTexts = [];
            for (const cue of sourceCues) {
                const text = (cue.original || '').trim();
                if (text.length === 0) {
                    translationByText.set(cue.original, '');
                    continue;
                }
                if (!translationByText.has(cue.original)) {
                    translationByText.set(cue.original, null);
                    uniqueTexts.push(cue.original);
                }
            }

            setCues((prev) => prev.map((c) => (c.original.trim().length > 0 ? { ...c, translating: true } : c)));

            const needTranslate = [];
            let localCacheHits = 0;
            for (const text of uniqueTexts) {
                if (cancelRef.current) break;
                const cached = await getCachedTranslation(modelId, sourceLang, targetLang, text);
                if (cached !== null && cached !== undefined) {
                    translationByText.set(text, cached);
                    localCacheHits++;
                    setCacheHits(localCacheHits);
                } else {
                    needTranslate.push(text);
                }
            }

            let doneUnique = uniqueTexts.length - needTranslate.length;
            let rateLimited = false;

            if (isGemini) {
                // GEMINI PATH — reuse the Rust translate_document pipeline (structured
                // batches + dedup); results stream back via document://* events.
                const runId = Date.now();
                let resolveDone;
                const donePromise = new Promise((r) => { resolveDone = r; });

                const onProgress = (event) => {
                    const p = event.payload;
                    if (!p || p.runId !== runId) return;
                    setProgressText(`${p.text} · ${localCacheHits} cache hits`);
                };
                const onBatch = (event) => {
                    const p = event.payload;
                    if (!p || p.runId !== runId) return;
                    for (const r of p.results) {
                        if (r.translation !== null && r.translation !== undefined && r.translation.length > 0) {
                            translationByText.set(r.source, r.translation);
                            setCachedTranslation(modelId, sourceLang, targetLang, r.source, r.translation);
                        } else {
                            translationByText.set(r.source, r.error ? '__ERROR__:' + r.error : '__MISMATCH__');
                        }
                        doneUnique++;
                    }
                    setProgressPercent(Math.min(10 + Math.floor((doneUnique / Math.max(1, uniqueTexts.length)) * 90), 100));
                };
                const onDone = (event) => {
                    const p = event.payload;
                    if (!p || p.runId !== runId) return;
                    resolveDone(p);
                };

                unlistenProgress = await listen('document://progress', onProgress);
                unlistenBatch = await listen('document://batch', onBatch);
                unlistenDone = await listen('document://done', onDone);

                if (needTranslate.length === 0) {
                    resolveDone({ status: 'completed' });
                } else {
                    invoke('translate_document', {
                        request: {
                            runId,
                            texts: needTranslate,
                            apiKey: instanceConfig.apiKey,
                            apiBase: base,
                            model,
                            fromLabel: LanguageEnum[sourceLang],
                            toLabel: LanguageEnum[targetLang],
                            charsPerRequest: Math.max(1000, charsPerRequest | 0),
                            rpmLimit: Math.max(1, rpmLimit | 0),
                            glossary: glossaryPayload,
                        },
                    }).catch((err) => resolveDone({ status: 'error', message: String(err) }));
                }

                const outcome = await donePromise;
                if (outcome.status === 'rate_limited') {
                    rateLimited = true;
                } else if (outcome.status === 'cancelled') {
                    cancelRef.current = true;
                } else if (outcome.status === 'error') {
                    for (const text of needTranslate) {
                        if (translationByText.get(text) === null) {
                            translationByText.set(text, '__ERROR__:' + outcome.message);
                        }
                    }
                }
            } else {
                // NON-GEMINI PATH — translate each unique cue in the renderer, paced under
                // the RPM ceiling with exponential-backoff retry (mirrors the Document
                // window's per-paragraph path). Each success is cached for re-run reuse.
                const rpmLimiter = new RpmLimiter(Math.max(1, rpmLimit | 0));
                const translateCue = async (text) => {
                    const maxRetries = 3;
                    let attempt = 0;
                    let delay = 1000;
                    while (attempt <= maxRetries) {
                        if (cancelRef.current) throw new Error('cancelled');
                        try {
                            let finalResult = '';
                            return await new Promise((resolve, reject) => {
                                builtinServices[engineName].translate(
                                    text,
                                    LanguageEnum[sourceLang],
                                    LanguageEnum[targetLang],
                                    {
                                        config: instanceConfig,
                                        detect: sourceLang === 'auto' ? 'en' : sourceLang,
                                        glossaryEntries,
                                        setResult: (v) => { finalResult = v; },
                                    }
                                ).then((res) => {
                                    let val = res || finalResult;
                                    if (typeof val === 'string' && !BUILTIN_LLM_ENGINES.includes(engineName)) {
                                        val = applyGlossaryPostTranslate(val, glossaryEntries);
                                    }
                                    resolve(val);
                                }).catch(reject);
                            });
                        } catch (err) {
                            attempt++;
                            if (attempt > maxRetries || cancelRef.current) throw err;
                            await new Promise((r) => setTimeout(r, delay));
                            delay *= 2;
                        }
                    }
                };

                for (const text of needTranslate) {
                    if (cancelRef.current) break;
                    setProgressText(`Translating ${Math.min(doneUnique + 1, uniqueTexts.length)}/${uniqueTexts.length} · ${localCacheHits} cache hits`);
                    try {
                        await rpmLimiter.acquire();
                        const translated = await translateCue(text);
                        if (typeof translated === 'string' && translated.length > 0) {
                            translationByText.set(text, translated);
                            setCachedTranslation(modelId, sourceLang, targetLang, text, translated);
                        } else {
                            translationByText.set(text, '__MISMATCH__');
                        }
                    } catch (err) {
                        const errStr = String(err);
                        if (errStr === 'Error: cancelled') {
                            cancelRef.current = true;
                            break;
                        }
                        translationByText.set(text, '__ERROR__:' + errStr);
                        // A 429 means the rate limit / daily quota is hit — stop now; the
                        // cues done so far are cached so a re-run continues where it left off.
                        if (errStr.includes('429') || /too many requests|rate limit|quota|resource exhausted/i.test(errStr)) {
                            rateLimited = true;
                            break;
                        }
                    }
                    doneUnique++;
                    setProgressPercent(Math.min(10 + Math.floor((doneUnique / Math.max(1, uniqueTexts.length)) * 90), 100));
                }
            }

            // FAN OUT to every cue.
            setCues((prev) => prev.map((c) => {
                const tv = translationByText.get(c.original);
                const next = { ...c, translating: false };
                if (tv === null || tv === undefined) {
                    if (!cancelRef.current) next.error = 'Not translated';
                } else if (typeof tv === 'string' && tv.startsWith('__ERROR__:')) {
                    next.error = tv.slice('__ERROR__:'.length);
                } else if (tv === '__MISMATCH__') {
                    next.error = 'Segment mismatch during batch translation.';
                } else {
                    next.translated = tv;
                    next.error = undefined;
                }
                return next;
            }));

            if (cancelRef.current) {
                setStatus('done');
                toast.error('Translation cancelled.');
            } else if (rateLimited) {
                setStatus('done');
                setProgressText('Stopped — provider rate limit / daily quota reached.');
                toast.error('Stopped: the provider returned 429 (rate limit / daily quota). Translated segments so far are cached — re-run after the limit resets.', { style: toastStyle, duration: 10000 });
            } else {
                setStatus('done');
                setProgressPercent(100);
                setProgressText('Subtitle translation completed!');
                toast.success('Subtitles translated!');
            }
        } catch (e) {
            setStatus('done');
            setCues((prev) => prev.map((c) => ({ ...c, translating: false })));
            toast.error(String(e), { style: toastStyle, duration: 8000 });
        } finally {
            if (unlistenProgress) unlistenProgress();
            if (unlistenBatch) unlistenBatch();
            if (unlistenDone) unlistenDone();
        }
    };

    const handleCancel = () => {
        cancelRef.current = true;
        invoke('cancel_transcription').catch(() => {});
        invoke('cancel_document_translation').catch(() => {});
    };

    const handleReset = () => {
        setFilePath('');
        setMediaKind('');
        setCues([]);
        setStatus('idle');
        setProgressPercent(0);
        setProgressText('');
        setCacheHits(0);
    };

    // ── Live caption (system-audio loopback) ──────────────────────────────────
    const handleStartLive = async () => {
        try {
            // Transcription is Gemini-only; validate the engine + key up front.
            const { model, base, instanceConfig } = await resolveGeminiConfig();
            const LanguageEnum = builtinServices.geminipro.Language;

            const glossarySrc = sourceLang === 'auto' ? 'en' : sourceLang;
            const glossaryEntries = await fetchActiveGlossary(glossarySrc, targetLang, 'document');
            const glossaryPayload = (glossaryEntries || []).map((e) => ({
                sourceTerm: e.source_term,
                targetTerm: e.target_term,
            }));

            const runId = Date.now();
            liveRunIdRef.current = runId;

            // Open the transparent overlay and reflect lifecycle here.
            await invoke('live_caption_window');
            if (liveUnlistenRef.current) liveUnlistenRef.current();
            liveUnlistenRef.current = await listen('live_caption://state', (event) => {
                const p = event.payload;
                if (!p || p.runId !== runId) return;
                setLiveState(p.state || '');
                setLiveMessage(p.message || '');
                if (p.state === 'stopped' || p.state === 'error') setLiveActive(false);
            });

            await invoke('start_live_caption', {
                request: {
                    runId,
                    apiKey: instanceConfig.apiKey,
                    apiBase: base,
                    model,
                    fromLabel: sourceLang === 'auto' ? 'Auto' : LanguageEnum[sourceLang],
                    toLabel: LanguageEnum[targetLang],
                    chunkSeconds: Math.max(2, Math.min(15, liveChunkSeconds | 0)),
                    glossary: glossaryPayload,
                },
            });
            setLiveActive(true);
            setLiveState('listening');
            setLiveClickThrough(false);
            toast.success(t('config.subtitle.live.started'), { style: toastStyle, duration: 6000 });
        } catch (e) {
            toast.error(String(e?.message || e), { style: toastStyle, duration: 8000 });
        }
    };

    const handleStopLive = async () => {
        try {
            await invoke('stop_live_caption');
        } catch {
            // already stopped
        }
        try {
            const w = WebviewWindow.getByLabel('live_caption');
            if (w) await w.close();
        } catch {
            // overlay may already be closed
        }
        if (liveUnlistenRef.current) {
            liveUnlistenRef.current();
            liveUnlistenRef.current = null;
        }
        setLiveActive(false);
        setLiveState('idle');
        setLiveClickThrough(false);
    };

    // Click-through is authoritative from here (the overlay can enable it but can't
    // un-toggle itself once the mouse passes through). The overlay listens for this.
    const handleToggleLivePassthrough = async (enabled) => {
        setLiveClickThrough(enabled);
        try {
            await emit('live_caption://passthrough', { enabled });
        } catch {
            // overlay not open
        }
    };

    const liveStatusLabel = () => {
        if (liveState === 'transcribing') return t('config.subtitle.live.transcribing');
        if (liveState === 'error') return t('config.subtitle.live.error');
        if (liveState === 'stopped') return t('config.subtitle.live.stopped');
        return t('config.subtitle.live.listening');
    };

    // Export SRT/VTT. key format: "<format>:<mode>" e.g. "srt:translated".
    const handleExport = async (key) => {
        try {
            if (cues.length === 0) return;
            const [format, mode] = String(key).split(':');
            const content = format === 'vtt' ? buildVtt(cues, mode) : buildSrt(cues, mode);
            if (!content.trim()) {
                toast.error('Nothing to export yet.', { style: toastStyle });
                return;
            }
            const baseName = (filePath.split(/[\\/]/).pop() || 'subtitles').replace(/\.[^.]+$/, '');
            const suffix = mode === 'original' ? sourceLang : mode === 'bilingual' ? `${targetLang}_bilingual` : targetLang;
            const savePath = await save({
                filters: [{ name: format.toUpperCase(), extensions: [format] }],
                defaultPath: `${baseName}_${suffix}.${format}`,
            });
            if (!savePath) return;
            await writeTextFile(savePath, content);
            toast.success(`${format.toUpperCase()} saved to ${savePath}`, { style: toastStyle, duration: 6000 });
        } catch (e) {
            toast.error('Export failed: ' + String(e), { style: toastStyle, duration: 8000 });
        }
    };

    const hasTranslation = cues.some((c) => c.translated && c.translated.length > 0);
    const busy = status === 'uploading' || status === 'transcribing' || status === 'translating';

    return (
        <div className='flex flex-col h-screen w-screen bg-background text-foreground overflow-hidden'>
            <Toaster />

            <div className='flex justify-between items-center h-[50px] px-4 border-b border-default-100 bg-content1 shadow-sm select-none' data-tauri-drag-region='true'>
                <div className='flex items-center gap-2'>
                    <MdGraphicEq className='text-[24px] text-primary' />
                    <span className='font-semibold text-lg'>Audio / Video Subtitles (Gemini)</span>
                </div>
            </div>

            <div className='flex-1 flex flex-col p-4 overflow-hidden gap-4'>
                {/* Landing */}
                {status === 'idle' && cues.length === 0 && (
                    <div className='flex-1 flex flex-col justify-center items-center border-2 border-dashed border-default-300 rounded-xl bg-content1/50 backdrop-blur-md p-8 gap-6 transition-all hover:border-primary'>
                        <MdMovie className='text-[72px] text-default-400 animate-bounce' />
                        <div className='text-center'>
                            <p className='text-xl font-medium'>Drag & drop an audio or video file here</p>
                            <p className='text-sm text-default-400 mt-1'>Gemini transcribes the speech; "Transcribe &amp; Translate" also renders it into your target language in one step. Files only — no live capture.</p>
                        </div>
                        <Button color='primary' variant='flat' onPress={handleSelectFile}>
                            Browse Files
                        </Button>

                        {filePath && (
                            <Card className='max-w-[420px] border border-primary/20 bg-content2'>
                                <CardBody className='flex flex-row items-center gap-3 p-3'>
                                    {mediaKind === 'video' ? <MdMovie className='text-primary text-[24px] flex-shrink-0' /> : <MdGraphicEq className='text-primary text-[24px] flex-shrink-0' />}
                                    <div className='overflow-hidden'>
                                        <p className='font-semibold text-sm truncate'>{filePath.split(/[\\/]/).pop()}</p>
                                        <p className='text-xs text-default-400 truncate'>{filePath}</p>
                                    </div>
                                </CardBody>
                            </Card>
                        )}

                        {filePath && (
                            <div className='flex gap-4 items-center w-full max-w-[640px] mt-4 flex-wrap justify-center'>
                                <Select
                                    label='Spoken Language'
                                    size='sm'
                                    variant='bordered'
                                    selectedKeys={[sourceLang]}
                                    onSelectionChange={(keys) => setSourceLang([...keys][0])}
                                    className='max-w-[160px]'
                                >
                                    {LANG_OPTIONS.map((l) => (
                                        <SelectItem key={l.value} value={l.value}>
                                            {l.label}
                                        </SelectItem>
                                    ))}
                                </Select>
                                <Select
                                    label='Translate To'
                                    size='sm'
                                    variant='bordered'
                                    selectedKeys={[targetLang]}
                                    onSelectionChange={(keys) => setTargetLang([...keys][0])}
                                    className='max-w-[160px]'
                                >
                                    {LANG_OPTIONS.slice(1).map((l) => (
                                        <SelectItem key={l.value} value={l.value}>
                                            {l.label}
                                        </SelectItem>
                                    ))}
                                </Select>
                                <Select
                                    label='Gemini Engine'
                                    size='sm'
                                    variant='bordered'
                                    selectedKeys={selectedEngine ? [selectedEngine] : []}
                                    onSelectionChange={(keys) => setSelectedEngine([...keys][0])}
                                    className='max-w-[200px]'
                                >
                                    {translateServiceInstanceList.map((e) => (
                                        <SelectItem key={e} value={e}>
                                            {e}
                                        </SelectItem>
                                    ))}
                                </Select>
                                <Tooltip content='Max requests/minute for the translation step. Gemini free-tier safe = 10.'>
                                    <Input
                                        label='Max RPM'
                                        size='sm'
                                        variant='bordered'
                                        type='number'
                                        min={1}
                                        max={600}
                                        value={String(rpmLimit)}
                                        onValueChange={(v) => setRpmLimit(Math.max(1, Math.min(600, parseInt(v, 10) || 10)))}
                                        className='max-w-[100px]'
                                    />
                                </Tooltip>
                                <Button color='primary' className='h-[40px] px-6' onPress={() => handleTranscribe({ chain: true })} startContent={<MdTranslate />}>
                                    Transcribe &amp; Translate
                                </Button>
                                <Button variant='flat' className='h-[40px] px-4' onPress={() => handleTranscribe()} startContent={<MdGraphicEq />}>
                                    Transcribe only
                                </Button>
                            </div>
                        )}
                    </div>
                )}

                {/* Live caption (system-audio loopback) — separate from file transcription */}
                {status === 'idle' && cues.length === 0 && (
                    <Card className='border border-default-200 flex-shrink-0'>
                        <CardBody className='flex flex-col gap-3 p-4'>
                            <div className='flex items-center gap-2 flex-wrap'>
                                <MdHearing className='text-primary text-[22px]' />
                                <span className='font-semibold'>{t('config.subtitle.live.title')}</span>
                                {liveActive && (
                                    <span className='flex items-center gap-1 text-xs text-success ml-1'>
                                        <span className='w-2 h-2 rounded-full bg-success animate-pulse' />
                                        {liveStatusLabel()}
                                    </span>
                                )}
                            </div>
                            <p className='text-xs text-default-400'>{t('config.subtitle.live.desc')}</p>
                            <div className='flex flex-wrap gap-3 items-end'>
                                <Select
                                    label={t('config.subtitle.live.target')}
                                    size='sm'
                                    variant='bordered'
                                    selectedKeys={[targetLang]}
                                    onSelectionChange={(keys) => setTargetLang([...keys][0])}
                                    isDisabled={liveActive}
                                    className='max-w-[150px]'
                                >
                                    {LANG_OPTIONS.slice(1).map((l) => (
                                        <SelectItem key={l.value} value={l.value}>
                                            {l.label}
                                        </SelectItem>
                                    ))}
                                </Select>
                                <Select
                                    label={t('config.subtitle.live.engine')}
                                    size='sm'
                                    variant='bordered'
                                    selectedKeys={selectedEngine ? [selectedEngine] : []}
                                    onSelectionChange={(keys) => setSelectedEngine([...keys][0])}
                                    isDisabled={liveActive}
                                    className='max-w-[190px]'
                                >
                                    {translateServiceInstanceList.map((e) => (
                                        <SelectItem key={e} value={e}>
                                            {e}
                                        </SelectItem>
                                    ))}
                                </Select>
                                <Tooltip content={t('config.subtitle.live.chunkDesc')}>
                                    <Input
                                        label={t('config.subtitle.live.chunk')}
                                        size='sm'
                                        variant='bordered'
                                        type='number'
                                        min={2}
                                        max={15}
                                        value={String(liveChunkSeconds)}
                                        onValueChange={(v) => setLiveChunkSeconds(Math.max(2, Math.min(15, parseInt(v, 10) || 5)))}
                                        isDisabled={liveActive}
                                        className='max-w-[90px]'
                                    />
                                </Tooltip>
                                {!liveActive ? (
                                    <Button color='primary' className='h-[40px]' startContent={<MdHearing />} onPress={handleStartLive}>
                                        {t('config.subtitle.live.start')}
                                    </Button>
                                ) : (
                                    <>
                                        <Button color='danger' variant='flat' className='h-[40px]' startContent={<MdStop />} onPress={handleStopLive}>
                                            {t('config.subtitle.live.stop')}
                                        </Button>
                                        <Tooltip content={t('config.subtitle.live.clickThroughHint')}>
                                            <div>
                                                <Switch size='sm' isSelected={liveClickThrough} onValueChange={handleToggleLivePassthrough}>
                                                    <span className='text-xs'>{t('config.subtitle.live.clickThrough')}</span>
                                                </Switch>
                                            </div>
                                        </Tooltip>
                                    </>
                                )}
                            </div>
                            {liveActive && liveMessage && <span className='text-xs text-default-500'>{liveMessage}</span>}
                        </CardBody>
                    </Card>
                )}

                {/* Progress (before any cues arrive) */}
                {busy && cues.length === 0 && (
                    <div className='flex-1 flex flex-col justify-center items-center p-8 gap-4'>
                        <Progress size='md' value={progressPercent} color='primary' className='max-w-md' showValueLabel={true} />
                        <p className='text-sm font-medium animate-pulse text-default-600'>{progressText}</p>
                        <Button size='sm' color='danger' variant='flat' onPress={handleCancel} startContent={<MdCancel />}>
                            Cancel
                        </Button>
                    </div>
                )}

                {/* Cue viewer */}
                {cues.length > 0 && (
                    <div className='flex-1 flex flex-col overflow-hidden gap-3'>
                        <div className='flex justify-between items-center p-2 rounded-lg bg-content1 border border-default-100 shadow-sm flex-wrap gap-2'>
                            <div className='flex gap-2 items-center'>
                                <Button size='sm' variant='light' startContent={<MdArrowBack />} onPress={handleReset}>
                                    Load New File
                                </Button>
                                {!busy && (
                                    <>
                                        <Tooltip content='Engine used to translate the transcribed cues. Gemini routes through the fast batched pipeline; other engines translate per-cue.'>
                                            <Select
                                                aria-label='Translation engine'
                                                size='sm'
                                                variant='bordered'
                                                selectedKeys={translateEngine ? [translateEngine] : []}
                                                onSelectionChange={(keys) => setTranslateEngine([...keys][0])}
                                                className='max-w-[170px] min-w-[120px]'
                                            >
                                                {translateServiceInstanceList.map((e) => (
                                                    <SelectItem key={e} value={e}>
                                                        {e}
                                                    </SelectItem>
                                                ))}
                                            </Select>
                                        </Tooltip>
                                        <Button size='sm' color='primary' variant='flat' startContent={<MdTranslate />} onPress={() => handleTranslate()}>
                                            Translate Subtitles
                                        </Button>
                                    </>
                                )}
                                {busy && (
                                    <div className='flex items-center gap-2 px-2 border-l border-default-200'>
                                        <Progress size='sm' value={progressPercent} className='w-[100px]' color='primary' />
                                        <span className='text-xs text-default-500 animate-pulse truncate max-w-[260px]'>{progressText}</span>
                                        <Button isIconOnly size='sm' color='danger' variant='light' onPress={handleCancel}>
                                            <MdCancel />
                                        </Button>
                                    </div>
                                )}
                                {cacheHits > 0 && <span className='text-xs text-default-400'>{cacheHits} cache hits</span>}
                            </div>
                            <div className='flex gap-2 items-center'>
                                {!hasTranslation && !busy && (
                                    <span className='text-xs text-warning max-w-[260px] leading-tight'>
                                        Not translated yet — export will be original-only. Run "Translate Subtitles" first.
                                    </span>
                                )}
                                <Dropdown>
                                    <DropdownTrigger>
                                        <Button size='sm' color='primary' variant='flat' isDisabled={cues.length === 0} startContent={<MdFileDownload />}>
                                            Export SRT
                                        </Button>
                                    </DropdownTrigger>
                                    <DropdownMenu aria-label='SRT export mode' onAction={(key) => handleExport(`srt:${key}`)}>
                                        <DropdownItem key='original' description='Transcribed text only'>
                                            Original
                                        </DropdownItem>
                                        <DropdownItem key='translated' description='Translated text only' isDisabled={!hasTranslation}>
                                            Translated
                                        </DropdownItem>
                                        <DropdownItem key='bilingual' description='Original + translation per cue' isDisabled={!hasTranslation}>
                                            Bilingual
                                        </DropdownItem>
                                    </DropdownMenu>
                                </Dropdown>
                                <Dropdown>
                                    <DropdownTrigger>
                                        <Button size='sm' variant='flat' isDisabled={cues.length === 0} startContent={<MdFileDownload />}>
                                            Export VTT
                                        </Button>
                                    </DropdownTrigger>
                                    <DropdownMenu aria-label='VTT export mode' onAction={(key) => handleExport(`vtt:${key}`)}>
                                        <DropdownItem key='original' description='Transcribed text only'>
                                            Original
                                        </DropdownItem>
                                        <DropdownItem key='translated' description='Translated text only' isDisabled={!hasTranslation}>
                                            Translated
                                        </DropdownItem>
                                        <DropdownItem key='bilingual' description='Original + translation per cue' isDisabled={!hasTranslation}>
                                            Bilingual
                                        </DropdownItem>
                                    </DropdownMenu>
                                </Dropdown>
                            </div>
                        </div>

                        <div className='flex-1 overflow-y-auto pr-1 flex flex-col gap-2'>
                            {cues.map((cue, idx) => (
                                <Card key={idx} shadow='sm' className='border border-default-100'>
                                    <CardHeader className='bg-content2/50 py-1 px-3 text-xs font-mono text-primary justify-between'>
                                        <span>#{idx + 1}</span>
                                        <span>{clockOf(cue.start)} → {clockOf(cue.end)}</span>
                                    </CardHeader>
                                    <CardBody className='grid grid-cols-1 md:grid-cols-2 gap-3 p-3'>
                                        <div className='text-sm leading-relaxed select-text font-medium border-r border-default-100 pr-2'>
                                            {cue.original}
                                        </div>
                                        <div className='text-sm leading-relaxed select-text text-default-600'>
                                            {cue.translating && <span className='text-xs text-primary animate-pulse font-medium'>Translating…</span>}
                                            {cue.error && <span className='text-xs text-danger font-medium'>{cue.error}</span>}
                                            {!cue.translating && !cue.error && (cue.translated || <span className='text-xs text-default-300'>—</span>)}
                                        </div>
                                    </CardBody>
                                </Card>
                            ))}
                        </div>
                    </div>
                )}
            </div>
        </div>
    );
}

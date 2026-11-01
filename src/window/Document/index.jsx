import { Modal, ModalContent, ModalHeader, ModalBody, ModalFooter, useDisclosure, Progress, Card, CardBody, CardHeader } from '@nextui-org/react';
import { Button, Dropdown, DropdownTrigger, DropdownMenu, DropdownItem, Input, Select, SelectItem, Switch, Textarea, Tooltip } from '@nextui-org/react';
import React, { useState, useEffect, useRef } from 'react';
import { save, open } from '@tauri-apps/api/dialog';
import { writeTextFile } from '@tauri-apps/api/fs';
import { invoke } from '@tauri-apps/api/tauri';
import toast, { Toaster } from 'react-hot-toast';
import { useTranslation } from 'react-i18next';
import { MdInsertDriveFile, MdTranslate, MdCancel, MdFileDownload, MdArrowBack } from 'react-icons/md';
import { listen } from '@tauri-apps/api/event';
import { appWindow } from '@tauri-apps/api/window';


import { useToastStyle, useConfig } from '../../hooks';
import { store } from '../../utils/store';
import * as builtinServices from '../../services/translate';
import { fetchActiveGlossary, applyGlossaryPostTranslate, BUILTIN_LLM_ENGINES } from '../../utils/glossary';
import { getServiceName, whetherPluginService } from '../../utils/service_instance';
import { shouldPassThrough } from '../../utils/passthrough';
import { normalizeTocTranslation, translateShortCaption } from '../../utils/pdfParagraphs';
import { getEngineModelId, getCachedTranslation, setCachedTranslation, RpmLimiter, sha256Hex } from '../../utils/translationCache';
import { DEFAULT_IMAGE_SERVICE_URL, normalizeServiceUrl, serviceLangName } from '../../utils/imageService';

// Numbered-marker join/parse for robust multi-paragraph batching. Markers like
// @@1@@ survive translation far better than a word marker and let us map results
// BY NUMBER — a single dropped / merged / translated marker (or a truncated tail)
// only affects that one segment instead of cascading mismatch through the whole
// batch the way positional [PARAGRAPH] splitting did.
function buildNumberedJoin(texts) {
    return texts.map((t, i) => `@@${i + 1}@@\n${t}`).join('\n\n');
}

function parseNumberedSegments(translatedText, count) {
    const re = /@@\s*(\d+)\s*@@/g;
    const matches = [];
    let m;
    while ((m = re.exec(translatedText)) !== null) {
        matches.push({ num: parseInt(m[1], 10), start: m.index, end: re.lastIndex });
    }
    const result = new Array(count).fill(null);
    for (let k = 0; k < matches.length; k++) {
        const num = matches[k].num;
        if (num < 1 || num > count) continue;
        const segStart = matches[k].end;
        const segEnd = k + 1 < matches.length ? matches[k + 1].start : translatedText.length;
        const seg = translatedText.slice(segStart, segEnd).trim();
        if (seg.length > 0) result[num - 1] = seg;
    }
    return result;
}

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

export default function Document() {
    const { t } = useTranslation();
    const toastStyle = useToastStyle();

    useEffect(() => {
        if (appWindow.label === 'document') {
            appWindow.show();
        }
    }, []);


    // Config states from App
    const [translateServiceInstanceList] = useConfig('translate_service_list', ['google']);
    const [translateSecondLanguage] = useConfig('translate_second_language', 'en');

    // UI States
    const [filePath, setFilePath] = useState('');
    const [fileType, setFileType] = useState(''); // 'pdf' | 'epub'
    const [status, setStatus] = useState('idle'); // idle | loading_lib | extracting | translating | done
    const [progressPercent, setProgressPercent] = useState(0);
    const [progressText, setProgressText] = useState('');
    const [pages, setPages] = useState([]); // Array of { index: number, paragraphs: Array<{ original: string, translated: string, translating: boolean, error?: string }> }
    const [sourceLang, setSourceLang] = useState('auto');
    const [targetLang, setTargetLang] = useState('tr');
    const [selectedEngine, setSelectedEngine] = useState('');
    const [translationMode, setTranslationMode] = useState('page'); // 'page' | 'paragraph'
    const [charsPerRequest, setCharsPerRequest] = useState(6000); // source-char budget per API call (safe default for weaker models like flash-lite; raise it on flash)
    const [rpmLimit, setRpmLimit] = useState(10); // requests/min ceiling (Gemini free-tier safe default)
    const [cacheHits, setCacheHits] = useState(0);
    const [layoutExporting, setLayoutExporting] = useState(false);
    const layoutCancelRef = useRef(false);
    // Vision is the default; saved service preferences remain selectable.
    const [translateImages, setTranslateImages] = useState(true);
    const [imageMethod, setImageMethod] = useConfig('document_image_method', 'gemini');
    const [imageServiceUrl, setImageServiceUrl] = useConfig('image_translator_service_url', DEFAULT_IMAGE_SERVICE_URL);

    const cancelRef = useRef(false);

    // Initial Engine Setup
    useEffect(() => {
        if (translateServiceInstanceList && translateServiceInstanceList.length > 0 && !selectedEngine) {
            setSelectedEngine(translateServiceInstanceList[0]);
        }
    }, [translateServiceInstanceList, selectedEngine]);

    // Handle File Drop via Tauri listen
    useEffect(() => {
        const unlistenDrop = listen('tauri://file-drop', (event) => {
            const files = event.payload;
            if (Array.isArray(files) && files.length > 0) {
                const first = files[0];
                const lower = first.toLowerCase();
                if (lower.endsWith('.pdf') || lower.endsWith('.epub')) {
                    const type = lower.endsWith('.epub') ? 'epub' : 'pdf';
                    setFilePath(first);
                    setFileType(type);
                    toast.success(type === 'epub' ? 'EPUB File Loaded' : 'PDF File Loaded');
                } else {
                    toast.error('Only PDF and EPUB files are supported');
                }
            }
        });
        return () => {
            unlistenDrop.then((f) => f());
        };
    }, []);

    // File Selector
    const handleSelectFile = async () => {
        try {
            const selected = await open({
                filters: [{ name: 'Documents', extensions: ['pdf', 'epub'] }],
                multiple: false,
            });
            if (selected) {
                const type = String(selected).toLowerCase().endsWith('.epub') ? 'epub' : 'pdf';
                setFilePath(selected);
                setFileType(type);
                toast.success(type === 'epub' ? 'EPUB File Loaded' : 'PDF File Loaded');
            }
        } catch (e) {
            toast.error(String(e), { style: toastStyle });
        }
    };

    // Main translation workflow
    const handleStartTranslation = async () => {
        if (!filePath) {
            toast.error('Please load a PDF or EPUB file first');
            return;
        }

        cancelRef.current = false;
        const isEpub = fileType === 'epub';
        setStatus(isEpub ? 'extracting' : 'loading_lib');
        setProgressPercent(10);
        setProgressText(isEpub ? 'Reading EPUB structure...' : 'Verifying and initializing PDFium dynamic library...');

        try {
            const instanceConfig = (await store.get(selectedEngine)) ?? {};
            // Step 1: Extract text — per page for PDF, per spine chapter for EPUB.
            // Both commands return an array of strings, so the rest of the pipeline
            // (split into paragraphs -> dedup -> cache -> batch -> translate) is shared.
            setStatus('extracting');
            setProgressPercent(25);
            setProgressText(isEpub ? 'Extracting EPUB chapters...' : 'Loading PDF and extracting text layers...');

            const extractedPages = isEpub
                ? await invoke('extract_epub', { path: filePath })
                : await invoke('extract_pdf_layout', { path: filePath });
            if (!Array.isArray(extractedPages) || extractedPages.length === 0) {
                throw new Error(isEpub ? 'No readable chapters found in EPUB.' : 'No text layers extracted or PDF is empty.');
            }

            // Map into structured page-paragraph state
            const mappedPages = extractedPages.map((text, idx) => {
                const rawParas = isEpub
                    ? text.split(/\n\s*\n/).map((p) => p.trim()).filter((p) => p.length > 0)
                    : text.blocks;
                const paragraphs = rawParas.map((p) => ({
                    original: isEpub ? p : p.translationText,
                    sourceText: isEpub ? p : p.text,
                    blockId: isEpub ? null : p.blockId,
                    protected: !isEpub && p.protected,
                    translated: '',
                    translating: false,
                }));
                return {
                    index: idx + 1,
                    paragraphs,
                };
            });

            setPages(mappedPages);
            setStatus('translating');
            setProgressPercent(40);
            setProgressText(`Starting translation queue...`);

            // Step 2: Translate paragraphs queue
            const engineName = getServiceName(selectedEngine);
            const isPlugin = whetherPluginService(selectedEngine);
            if (isPlugin) {
                throw new Error('.potext plugins are currently not supported for document translation.');
            }

            const LanguageEnum = builtinServices[engineName]?.Language;
            if (!LanguageEnum) {
                throw new Error(`Engine ${engineName} does not expose compatible languages.`);
            }

            // Get glossary entries
            const _glossarySrc = sourceLang === 'auto' ? (mappedPages.some((p) => p.paragraphs.some((b) => /[\u3400-\u9fff]/u.test(b.original))) ? 'zh_cn' : 'en') : sourceLang;
            const _glossaryEntries = await fetchActiveGlossary(_glossarySrc, targetLang, 'document');

            let totalParagraphs = mappedPages.reduce((acc, p) => acc + p.paragraphs.length, 0);
            let translatedCount = 0;

            // Phase 2 optimization: stable model id for cache keys + sliding-window
            // RPM limiter + per-run cache-hit counter for UI feedback. The blind 150ms
            // proactive delay used by the previous implementation was both too fast for
            // Gemini's free-tier RPM limit and oblivious to longer-window enforcement;
            // RpmLimiter replaces it with an explicit sliding window.
            const glossaryRevision = await sha256Hex(JSON.stringify(_glossaryEntries.map((g) => [g.source_term, g.target_term, g.case_sensitive])));
            const engineModelId = engineName === 'geminipro' ? `gemini:${builtinServices.geminipro.resolveModelAndBase(instanceConfig).model}` : getEngineModelId(engineName, instanceConfig);
            const modelId = engineModelId + (isEpub ? '' : `:canonical-v1:${glossaryRevision}`);
            const rpmLimiter = new RpmLimiter(Math.max(1, rpmLimit | 0));
            let localCacheHits = 0;
            let rateLimited = false;
            setCacheHits(0);

            const translateParagraphWithRetry = async (text, pageIdx, paraIdx) => {
                const maxRetries = 3;
                let attempt = 0;
                let delay = 1000;

                while (attempt <= maxRetries) {
                    if (cancelRef.current) {
                        throw new Error('cancelled');
                    }

                    try {
                        if (engineName === 'geminipro') {
                            const [result] = await builtinServices.geminipro.translateBatchStructured([text], LanguageEnum[sourceLang], LanguageEnum[targetLang], {
                                config: instanceConfig, glossaryEntries: _glossaryEntries, beforeRetry: () => rpmLimiter.acquire(),
                            });
                            if (!result) {
                                const error = new Error('Translation failed validation after single-item retry.');
                                error.code = 'TRANSLATION_VALIDATION';
                                throw error;
                            }
                            return result;
                        }
                        let finalResult = '';
                        const resText = await new Promise((resolve, reject) => {
                            builtinServices[engineName].translate(
                                text,
                                LanguageEnum[sourceLang],
                                LanguageEnum[targetLang],
                                {
                                    config: instanceConfig,
                                    detect: sourceLang === 'auto' ? 'en' : sourceLang,
                                    glossaryEntries: _glossaryEntries,
                                    setResult: (v) => {
                                        finalResult = v;
                                    }
                                }
                            ).then((res) => {
                                let val = res || finalResult;
                                if (typeof val === 'string' && !BUILTIN_LLM_ENGINES.includes(engineName)) {
                                    val = applyGlossaryPostTranslate(val, _glossaryEntries);
                                }
                                resolve(val);
                            }).catch(reject);
                        });

                        return resText;
                    } catch (err) {
                        if (err.code === 'TRANSLATION_VALIDATION' || /429|RESOURCE_EXHAUSTED|quota/i.test(String(err))) throw err;
                        attempt++;
                        if (attempt > maxRetries || cancelRef.current) {
                            throw err;
                        }
                        
                        // Update progress UI status to let the user know we are retrying
                        if (paraIdx === -1) {
                            setProgressText(`Retrying Page ${pageIdx + 1}... Attempt ${attempt}/${maxRetries} in ${(delay / 1000).toFixed(1)}s`);
                        } else {
                            setProgressText(`Retrying Page ${pageIdx + 1} para ${paraIdx + 1}... Attempt ${attempt}/${maxRetries} in ${(delay / 1000).toFixed(1)}s`);
                        }
                        
                        // Wait with exponential backoff before the next attempt
                        await new Promise(resolve => setTimeout(resolve, delay));
                        delay *= 2; // double the delay
                    }
                }
            };

            // Cache-first wrapper around the retry helper. Same input across re-runs
            // returns the cached output with NO API call. After a successful
            // translation, the result is stored so subsequent retries of this document
            // (or any document containing the same chunk) are free.
            const translateWithCacheAndRetry = async (text, pageIdx, paraIdx) => {
                if (typeof text !== 'string' || text.trim().length === 0) return '';
                const cached = await getCachedTranslation(modelId, sourceLang, targetLang, text);
                if (cached !== null && cached !== undefined) {
                    localCacheHits++;
                    setCacheHits(localCacheHits);
                    return cached;
                }
                // Pace under RPM ceiling before the live call.
                await rpmLimiter.acquire();
                const result = await translateParagraphWithRetry(text, pageIdx, paraIdx);
                if (typeof result === 'string' && result.length > 0) {
                    // fire-and-forget — never block translation on cache write failure
                    setCachedTranslation(modelId, sourceLang, targetLang, text, result);
                }
                return result;
            };

            // Gemini supports schema-constrained JSON output (HANDOFF §8 #1). When the
            // selected engine is Gemini we translate each batch as a structured
            // { id, translation } array — no fragile @@n@@ markers, so no marker drift
            // and no source-echo on weak/free-tier models. Every other engine keeps the
            // numbered-marker path. This is decided once per run.
            // The Gemini document path now runs in a Rust background thread (Phase 2 §7).
            // `resolveModelAndBase` is exported by the geminipro service so we can hand the
            // Rust loop the same { model, base } the in-renderer path would have used.
            const supportsStructured =
                engineName === 'geminipro' &&
                typeof builtinServices.geminipro?.resolveModelAndBase === 'function';

            if (translationMode === 'page') {
                const charBudget = Math.max(1000, charsPerRequest | 0);

                // Flatten every paragraph into a unit that remembers its location.
                const units = []; // { pageIdx, paraIdx, text }
                for (let pi = 0; pi < mappedPages.length; pi++) {
                    const pg = mappedPages[pi];
                    for (let pj = 0; pj < pg.paragraphs.length; pj++) {
                        units.push({ pageIdx: pi, paraIdx: pj, text: pg.paragraphs[pj].original, protected: pg.paragraphs[pj].protected });
                    }
                }

                // DEDUP — manuals repeat the same header / footer / page-number on
                // every page. Translate each UNIQUE source paragraph ONCE, then fan the
                // result out to all its locations. translationByText maps sourceText →
                // result; sentinels: null = pending, string = done, '__MISMATCH__' and
                // '__ERROR__:<msg>' flag failures.
                const translationByText = new Map();
                const uniqueTexts = [];
                for (const u of units) {
                    if (u.text.trim().length === 0) {
                        translationByText.set(u.text, '');
                        continue;
                    }
                    // Formula / code / numeric block → keep verbatim, never sent to the API.
                    if (u.protected || shouldPassThrough(u.text)) {
                        translationByText.set(u.text, u.text);
                        continue;
                    }
                    const caption = translateShortCaption(u.text, targetLang);
                    if (caption) { translationByText.set(u.text, caption); continue; }
                    if (!translationByText.has(u.text)) {
                        translationByText.set(u.text, null);
                        uniqueTexts.push(u.text);
                    }
                }

                // Mark everything translating up front.
                setPages(prev => {
                    const copy = [...prev];
                    for (const u of units) {
                        if (u.text.trim().length > 0) copy[u.pageIdx].paragraphs[u.paraIdx].translating = true;
                    }
                    return copy;
                });

                // CACHE — resolve unique texts already known; queue the rest.
                const needTranslate = [];
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

                if (supportsStructured) {
                    // GEMINI PATH — hand the long-running batch/RPM/retry/parse loop to a
                    // Rust background thread (Phase 2 §7 / HANDOFF §7). The renderer no
                    // longer blocks on network waits + RPM sleeps for very long docs;
                    // results stream back via `document://*` events. Extraction, dedup,
                    // pass-through and the cache stay frontend/SQLite-owned (applied above),
                    // so the cache pre-check already ran and `needTranslate` is the remainder.
                    const { model, base } = builtinServices.geminipro.resolveModelAndBase(instanceConfig);
                    const runId = Date.now();
                    const glossaryPayload = (_glossaryEntries || []).map((e) => ({
                        sourceTerm: e.source_term,
                        targetTerm: e.target_term,
                    }));

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
                                // per-paragraph cache → cross-document + retry reuse
                                setCachedTranslation(modelId, sourceLang, targetLang, r.source, r.translation);
                            } else {
                                translationByText.set(r.source, r.error ? '__ERROR__:' + r.error : '__MISMATCH__');
                            }
                            doneUnique++;
                        }
                        const percent = Math.min(40 + Math.floor((doneUnique / Math.max(1, uniqueTexts.length)) * 60), 100);
                        setProgressPercent(percent);
                    };
                    const onDone = (event) => {
                        const p = event.payload;
                        if (!p || p.runId !== runId) return;
                        resolveDone(p);
                    };

                    // Register listeners BEFORE invoking so an instant `done` can't race.
                    const unlistenProgress = await listen('document://progress', onProgress);
                    const unlistenBatch = await listen('document://batch', onBatch);
                    const unlistenDone = await listen('document://done', onDone);

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

                    const outcome = await donePromise;
                    unlistenProgress();
                    unlistenBatch();
                    unlistenDone();

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
                    // NON-GEMINI PATH — numbered-marker batching, run in-renderer.
                    // BATCH by CHARACTER BUDGET (not page count) — pack each request as
                    // full as is safe so the document costs the fewest possible API calls.
                    // Also cap by a hard paragraph count: many short units (e.g. a
                    // table-of-contents page) would otherwise stuff dozens of markers into
                    // one call and raise alignment-drift risk.
                    const MAX_PARAS_PER_BATCH = 25;
                    const batches = [];
                    let cur = [];
                    let curChars = 0;
                    for (const text of needTranslate) {
                        if (cur.length > 0 && (curChars + text.length > charBudget || cur.length >= MAX_PARAS_PER_BATCH)) {
                            batches.push(cur);
                            cur = [];
                            curChars = 0;
                        }
                        cur.push(text);
                        curChars += text.length;
                    }
                    if (cur.length > 0) batches.push(cur);

                    for (let bIdx = 0; bIdx < batches.length; bIdx++) {
                        if (cancelRef.current) break;
                        const batchTexts = batches[bIdx];
                        const expectedCount = batchTexts.length;
                        setProgressText(`Batch ${bIdx + 1}/${batches.length} · ${doneUnique}/${uniqueTexts.length} unique paragraphs · ${localCacheHits} cache hits`);

                        try {
                            // Per-paragraph caching is done below, so skip the batch-level
                            // cache wrapper — just pace under the RPM ceiling and retry.
                            await rpmLimiter.acquire();

                            const joinedText = buildNumberedJoin(batchTexts);
                            const translatedText = await translateParagraphWithRetry(joinedText, 0, -1);

                            // Primary: map segments BY NUMBER (robust to dropped/merged
                            // markers and to a truncated output tail).
                            let results = parseNumberedSegments(translatedText, expectedCount);
                            const gotCount = results.filter((r) => r !== null).length;

                            // Fallback only if the numbered markers didn't survive at all.
                            if (gotCount === 0) {
                                let flat = translatedText.split(/\s*\[PARAGRAPH\]\s*/i).map((p) => p.trim()).filter((p) => p.length > 0);
                                if (flat.length !== expectedCount) {
                                    const nl = translatedText.split(/\n\s*\n/).map((p) => p.trim()).filter((p) => p.length > 0);
                                    if (nl.length === expectedCount) flat = nl;
                                }
                                if (flat.length === expectedCount) {
                                    results = flat;
                                } else if (expectedCount === 1) {
                                    results = [translatedText.trim()];
                                }
                            }

                            for (let i = 0; i < expectedCount; i++) {
                                const srcText = batchTexts[i];
                                const trans = results[i];
                                if (trans !== null && trans !== undefined && trans.length > 0) {
                                    translationByText.set(srcText, trans);
                                    // per-paragraph cache → cross-document + retry reuse
                                    setCachedTranslation(modelId, sourceLang, targetLang, srcText, trans);
                                } else {
                                    // Only THIS paragraph failed (dropped marker / truncated
                                    // tail). A re-run retries just this one — the rest are cached.
                                    translationByText.set(srcText, '__MISMATCH__');
                                }
                            }
                        } catch (err) {
                            const errStr = String(err);
                            for (const srcText of batchTexts) {
                                translationByText.set(srcText, '__ERROR__:' + errStr);
                            }
                            // A 429 means the provider's rate limit / daily quota is hit —
                            // hammering the remaining batches just piles up identical
                            // failures and burns more quota. Stop now; paragraphs done so
                            // far are cached, so a re-run after the limit resets continues
                            // exactly where we left off.
                            if (errStr.includes('429') || /too many requests|rate limit|quota|resource exhausted/i.test(errStr)) {
                                rateLimited = true;
                                break;
                            }
                        }

                        doneUnique += expectedCount;
                        const percent = Math.min(40 + Math.floor((doneUnique / Math.max(1, uniqueTexts.length)) * 60), 100);
                        setProgressPercent(percent);
                    }
                }

                // FAN OUT — apply each resolved translation to every location (dups included).
                setPages(prev => {
                    const copy = [...prev];
                    for (const u of units) {
                        const para = copy[u.pageIdx].paragraphs[u.paraIdx];
                        para.translating = false;
                        const t = translationByText.get(u.text);
                        if (t === null || t === undefined) {
                            if (!cancelRef.current) para.error = 'Not translated';
                        } else if (typeof t === 'string' && t.startsWith('__ERROR__:')) {
                            para.error = t.slice('__ERROR__:'.length);
                        } else if (t === '__MISMATCH__') {
                            para.error = 'Paragraph mismatch during batch translation layout mapping.';
                        } else {
                            // Pass-through results (t === source) stay verbatim; real
                            // translations of TOC lines get their dot leader + page
                            // number restored if the model mangled them.
                            para.translated = t === u.text ? t : normalizeTocTranslation(u.text, t);
                            para.error = undefined;
                        }
                    }
                    return copy;
                });
                translatedCount = totalParagraphs;
                setProgressPercent(100);
            } else {
                for (let pageIdx = 0; pageIdx < mappedPages.length; pageIdx++) {
                    if (cancelRef.current) break;

                    const page = mappedPages[pageIdx];
                    for (let paraIdx = 0; paraIdx < page.paragraphs.length; paraIdx++) {
                        if (cancelRef.current) break;

                        setProgressText(`Translating Page ${page.index} / ${mappedPages.length}...`);

                        // Update translating state
                        setPages(prev => {
                            const copy = [...prev];
                            copy[pageIdx].paragraphs[paraIdx].translating = true;
                            return copy;
                        });

                        const paragraphText = page.paragraphs[paraIdx].original;

                        // Formula / code / numeric block → keep verbatim, skip the API.
                        const caption = translateShortCaption(paragraphText, targetLang);
                        if (caption || page.paragraphs[paraIdx].protected || shouldPassThrough(paragraphText)) {
                            setPages(prev => {
                                const copy = [...prev];
                                copy[pageIdx].paragraphs[paraIdx].translated = caption || paragraphText;
                                copy[pageIdx].paragraphs[paraIdx].translating = false;
                                copy[pageIdx].paragraphs[paraIdx].error = undefined;
                                return copy;
                            });
                            translatedCount++;
                            setProgressPercent(Math.min(40 + Math.floor((translatedCount / totalParagraphs) * 60), 100));
                            continue;
                        }

                        try {
                            const translatedText = await translateWithCacheAndRetry(paragraphText, pageIdx, paraIdx);
                            setPages(prev => {
                                const copy = [...prev];
                                copy[pageIdx].paragraphs[paraIdx].translated = normalizeTocTranslation(paragraphText, String(translatedText));
                                copy[pageIdx].paragraphs[paraIdx].translating = false;
                                copy[pageIdx].paragraphs[paraIdx].error = undefined; // Clear error on success
                                return copy;
                            });
                        } catch (err) {
                            if (String(err) === 'Error: cancelled') {
                                break;
                            }
                            setPages(prev => {
                                const copy = [...prev];
                                copy[pageIdx].paragraphs[paraIdx].error = String(err);
                                copy[pageIdx].paragraphs[paraIdx].translating = false;
                                return copy;
                            });
                        }

                        translatedCount++;
                        const percent = Math.min(40 + Math.floor((translatedCount / totalParagraphs) * 60), 100);
                        setProgressPercent(percent);
                    }
                }
            }

            if (cancelRef.current) {
                setStatus('idle');
                toast.error('Translation cancelled.');
            } else if (rateLimited) {
                setStatus('done');
                setProgressText('Stopped — provider rate limit / daily quota reached.');
                toast.error(
                    'Stopped: the provider returned 429 (rate limit / daily quota). Paragraphs translated so far are cached — re-run this document after the limit resets and it will continue from where it left off (cached paragraphs cost no quota).',
                    { style: toastStyle, duration: 12000 }
                );
            } else {
                setStatus('done');
                setProgressPercent(100);
                setProgressText('Bilingual Translation Completed!');
                toast.success('Document translation successfully completed!');
            }

        } catch (e) {
            setStatus('idle');
            toast.error(String(e), { style: toastStyle });
        }
    };

    // Cancel translation. Also signals the Rust background loop (Gemini path); the
    // invoke is harmless / no-op for the in-renderer engines.
    const handleCancel = () => {
        cancelRef.current = true;
        invoke('cancel_document_translation').catch(() => {});
    };

    // Reset View
    const handleReset = () => {
        setFilePath('');
        setFileType('');
        setPages([]);
        setStatus('idle');
        setProgressPercent(0);
        setProgressText('');
    };

    // Export Bilingual Markdown
    const handleExportMarkdown = async () => {
        try {
            const docKind = fileType === 'epub' ? 'EPUB' : 'PDF';
            const sectionLabel = fileType === 'epub' ? 'Section' : 'Page';
            let markdown = `# Bilingual ${docKind} Translation: ${filePath.split(/[\\/]/).pop()}\n\n`;
            for (const page of pages) {
                markdown += `## ${sectionLabel} ${page.index}\n\n`;
                for (const para of page.paragraphs) {
                    markdown += `**Original:**\n${para.original}\n\n`;
                    let translationText = para.translated;
                    if (!translationText) {
                        translationText = para.error ? `*[Translation Error: ${para.error}]*` : '*[Not Translated]*';
                    }
                    markdown += `**Translation:**\n${translationText}\n\n`;
                    markdown += `---\n\n`;
                }
            }

            const savePath = await save({
                filters: [{ name: 'Markdown', extensions: ['md'] }],
                defaultPath: 'translated_document.md',
            });
            if (!savePath) return;

            await writeTextFile(savePath, markdown);
            toast.success('Markdown exported successfully!');
        } catch (e) {
            toast.error(String(e), { style: toastStyle });
        }
    };

    // Export Bilingual HTML
    const handleExportHTML = async () => {
        try {
            const docKind = fileType === 'epub' ? 'EPUB' : 'PDF';
            const sectionLabel = fileType === 'epub' ? 'Section' : 'Page';
            let htmlContent = `
<!DOCTYPE html>
<html>
<head>
<meta charset="utf-8">
<title>Bilingual ${docKind} Translation</title>
<style>
  body { font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif; line-height: 1.6; max-width: 1200px; margin: 0 auto; padding: 20px; background-color: #fafafa; color: #333; }
  h1 { text-align: center; border-bottom: 2px solid #eaeaea; padding-bottom: 10px; }
  .page-section { background: white; padding: 20px; border-radius: 8px; box-shadow: 0 4px 6px rgba(0,0,0,0.05); margin-bottom: 30px; }
  .page-title { font-weight: bold; font-size: 1.2em; border-bottom: 1px solid #eee; padding-bottom: 5px; margin-bottom: 15px; color: #0070f3; }
  .grid-row { display: grid; grid-template-columns: 1fr 1fr; gap: 20px; border-bottom: 1px solid #f0f0f0; padding: 15px 0; }
  .source { font-weight: 500; }
  .target { color: #555; border-left: 3px solid #0070f3; padding-left: 10px; }
</style>
</head>
<body>
  <h1>Bilingual Translation: ${filePath.split(/[\\/]/).pop()}</h1>
`;

            for (const page of pages) {
                htmlContent += `  <div class="page-section">\n    <div class="page-title">${sectionLabel} ${page.index}</div>\n`;
                for (const para of page.paragraphs) {
                    htmlContent += `    <div class="grid-row">\n`;
                    htmlContent += `      <div class="source">${para.original.replace(/\n/g, '<br>')}</div>\n`;
                    let translationText = para.translated;
                    let isError = false;
                    if (!translationText) {
                        if (para.error) {
                            translationText = `[Translation Error: ${para.error}]`;
                            isError = true;
                        } else {
                            translationText = '*[Not Translated]*';
                        }
                    }
                    if (isError) {
                        htmlContent += `      <div class="target" style="color: #ea3838; font-style: italic;">${translationText.replace(/\n/g, '<br>')}</div>\n`;
                    } else {
                        htmlContent += `      <div class="target">${translationText.replace(/\n/g, '<br>')}</div>\n`;
                    }
                    htmlContent += `    </div>\n`;
                }
                htmlContent += `  </div>\n`;
            }

            htmlContent += `</body>\n</html>`;

            const savePath = await save({
                filters: [{ name: 'HTML', extensions: ['html'] }],
                defaultPath: 'translated_document.html',
            });
            if (!savePath) return;

            await writeTextFile(savePath, htmlContent);
            toast.success('Bilingual HTML exported successfully!');
        } catch (e) {
            toast.error(String(e), { style: toastStyle });
        }
    };

    // Build a print-optimized HTML document for one of the three output modes.
    // We render to PDF via the webview's own print pipeline (window.print →
    // "Save as PDF") rather than a native Rust PDF generator, because the
    // webview renders every script (CJK source, Turkish/accented target, RTL,
    // …) using system fonts — no font embedding and no >10MB CJK font bundle.
    const buildPrintHtml = (mode, fileName) => {
        const esc = (s) => (s ?? '').replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
        const br = (s) => esc(s).replace(/\n/g, '<br>');
        const transOf = (para) => {
            if (para.translated) return { text: para.translated, isError: false };
            if (para.error) return { text: `[Translation Error: ${para.error}]`, isError: true };
            return { text: '[Not Translated]', isError: true };
        };

        const baseCss = `
          * { box-sizing: border-box; }
          @page { size: A4; margin: 14mm; }
          html, body { margin: 0; padding: 0; }
          body { font-family: "Segoe UI", -apple-system, Roboto, "Noto Sans", "Microsoft YaHei", "Noto Sans CJK SC", sans-serif; line-height: 1.5; color: #1a1a1a; font-size: 11pt; }
          h1 { font-size: 16pt; text-align: center; margin: 0 0 16px; word-break: break-word; overflow-wrap: anywhere; }
          /* Break BETWEEN pages only — never before the first or after the last.
             (page-break-after:always on every .page is what produced a spurious
             leading/trailing blank sheet in the bilingual / side-by-side modes.) */
          .page + .page { page-break-before: always; }
          .page-title { font-weight: 600; font-size: 12pt; color: #0b5cad; border-bottom: 1px solid #ccc; padding-bottom: 4px; margin: 0 0 10px; }
          .para { page-break-inside: avoid; margin-bottom: 10px; }
          .orig, .trans { word-break: break-word; overflow-wrap: anywhere; }
          .orig { color: #444; }
          .trans { color: #111; }
          .err { color: #b00020; font-style: italic; }
        `;
        let layoutCss;
        if (mode === 'sidebyside') {
            layoutCss = `.para { display: grid; grid-template-columns: 1fr 1fr; gap: 14px; border-bottom: 1px solid #eee; padding: 6px 0; }
              .trans { border-left: 2px solid #0b5cad; padding-left: 10px; }`;
        } else if (mode === 'bilingual') {
            layoutCss = `.orig { margin-bottom: 4px; }
              .trans { border-left: 2px solid #0b5cad; padding-left: 10px; margin-bottom: 8px; }
              .para { border-bottom: 1px solid #f0f0f0; padding-bottom: 6px; }`;
        } else {
            layoutCss = `.trans { margin-bottom: 8px; }`;
        }

        // H1 lives INSIDE the first .page so there is no stray block before the
        // first page box (another contributor to the leading blank sheet).
        let body = '';
        let firstPage = true;
        for (const page of pages) {
            body += `<div class="page">`;
            if (firstPage) {
                body += `<h1>${esc(fileName)}</h1>`;
                firstPage = false;
            }
            body += `<div class="page-title">Page ${page.index}</div>`;
            for (const para of page.paragraphs) {
                const t = transOf(para);
                if (mode === 'translated') {
                    body += `<div class="para"><div class="trans ${t.isError ? 'err' : ''}">${br(t.text)}</div></div>`;
                } else {
                    body += `<div class="para"><div class="orig">${br(para.original)}</div><div class="trans ${t.isError ? 'err' : ''}">${br(t.text)}</div></div>`;
                }
            }
            body += `</div>`;
        }

        return `<!DOCTYPE html><html><head><meta charset="utf-8"><title>${esc(fileName)}</title><style>${baseCss}${layoutCss}</style></head><body>${body}</body></html>`;
    };

    // Export to PDF via a hidden iframe + the webview print dialog.
    // mode: 'bilingual' | 'sidebyside' | 'translated'
    const handleExportPdf = (mode) => {
        try {
            if (pages.length === 0) return;
            const fileName = filePath.split(/[\\/]/).pop() || 'document';
            const printHtml = buildPrintHtml(mode, fileName);

            const iframe = document.createElement('iframe');
            iframe.setAttribute('aria-hidden', 'true');
            // Off-screen but with a REAL A4-ish viewport (96dpi). A 0x0 iframe makes
            // the print engine lay out at zero width, which can emit a blank sheet.
            iframe.style.position = 'fixed';
            iframe.style.left = '-10000px';
            iframe.style.top = '0';
            iframe.style.width = '794px';
            iframe.style.height = '1123px';
            iframe.style.border = '0';
            iframe.style.opacity = '0';
            document.body.appendChild(iframe);

            const cleanup = () => {
                if (iframe.parentNode) iframe.parentNode.removeChild(iframe);
            };

            const doc = iframe.contentWindow.document;
            doc.open();
            doc.write(printHtml);
            doc.close();

            iframe.contentWindow.addEventListener('afterprint', () => setTimeout(cleanup, 500));

            let triggered = false;
            const printWhenReady = () => {
                if (triggered) return;
                triggered = true;
                try {
                    iframe.contentWindow.focus();
                    iframe.contentWindow.print();
                } catch (err) {
                    cleanup();
                    toast.error('Print failed: ' + String(err), { style: toastStyle });
                    return;
                }
                // Safety net in case afterprint never fires.
                setTimeout(cleanup, 120000);
            };

            // A big document must FULLY lay out before we print, otherwise the print
            // engine captures a blank/partial page and produces a corrupt PDF that
            // "won't open". Wait for the iframe load event, and also use a fallback
            // delay scaled to the page count (doc.write doesn't always fire onload in
            // every webview). Whichever fires first wins (guarded by `triggered`).
            iframe.onload = () => setTimeout(printWhenReady, 600);
            const settleMs = Math.min(12000, 1500 + pages.length * 140);
            setTimeout(printWhenReady, settleMs);

            toast.success(`Preparing ${pages.length}-page PDF… the print dialog will open shortly — choose "Save as PDF" / "Microsoft Print to PDF" as the destination.`, { duration: 8000 });
        } catch (e) {
            toast.error(String(e), { style: toastStyle });
        }
    };

    // Native PDF export (Phase 2 #3) — assembled in Rust via printpdf. Bilingual /
    // side-by-side embed the rendered ORIGINAL page image (rendered in-Rust) next to
    // the translation as real, selectable text. Preferred over the webview-print path.
    // mode: 'bilingual' | 'sidebyside' | 'translated'
    const handleExportPdfNative = async (mode) => {
        try {
            if (pages.length === 0) return;
            const baseName = (filePath.split(/[\\/]/).pop() || 'document').replace(/\.pdf$/i, '');
            const outputPath = await save({
                filters: [{ name: 'PDF', extensions: ['pdf'] }],
                defaultPath: `${baseName}_${mode}.pdf`,
            });
            if (!outputPath) return;

            // Only the translation + error are needed Rust-side; the original side is
            // rebuilt from the source PDF image, not from extracted text.
            const payloadPages = pages.map((page) => ({
                paragraphs: page.paragraphs.map((p) => ({
                    translated: p.translated || '',
                    error: p.error || null,
                })),
            }));

            const needImages = mode === 'bilingual' || mode === 'sidebyside';
            toast.loading(
                needImages
                    ? `Building ${pages.length}-page PDF (rendering original pages, this can take a moment)…`
                    : `Building ${pages.length}-page PDF…`,
                { id: 'native-pdf', style: toastStyle }
            );

            await invoke('export_translated_pdf', {
                sourcePdfPath: filePath,
                outputPath,
                mode,
                targetLang,
                pages: payloadPages,
            });

            toast.success(`PDF saved to ${outputPath}`, { id: 'native-pdf', style: toastStyle, duration: 6000 });
        } catch (e) {
            toast.error('PDF export failed: ' + String(e), { id: 'native-pdf', style: toastStyle, duration: 10000 });
        }
    };

    // External image-translation service path — sends each embedded PDF image
    // to the layout-image-translator microservice (PaddleOCR-VL) and collects
    // the returned translated images (same pixel dimensions, layout preserved
    // WITHIN the image) as replacements for the original bitmap. Any failed
    // image aborts export so an incomplete translation cannot be published.
    const gatherImageReplacements = async () => {
        const baseUrl = normalizeServiceUrl(imageServiceUrl);
        const available = await invoke('check_image_service', { baseUrl });
        if (!available) {
            throw new Error(t('config.document.imageServiceDown', { url: baseUrl }));
        }

        const svcSource = serviceLangName(sourceLang);
        const svcTarget = serviceLangName(targetLang);
        const replacements = [];

        let unlistenScan;
        try {
            unlistenScan = await listen('image_scan://progress', (event) => {
                const p = event.payload;
                if (!p) return;
                toast.loading(p.text || `Scanning images on page ${(p.pageIndex ?? 0) + 1}/${p.pageCount || pages.length}…`, {
                    id: 'layout-pdf',
                    style: toastStyle,
                });
            });

            toast.loading('Scanning embedded PDF images…', { id: 'layout-pdf', style: toastStyle });
            const allImages = await invoke('scan_pdf_images', { path: filePath, pageCount: pages.length });
            const total = (allImages || []).length;

            let done = 0;
            let lastError = null;
            for (const img of allImages || []) {
                done++;
                if (!img.cachePath) continue;
                toast.loading(
                    t('config.document.imageServiceProgress', { done, total }),
                    { id: 'layout-pdf', style: toastStyle }
                );
                try {
                    const translatedPath = await invoke('translate_image_via_service', {
                        baseUrl,
                        cachePath: img.cachePath,
                        sourceLang: svcSource,
                        targetLang: svcTarget,
                    });
                    replacements.push({
                        pageIndex: img.pageIndex ?? 0,
                        left: img.left,
                        bottom: img.bottom,
                        right: img.right,
                        top: img.top,
                        imagePath: translatedPath,
                    });
                } catch (e) {
                    // Skip this image (original stays in place); keep going.
                    lastError = e;
                    console.warn('Image service failed for', img.cachePath, e);
                }
            }
            // Every single image failed while the service was reachable at the
            // health check — that is systemic (service died mid-run, provider
            // misconfigured, …). Fail LOUDLY instead of silently exporting a
            // PDF with untranslated images after a long wait.
            if (total > 0 && replacements.length === 0) {
                throw new Error(
                    t('config.document.imageServiceAllFailed', { total }) +
                        (lastError ? ` (${String(lastError)})` : '')
                );
            }
            if (replacements.length < total) {
                throw new Error(t('config.document.imageServicePartial', { ok: replacements.length, total }));
            }
        } finally {
            if (unlistenScan) unlistenScan();
        }
        return replacements;
    };

    // Layout PDF export uses canonical block IDs, removes original text objects,
    // and fits every translated glyph while retaining source graphics. Native
    // Vision edits image glyph pixels; the optional service replaces bitmaps.
    const handleExportLayoutPdf = async () => {
        if (layoutExporting) return;
        let unlistenProgress;
        let unlistenDone;
        let unlistenImageOcr;
        let ocrWorkerPromise;
        let ocrLanguage;
        try {
            if (pages.length === 0 || fileType !== 'pdf') return;
            const baseName = (filePath.split(/[\\/]/).pop() || 'document').replace(/\.pdf$/i, '');
            const outputPath = await save({
                filters: [{ name: 'PDF', extensions: ['pdf'] }],
                defaultPath: `${baseName}_${targetLang}_layout.pdf`,
            });
            if (!outputPath) return;
            layoutCancelRef.current = false;
            setLayoutExporting(true);

            // Rust requires the stable ID and exact source from extraction.
            const payloadPages = pages.map((page) => ({
                blockIds: page.paragraphs.map((p) => p.blockId),
                paragraphs: page.paragraphs.map((p) => ({
                    original: p.sourceText || p.original || '',
                    translated: p.translated || '',
                })),
            }));

            // The retired OCR setting maps to Vision for existing saved settings.
            const imageOverlays = [];
            let imageReplacements = [];
            let imageTranslation = null;
            if (translateImages) {
                try {
                    if ((imageMethod || 'gemini') === 'gemini' || imageMethod === 'ocr') {
                        if (getServiceName(selectedEngine) !== 'geminipro') throw new Error(t('config.document.imageGeminiRequired'));
                        const config = (await store.get(selectedEngine)) ?? {};
                        const { model, base } = builtinServices.geminipro.resolveModelAndBase(config);
                        const glossary = await fetchActiveGlossary(sourceLang === 'auto' ? 'zh_cn' : sourceLang, targetLang, 'document');
                        const terms = new Map();
                        for (const g of glossary) if (!terms.has(g.source_term)) terms.set(g.source_term,g.target_term);
                        // Keep repeated diagram terms consistent with the text
                        // already translated in this document. User terms win.
                        for (const page of pages) for (const p of page.paragraphs) {
                            const source=(p.original || '').replace(/^\d+(?:\.\d+)*\s+/, '').trim();
                            const target=(p.translated || '').replace(/^\d+(?:\.\d+)*\s+/, '').trim();
                            if (terms.size<128 && source.length>=2 && source.length<=32 && target && target.length<=96 && /[\u3400-\u9fff]/u.test(source) && !/[\u3400-\u9fff]/u.test(target) && !target.startsWith('__ERROR__') && !/^[图表]\s*\d/u.test(source) && !terms.has(source)) terms.set(source,target);
                        }
                        imageTranslation = { apiKey: config.apiKey, apiBase: base, model, sourceLang, targetLang, rpmLimit,
                            glossary: [...terms].map(([sourceTerm,targetTerm]) => ({sourceTerm,targetTerm})) };
                    } else if (imageMethod === 'service') {
                        imageReplacements = await gatherImageReplacements();
                    } else {
                        throw new Error('Unknown image translation method. Select Gemini Vision or the image translation service.');
                    }
                } catch (e) {
                    toast.error('In-image translation failed: ' + String(e), { id: 'layout-pdf', style: toastStyle, duration: 9000 });
                    return;
                }
            }

            const runId = Date.now();
            let resolveDone;
            const donePromise = new Promise((r) => { resolveDone = r; });

            const onProgress = (event) => {
                const p = event.payload;
                if (!p || p.runId !== runId) return;
                toast.loading(p.text, { id: 'layout-pdf', style: toastStyle });
            };
            const onDone = (event) => {
                const p = event.payload;
                if (!p || p.runId !== runId) return;
                resolveDone(p);
            };

            // Register listeners BEFORE invoking so an instant `done` can't race.
            unlistenProgress = await listen('layout_export://progress', onProgress);
            unlistenDone = await listen('layout_export://done', onDone);
            unlistenImageOcr = await listen('layout_export://image_ocr', async ({payload:p}) => {
                if (!p || p.runId!==runId) return;
                try {
                    if (layoutCancelRef.current) throw new Error('Export cancelled.');
                    if (!ocrWorkerPromise || ocrLanguage!==p.language) {
                        if (ocrWorkerPromise) await (await ocrWorkerPromise).terminate();
                        ocrLanguage=p.language;
                        ocrWorkerPromise=import('tesseract.js').then(({createWorker})=>createWorker(p.language,1,{
                            workerPath:'/worker.min.js',corePath:'/tesseract-core-simd-lstm.wasm.js',
                            langPath:'https://pub-f6afb74f13c64cd89561b4714dca1c27.r2.dev',
                        }));
                    }
                    const worker=await ocrWorkerPromise;
                    await worker.setParameters({tessedit_pageseg_mode:String(p.psm),preserve_interword_spaces:'1'});
                    const pixels=Uint8Array.from(atob(p.imageBase64),c=>c.charCodeAt(0));
                    const {data}=await worker.recognize(pixels);
                    const lines=[...(data.lines||[])];
                    if (p.normalizedBase64) {
                        const normalized=Uint8Array.from(atob(p.normalizedBase64),c=>c.charCodeAt(0));
                        const {data:second}=await worker.recognize(normalized);
                        lines.push(...(second.lines||[]));
                    }
                    if (layoutCancelRef.current) throw new Error('Export cancelled.');
                    await invoke('submit_layout_image_ocr',{jobId:p.jobId,error:null,document:{width:p.width,height:p.height,
                        lines:lines.map(line=>({words:(line.words||[]).map(word=>({text:word.text,bbox:word.bbox,confidence:word.confidence}))}))}});
                } catch(error) {
                    await invoke('submit_layout_image_ocr',{jobId:p.jobId,document:null,error:String(error)}).catch(()=>{});
                }
            });
            if (layoutCancelRef.current) throw new Error('Export cancelled.');

            toast.loading('Building layout-preserving PDF…', { id: 'layout-pdf', style: toastStyle });
            invoke('export_layout_pdf', {
                request: {
                    runId,
                    sourcePdfPath: filePath,
                    outputPath,
                    targetLang,
                    pages: payloadPages,
                    imageOverlays,
                    imageReplacements,
                    imageTranslation,
                },
            }).catch((err) => resolveDone({ status: 'error', message: String(err) }));

            const outcome = await donePromise;

            if (outcome.status === 'completed') {
                toast.success(`PDF saved to ${outputPath}`, { id: 'layout-pdf', style: toastStyle, duration: 6000 });
            } else if (outcome.status === 'cancelled') {
                toast('Export cancelled.', { id: 'layout-pdf', style: toastStyle });
            } else {
                toast.error('Layout PDF export failed: ' + (outcome.message || 'unknown error'), { id: 'layout-pdf', style: toastStyle, duration: 10000 });
            }
        } catch (e) {
            if (layoutCancelRef.current) toast('Export cancelled.', { id: 'layout-pdf', style: toastStyle });
            else toast.error('Layout PDF export failed: ' + String(e), { id: 'layout-pdf', style: toastStyle, duration: 10000 });
        } finally {
            unlistenProgress?.();
            unlistenDone?.();
            unlistenImageOcr?.();
            ocrWorkerPromise?.then(worker=>worker.terminate()).catch(()=>{});
            setLayoutExporting(false);
        }
    };

    // EPUB export (Phase 3) — rebuild the source EPUB via the Rust `assemble_epub`.
    // mode 'translated' (default) swaps each translatable text node for its
    // translation; mode 'bilingual' keeps the original and appends the translation
    // per block. Only valid when the loaded source is an EPUB. Untranslated /
    // pass-through paragraphs (and all images/CSS/fonts) are preserved exactly.
    const handleExportEpub = async (mode = 'translated') => {
        try {
            if (pages.length === 0 || fileType !== 'epub') return;
            const bilingual = mode === 'bilingual';
            const baseName = (filePath.split(/[\\/]/).pop() || 'document').replace(/\.epub$/i, '');
            const suffix = bilingual ? `${targetLang}_bilingual` : targetLang;
            const outputPath = await save({
                filters: [{ name: 'EPUB', extensions: ['epub'] }],
                defaultPath: `${baseName}_${suffix}.epub`,
            });
            if (!outputPath) return;

            // Map normalized source paragraph -> translation. Keys match the segments
            // `extract_epub` produced, so the Rust side swaps them back in place.
            const translations = {};
            for (const page of pages) {
                for (const para of page.paragraphs) {
                    if (para.translated && para.translated.length > 0) {
                        translations[para.original] = para.translated;
                    }
                }
            }

            toast.loading(bilingual ? 'Building bilingual EPUB...' : 'Building translated EPUB...', { id: 'epub-export', style: toastStyle });
            await invoke('assemble_epub', { path: filePath, outputPath, translations, bilingual });
            toast.success(`EPUB saved to ${outputPath}`, { id: 'epub-export', style: toastStyle, duration: 6000 });
        } catch (e) {
            toast.error('EPUB export failed: ' + String(e), { id: 'epub-export', style: toastStyle, duration: 10000 });
        }
    };

    return (
        <div className="flex flex-col h-screen w-screen bg-background text-foreground overflow-hidden">
            <Toaster />

            {/* Header / Navigation bar */}
            <div className="flex justify-between items-center h-[50px] px-4 border-b border-default-100 bg-content1 shadow-sm select-none" data-tauri-drag-region="true">
                <div className="flex items-center gap-2">
                    <MdInsertDriveFile className="text-[24px] text-primary" />
                    <span className="font-semibold text-lg">Bilingual Document Translation (PDF / EPUB)</span>
                </div>
            </div>

            {/* Main Area */}
            <div className="flex-1 flex flex-col p-4 overflow-hidden gap-4">
                
                {/* Landing File Drop Zone / Setup if Idle */}
                {status === 'idle' && pages.length === 0 && (
                    <div className="flex-1 flex flex-col justify-center items-center border-2 border-dashed border-default-300 rounded-xl bg-content1/50 backdrop-blur-md p-8 gap-6 transition-all hover:border-primary">
                        <MdInsertDriveFile className="text-[72px] text-default-400 animate-bounce" />
                        <div className="text-center">
                            <p className="text-xl font-medium">Drag & Drop your PDF or EPUB file here</p>
                            <p className="text-sm text-default-400 mt-1">Or click the button below to browse files</p>
                        </div>
                        <Button color="primary" variant="flat" onPress={handleSelectFile}>
                            Browse Files
                        </Button>

                        {filePath && (
                            <Card className="max-w-[400px] border border-primary/20 bg-content2">
                                <CardBody className="flex flex-row items-center gap-3 p-3">
                                    <MdInsertDriveFile className="text-primary text-[24px] flex-shrink-0" />
                                    <div className="overflow-hidden">
                                        <p className="font-semibold text-sm truncate">{filePath.split(/[\\/]/).pop()}</p>
                                        <p className="text-xs text-default-400 truncate">{filePath}</p>
                                    </div>
                                </CardBody>
                            </Card>
                        )}

                        {/* Configurations */}
                        {filePath && (
                            <div className="flex gap-4 items-center w-full max-w-[600px] mt-4 flex-wrap justify-center">
                                <Select
                                    label="Source Language"
                                    size="sm"
                                    variant="bordered"
                                    selectedKeys={[sourceLang]}
                                    onSelectionChange={(keys) => setSourceLang([...keys][0])}
                                    className="max-w-[150px]"
                                >
                                    {LANG_OPTIONS.map((l) => (
                                        <SelectItem key={l.value} value={l.value}>
                                            {l.label}
                                        </SelectItem>
                                    ))}
                                </Select>
                                <Select
                                    label="Target Language"
                                    size="sm"
                                    variant="bordered"
                                    selectedKeys={[targetLang]}
                                    onSelectionChange={(keys) => setTargetLang([...keys][0])}
                                    className="max-w-[150px]"
                                >
                                    {LANG_OPTIONS.slice(1).map((l) => (
                                        <SelectItem key={l.value} value={l.value}>
                                            {l.label}
                                        </SelectItem>
                                    ))}
                                </Select>
                                <Select
                                    label="Translation Engine"
                                    size="sm"
                                    variant="bordered"
                                    selectedKeys={[selectedEngine]}
                                    onSelectionChange={(keys) => setSelectedEngine([...keys][0])}
                                    className="max-w-[180px]"
                                >
                                    {translateServiceInstanceList.map((e) => (
                                        <SelectItem key={e} value={e}>
                                            {e}
                                        </SelectItem>
                                    ))}
                                </Select>
                                <Select
                                    label="Translation Mode"
                                    size="sm"
                                    variant="bordered"
                                    selectedKeys={[translationMode]}
                                    onSelectionChange={(keys) => setTranslationMode([...keys][0])}
                                    className="max-w-[180px]"
                                >
                                    <SelectItem key="page" value="page">
                                        Batched (Recommended)
                                    </SelectItem>
                                    <SelectItem key="paragraph" value="paragraph">
                                        Paragraph by Paragraph
                                    </SelectItem>
                                </Select>
                                <Tooltip content="Source characters packed into one API call. Higher = fewer requests but the model must hold more numbered segments straight at once. Weaker models (e.g. gemini-3.1-flash-lite) start mis-aligning or echoing the source above ~6000; stronger models (gemini-3.5-flash) handle 12000-15000 fine. Unique paragraphs are de-duplicated + cached, so a re-run only retries what failed.">
                                    <Input
                                        label="Chars / request"
                                        size="sm"
                                        variant="bordered"
                                        type="number"
                                        min={1000}
                                        max={40000}
                                        step={1000}
                                        value={String(charsPerRequest)}
                                        onValueChange={(v) => setCharsPerRequest(Math.max(1000, Math.min(40000, parseInt(v, 10) || 6000)))}
                                        isDisabled={translationMode !== 'page'}
                                        className="max-w-[130px]"
                                    />
                                </Tooltip>
                                <Tooltip content="Max requests/minute. Gemini free-tier safe = 10. Paid tier: bump up.">
                                    <Input
                                        label="Max RPM"
                                        size="sm"
                                        variant="bordered"
                                        type="number"
                                        min={1}
                                        max={600}
                                        value={String(rpmLimit)}
                                        onValueChange={(v) => setRpmLimit(Math.max(1, Math.min(600, parseInt(v, 10) || 10)))}
                                        className="max-w-[100px]"
                                    />
                                </Tooltip>
                                <Button color="primary" className="h-[40px] px-8" onPress={handleStartTranslation} startContent={<MdTranslate />}>
                                    Translate
                                </Button>
                            </div>
                        )}
                    </div>
                )}

                {/* Progress View */}
                {status !== 'idle' && status !== 'done' && pages.length === 0 && (
                    <div className="flex-1 flex flex-col justify-center items-center p-8 gap-4">
                        <Progress
                            size="md"
                            value={progressPercent}
                            color="primary"
                            className="max-w-md"
                            showValueLabel={true}
                        />
                        <p className="text-sm font-medium animate-pulse text-default-600">{progressText}</p>
                        {status === 'translating' && (
                            <Button size="sm" color="danger" variant="flat" onPress={handleCancel} startContent={<MdCancel />}>
                                Cancel
                            </Button>
                        )}
                    </div>
                )}

                {/* Completed bilingual viewer */}
                {pages.length > 0 && (
                    <div className="flex-1 flex flex-col overflow-hidden gap-3">
                        
                        {/* Control Toolbar */}
                        <div className="flex justify-between items-center p-2 rounded-lg bg-content1 border border-default-100 shadow-sm flex-wrap gap-2">
                            <div className="flex gap-2">
                                <Button size="sm" variant="light" startContent={<MdArrowBack />} onPress={handleReset}>
                                    Load New File
                                </Button>
                                {status !== 'done' && (
                                    <div className="flex items-center gap-2 px-2 border-l border-default-200">
                                        <Progress size="sm" value={progressPercent} className="w-[100px]" color="primary" />
                                        <span className="text-xs text-default-500 animate-pulse truncate max-w-[200px]">{progressText}</span>
                                        <Button isIconOnly size="sm" color="danger" variant="light" onPress={handleCancel}>
                                            <MdCancel />
                                        </Button>
                                    </div>
                                )}
                            </div>
                            <div className="flex gap-2">
                                <Button size="sm" variant="flat" onPress={handleExportMarkdown} isDisabled={pages.length === 0} startContent={<MdFileDownload />}>
                                    Export MD
                                </Button>
                                <Button size="sm" color="primary" variant="flat" onPress={handleExportHTML} isDisabled={pages.length === 0} startContent={<MdFileDownload />}>
                                    Export HTML
                                </Button>
                                {fileType === 'epub' && (
                                    <Dropdown>
                                        <DropdownTrigger>
                                            <Button
                                                size="sm"
                                                color="primary"
                                                isDisabled={pages.length === 0}
                                                startContent={<MdFileDownload />}
                                            >
                                                Save as EPUB
                                            </Button>
                                        </DropdownTrigger>
                                        <DropdownMenu
                                            aria-label="EPUB export mode"
                                            onAction={(key) => handleExportEpub(String(key))}
                                        >
                                            <DropdownItem key="translated" description="Replace each block with its translation">
                                                Translated only
                                            </DropdownItem>
                                            <DropdownItem key="bilingual" description="Keep the original and add the translation under each block">
                                                Bilingual (original + translation)
                                            </DropdownItem>
                                        </DropdownMenu>
                                    </Dropdown>
                                )}
                                {fileType === 'pdf' && (
                                    <div className="flex items-center gap-2 px-2 border-l border-default-200">
                                        <Tooltip content={t('config.document.translateImagesDesc')}>
                                            <div className="flex">
                                                <Switch size="sm" isSelected={translateImages} onValueChange={setTranslateImages}>
                                                    <span className="text-xs">{t('config.document.translateImages')}</span>
                                                </Switch>
                                            </div>
                                        </Tooltip>
                                        {translateImages && (
                                            <Select
                                                aria-label={t('config.document.imageMethod')}
                                                size="sm"
                                                variant="bordered"
                                                selectedKeys={[imageMethod === 'ocr' ? 'gemini' : (imageMethod || 'gemini')]}
                                                onSelectionChange={(keys) => {
                                                    const k = [...keys][0];
                                                    if (k) setImageMethod(k);
                                                }}
                                                className="max-w-[170px]"
                                            >
                                                <SelectItem key="gemini" value="gemini">
                                                    {t('config.document.imageMethodGemini')}
                                                </SelectItem>
                                                <SelectItem key="service" value="service">
                                                    {t('config.document.imageMethodService')}
                                                </SelectItem>
                                            </Select>
                                        )}
                                        {translateImages && (imageMethod || 'gemini') === 'service' && (
                                            <Tooltip content={t('config.document.imageServiceUrlDesc')}>
                                                <Input
                                                    aria-label={t('config.document.imageServiceUrl')}
                                                    size="sm"
                                                    variant="bordered"
                                                    value={imageServiceUrl ?? DEFAULT_IMAGE_SERVICE_URL}
                                                    onValueChange={setImageServiceUrl}
                                                    placeholder={DEFAULT_IMAGE_SERVICE_URL}
                                                    className="max-w-[210px]"
                                                />
                                            </Tooltip>
                                        )}
                                    </div>
                                )}
                                {fileType !== 'epub' && (
                                <>
                                {layoutExporting && <Button size="sm" color="danger" onPress={() => {
                                    layoutCancelRef.current = true;
                                    invoke('cancel_layout_export').catch((error) => toast.error(String(error), { style: toastStyle }));
                                }}>{t('common.cancel')}</Button>}
                                <Dropdown>
                                    <DropdownTrigger>
                                        <Button
                                            size="sm"
                                            color="primary"
                                            isDisabled={pages.length === 0 || layoutExporting}
                                            startContent={<MdFileDownload />}
                                        >
                                            Save as PDF
                                        </Button>
                                    </DropdownTrigger>
                                    <DropdownMenu
                                        aria-label="PDF export mode"
                                        onAction={(key) => {
                                            const k = String(key);
                                            if (k.startsWith('layout:')) {
                                                handleExportLayoutPdf();
                                            } else if (k.startsWith('print:')) {
                                                handleExportPdf(k.slice('print:'.length));
                                            } else {
                                                handleExportPdfNative(k.slice('native:'.length));
                                            }
                                        }}
                                    >
                                        <DropdownItem key="layout:preserve" description={t('config.document.exportLayoutDesc')} showDivider>
                                            {t('config.document.exportLayout')}
                                        </DropdownItem>
                                        <DropdownItem key="native:bilingual" description="Original page image + translation, stacked">
                                            Bilingual (stacked)
                                        </DropdownItem>
                                        <DropdownItem key="native:sidebyside" description="Original page image left, translation right">
                                            Side-by-side
                                        </DropdownItem>
                                        <DropdownItem key="native:translated" description="Translation only" showDivider>
                                            Translated only
                                        </DropdownItem>
                                        <DropdownItem key="print:bilingual" description="Fallback: browser print dialog (text only)">
                                            Print via browser…
                                        </DropdownItem>
                                    </DropdownMenu>
                                </Dropdown>
                                </>
                                )}
                            </div>
                        </div>

                        {/* Split Bilingual Paragraph Grid list */}
                        <div className="flex-1 overflow-y-auto pr-1 flex flex-col gap-4">
                            {pages.map((page) => (
                                <Card key={page.index} shadow="sm" className="border border-default-100">
                                    <CardHeader className="bg-content2/50 py-1.5 px-4 font-semibold text-sm text-primary">
                                        {fileType === 'epub' ? 'Section' : 'Page'} {page.index}
                                    </CardHeader>
                                    <CardBody className="p-0 divide-y divide-default-100">
                                        {page.paragraphs.map((para, idx) => (
                                            <div key={idx} className="grid grid-cols-1 md:grid-cols-2 gap-4 p-4 hover:bg-content2/30 transition-all">
                                                
                                                {/* Original Text */}
                                                <div className="text-sm leading-relaxed select-text font-medium border-r border-default-100 pr-2">
                                                    {para.original}
                                                </div>

                                                {/* Translated Text */}
                                                <div className="text-sm leading-relaxed select-text text-default-600">
                                                    {para.translating && (
                                                        <span className="text-xs text-primary animate-pulse font-medium">Translating paragraph...</span>
                                                    )}
                                                    {para.error && (
                                                        <span className="text-xs text-danger font-medium">{para.error}</span>
                                                    )}
                                                    {!para.translating && !para.error && (para.translated || <span className="text-xs text-default-300">*[Waiting to Translate]*</span>)}
                                                </div>
                                            </div>
                                        ))}
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

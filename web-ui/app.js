import { buildSrt, buildVtt } from '/subtitle.js';
const $ = (id) => document.getElementById(id);
const secret = location.hash.slice(1);
if (secret) {
  sessionStorage.setItem('turktranslate-session', secret);
  history.replaceState(null, '', '/');
}
const token = sessionStorage.getItem('turktranslate-session') || '';
let phase = 'empty';
let documentFormat = 'pdf';
let pollTimer;
let ocrTimer;
let ocrProcessing = false;
let ocrWorker;
let ocrLanguage;
let fileName = sessionStorage.getItem('turktranslate-filename') || '';

function show(id, visible) { $(id).classList.toggle('hidden', !visible); }
function notice(message) { $('notice').textContent = message || ''; show('notice', Boolean(message)); }
async function api(path, options = {}) {
  const response = await fetch(path, { ...options, headers: { Authorization: `Bearer ${token}`, ...(options.headers || {}) }, cache: 'no-store' });
  if (!response.ok) {
    let message = 'İşlem gerçekleştirilemedi.';
    try { message = (await response.json()).error || message; } catch (_) {}
    throw new Error(message);
  }
  return response;
}
function setBusy(busy) {
  $('translate').disabled = busy || !['loaded', 'translated', 'ready', 'error'].includes(phase);
  $('change-file').disabled = busy;
  $('choose').disabled = busy;
  $('source').disabled = busy;
  $('target').disabled = busy;
  $('key').disabled = busy;
  $('model').disabled = busy;
  $('rpm').disabled = busy;
  $('translate-images').disabled = busy;
  $('bilingual').disabled = busy;
}
function element(tag, className, text) {
  const node = document.createElement(tag);
  if (className) node.className = className;
  if (text != null) node.textContent = text;
  return node;
}
function drawPages(pages) {
  const container = $('pages');
  const fragment = document.createDocumentFragment();
  for (const page of pages) {
    fragment.append(element('div', 'page-label', `${documentFormat === 'epub' ? 'BÖLÜM' : 'SAYFA'} ${page.index}`));
    for (const block of page.blocks) {
      const row = element('div', 'paragraph');
      row.append(element('div', '', block.original));
      const content = block.protected ? 'Özgün içerik korunacak' : (block.translation || block.error || 'Çeviri bekleniyor');
      row.append(element('div', !block.translation ? 'pending' : '', content));
      fragment.append(row);
    }
  }
  container.replaceChildren(fragment);
  $('reader-count').textContent = `${pages.length} ${documentFormat === 'epub' ? 'bölüm' : 'sayfa'}`;
}
async function refresh() {
  try {
    const data = await (await api('/api/status')).json();
    phase = data.phase || 'empty';
    documentFormat = data.format || 'pdf';
    $('file-type').textContent = documentFormat.toUpperCase();
    show('bilingual-row', documentFormat === 'epub');
    show('images-row', documentFormat === 'pdf');
    if (data.pages && data.pages.length) {
      show('file-summary', true); show('dropzone', false);
      $('file-name').textContent = fileName || 'Yüklü belge.pdf';
      $('file-info').textContent = `${data.pages.length} ${documentFormat === 'epub' ? 'bölüm' : 'sayfa'}`;
    }
    const busy = phase === 'translating' || phase === 'exporting';
    setBusy(busy);
    show('progress-card', busy || phase === 'error');
    show('cancel', busy && !(phase === 'exporting' && documentFormat === 'epub'));
    show('result-card', phase === 'ready');
    $('progress-title').textContent = phase === 'exporting' ? `${documentFormat.toUpperCase()} hazırlanıyor` : phase === 'error' ? 'İşlem tamamlanamadı' : 'Çeviri devam ediyor';
    $('progress-message').textContent = data.message || '';
    const percent = Math.max(0, Math.min(100, Math.round(data.percent || 0)));
    $('progress-value').textContent = `${percent}%`;
    $('progress-bar').style.width = `${percent}%`;
    const hasResult = data.pages && data.pages.some((page) => page.blocks.some((b) => b.translation || b.error));
    show('reader', hasResult);
    if (hasResult) drawPages(data.pages);
    if (phase === 'translated') {
      phase = 'exporting';
      await api('/api/export', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ bilingual: $('bilingual').checked, translateImages: documentFormat === 'pdf' && $('translate-images').checked, apiKey: $('key').value.trim(), model: $('model').value.trim(), source: $('source').value, target: $('target').value, rpm: Number($('rpm').value) }) });
    }
    if (phase === 'exporting' && documentFormat === 'pdf' && (data.imageOcr || $('translate-images').checked) && !ocrTimer) pollOcr();
    if (phase !== 'exporting') { clearTimeout(ocrTimer); ocrTimer = null; }
    if (busy || phase === 'exporting') pollTimer = setTimeout(refresh, 1500);
  } catch (error) { setBusy(false); notice(error.message); }
}
async function loadFile(file) {
  if (!file) return;
  if (file.size > 100 * 1024 * 1024 || !/\.(pdf|epub)$/i.test(file.name)) { notice('En fazla 100 MB boyutunda bir PDF veya EPUB seçin.'); return; }
  notice(''); clearTimeout(pollTimer); setBusy(true);
  show('progress-card', true); show('result-card', false); show('reader', false);
  $('progress-title').textContent = 'Belge okunuyor'; $('progress-message').textContent = 'İçerik ve metin alanları hazırlanıyor…'; $('progress-value').textContent = ''; $('progress-bar').style.width = '10%';
  try {
    const data = await (await api('/api/upload', { method: 'POST', body: file, headers: { 'Content-Type': 'application/pdf' } })).json();
    fileName = file.name; documentFormat = data.format; sessionStorage.setItem('turktranslate-filename', fileName); phase = 'loaded'; $('file-name').textContent = fileName;
    $('file-type').textContent = documentFormat.toUpperCase(); show('bilingual-row', documentFormat === 'epub'); show('images-row', documentFormat === 'pdf');
    $('file-info').textContent = `${data.pages} ${documentFormat === 'epub' ? 'bölüm' : 'sayfa'} · ${data.blocks} metin alanı · ${(file.size / 1024 / 1024).toFixed(1)} MB`;
    show('file-summary', true); show('dropzone', false); show('progress-card', false);
  } catch (error) { notice(error.message); show('progress-card', false); }
  setBusy(false);
}
$('choose').addEventListener('click', (e) => { e.stopPropagation(); $('file').click(); });
$('change-file').addEventListener('click', () => $('file').click());
$('dropzone').addEventListener('click', () => $('file').click());
$('dropzone').addEventListener('keydown', (e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); $('file').click(); } });
$('file').addEventListener('change', (e) => { loadFile(e.target.files[0]); e.target.value = ''; });
for (const name of ['dragenter', 'dragover']) $('dropzone').addEventListener(name, (e) => { e.preventDefault(); $('dropzone').classList.add('drag'); });
for (const name of ['dragleave', 'drop']) $('dropzone').addEventListener(name, (e) => { e.preventDefault(); $('dropzone').classList.remove('drag'); });
$('dropzone').addEventListener('drop', (e) => loadFile(e.dataTransfer.files[0]));
$('translate').addEventListener('click', async () => {
  notice('');
  if (!$('key').value.trim()) { notice('Devam etmek için Gemini API anahtarınızı girin.'); $('key').focus(); return; }
  setBusy(true);
  try {
    await api('/api/translate', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ apiKey: $('key').value.trim(), model: $('model').value.trim(), source: $('source').value, target: $('target').value, rpm: Number($('rpm').value), translateImages: $('translate-images').checked }) });
    phase = 'translating'; refresh();
  } catch (error) { setBusy(false); notice(error.message); }
});
$('cancel').addEventListener('click', async () => { try { await api('/api/cancel', { method: 'POST' }); $('progress-message').textContent = 'İşlem durduruluyor…'; } catch (error) { notice(error.message); } });
$('download').addEventListener('click', async () => {
  try {
    const blob = await (await api('/api/download')).blob();
    const url = URL.createObjectURL(blob); const link = document.createElement('a');
    link.href = url; link.download = `${fileName.replace(/\.(pdf|epub)$/i, '') || 'belge'}-ceviri.${documentFormat}`; link.click();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
  } catch (error) { notice(error.message); }
});
async function pollOcr() {
  if (ocrProcessing) return;
  ocrProcessing = true;
  try {
    const job = await (await api('/api/ocr/job')).json();
    if (job) {
      try {
        if (!ocrWorker || ocrLanguage !== job.language) {
          if (ocrWorker) await ocrWorker.terminate();
          const { default: tesseract } = await import('/tesseract.js');
          ocrWorker = await tesseract.createWorker(job.language, 1, { workerPath: '/worker.min.js', corePath: '/tesseract-core-simd-lstm.wasm.js', langPath: 'https://pub-f6afb74f13c64cd89561b4714dca1c27.r2.dev' });
          ocrLanguage = job.language;
        }
        await ocrWorker.setParameters({ tessedit_pageseg_mode: String(job.psm), preserve_interword_spaces: '1' });
        const read = async (base64) => (await ocrWorker.recognize(Uint8Array.from(atob(base64), (c) => c.charCodeAt(0)))).data.lines || [];
        const lines = [...await read(job.imageBase64), ...(job.normalizedBase64 ? await read(job.normalizedBase64) : [])];
        const document = { width: job.width, height: job.height, lines: lines.map((line) => ({ words: (line.words || []).map((word) => ({ text: word.text, bbox: word.bbox, confidence: word.confidence })) })) };
        await api('/api/ocr/submit', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ runId: job.runId, jobId: job.jobId, document }) });
      } catch (error) {
        await api('/api/ocr/submit', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ runId: job.runId, jobId: job.jobId, error: String(error), document: null }) }).catch(() => {});
      }
    }
  } catch (error) { notice(`OCR bağlantısı: ${error.message}`); }
  finally { ocrProcessing = false; ocrTimer = phase === 'exporting' ? setTimeout(pollOcr, 1000) : null; }
}
let editingTerm = null;
function glossaryNotice(message) { $('glossary-notice').textContent = message || ''; show('glossary-notice', Boolean(message)); }
function resetTerm() {
  editingTerm = null; $('glossary-form').reset(); $('term-source-lang').value = 'en'; $('term-target-lang').value = 'tr';
  $('term-active').checked = true; $('glossary-form-title').textContent = 'Yeni terim'; glossaryNotice('');
}
async function loadGlossary() {
  try {
    const entries = await (await api('/api/glossary')).json();
    $('glossary-count').textContent = `${entries.length} terim`;
    const fragment = document.createDocumentFragment();
    if (!entries.length) fragment.append(element('div', 'glossary-empty', 'Henüz terim eklenmedi.'));
    for (const entry of entries) {
      const row = element('div', 'glossary-item'); const main = element('div', 'glossary-item-main');
      main.append(element('strong', '', `${entry.source_term} → ${entry.target_term}`));
      main.append(element('span', '', `${entry.source_lang} → ${entry.target_lang}${entry.scope ? ` · ${entry.scope}` : ''}${entry.active ? '' : ' · Pasif'}`));
      const actions = element('div', 'glossary-actions');
      const edit = element('button', 'text-button', 'Düzenle'); edit.type = 'button';
      edit.addEventListener('click', () => {
        editingTerm = entry.id; $('glossary-form-title').textContent = 'Terimi düzenle';
        $('term-source').value = entry.source_term; $('term-target').value = entry.target_term;
        $('term-source-lang').value = entry.source_lang; $('term-target-lang').value = entry.target_lang;
        $('term-scope').value = entry.scope || ''; $('term-notes').value = entry.notes || ''; $('term-case').checked = entry.case_sensitive; $('term-active').checked = entry.active;
        $('term-source').focus();
      });
      const remove = element('button', 'text-button', 'Sil'); remove.type = 'button';
      remove.addEventListener('click', async () => {
        if (!confirm(`“${entry.source_term}” terimini silmek istiyor musunuz?`)) return;
        try { await api('/api/glossary', { method: 'DELETE', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ id: entry.id }) }); if (editingTerm === entry.id) resetTerm(); await loadGlossary(); }
        catch (error) { glossaryNotice(error.message); }
      });
      actions.append(edit, remove); row.append(main, actions); fragment.append(row);
    }
    $('glossary-items').replaceChildren(fragment);
  } catch (error) { glossaryNotice(error.message); }
}
function selectView(name) {
  for (const view of ['document', 'glossary', 'media', 'text']) { show(`${view}-view`, view === name); $(`nav-${view}`).classList.toggle('active', view === name); }
  $('current-view').textContent = { document: 'Belge çevirisi', glossary: 'Terim sözlüğü', media: 'Ses ve video', text: 'Metin çevirisi' }[name];
}
$('nav-document').addEventListener('click', () => selectView('document'));
$('nav-glossary').addEventListener('click', () => { selectView('glossary'); loadGlossary(); });
$('nav-media').addEventListener('click', () => { selectView('media'); if (!$('media-key').value) $('media-key').value = $('key').value; refreshMedia(); refreshLive(); });
$('nav-text').addEventListener('click', () => { selectView('text'); if (!$('text-key').value) $('text-key').value = $('key').value; refreshText(); });
$('term-cancel').addEventListener('click', resetTerm);
$('glossary-form').addEventListener('submit', async (event) => {
  event.preventDefault(); glossaryNotice('');
  const entry = { source_term: $('term-source').value.trim(), target_term: $('term-target').value.trim(), source_lang: $('term-source-lang').value.trim(), target_lang: $('term-target-lang').value.trim(), scope: $('term-scope').value.trim() || null, case_sensitive: $('term-case').checked, active: $('term-active').checked, notes: $('term-notes').value.trim() || null };
  try { await api('/api/glossary', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ id: editingTerm, entry }) }); resetTerm(); await loadGlossary(); }
  catch (error) { glossaryNotice(error.message); }
});
$('glossary-export').addEventListener('click', async () => {
  try { const data = await (await api('/api/glossary')).json(); const url = URL.createObjectURL(new Blob([JSON.stringify(data, null, 2)], { type: 'application/json' })); const link = document.createElement('a'); link.href = url; link.download = 'terim-sozlugu.json'; link.click(); setTimeout(() => URL.revokeObjectURL(url), 1000); }
  catch (error) { glossaryNotice(error.message); }
});
$('glossary-import').addEventListener('click', () => $('glossary-file').click());
$('glossary-file').addEventListener('change', async (event) => {
  const file = event.target.files[0]; event.target.value = ''; if (!file) return;
  try {
    if (file.size > 5 * 1024 * 1024) throw new Error('Sözlük dosyası 5 MB sınırını aşıyor.');
    const entries = JSON.parse(await file.text()); if (!Array.isArray(entries)) throw new Error('JSON dosyası bir terim listesi içermeli.');
    let count = 0;
    for (const item of entries) {
      const entry = { source_term: String(item.source_term || ''), target_term: String(item.target_term || ''), source_lang: String(item.source_lang || '*'), target_lang: String(item.target_lang || '*'), scope: item.scope || null, case_sensitive: Boolean(item.case_sensitive), active: item.active !== false, notes: item.notes || null };
      await api('/api/glossary', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ entry }) }); count++;
    }
    await loadGlossary(); glossaryNotice(`${count} terim içe aktarıldı.`);
  } catch (error) { await loadGlossary(); glossaryNotice(`İçe aktarma durdu: ${error.message}`); }
});
let mediaPhase = 'empty';
let mediaCues = [];
let mediaName = sessionStorage.getItem('turktranslate-media-name') || '';
let mediaTimer;
function mediaNotice(message) { $('media-notice').textContent = message || ''; show('media-notice', Boolean(message)); }
function mediaBusy(busy) {
  $('media-transcribe').disabled = busy || !['loaded', 'transcribed', 'translated', 'error'].includes(mediaPhase);
  $('media-translate').disabled = busy || mediaCues.length === 0;
  $('media-change').disabled = busy; $('media-choose').disabled = busy;
  for (const id of ['media-source', 'media-target', 'media-model', 'media-key', 'media-rpm']) $(id).disabled = busy;
}
function timecode(seconds) { const n = Math.max(0, Math.floor(Number(seconds) || 0)); return [Math.floor(n / 3600), Math.floor(n / 60) % 60, n % 60].map((v) => String(v).padStart(2, '0')).join(':'); }
function drawCues() {
  const fragment = document.createDocumentFragment();
  for (const cue of mediaCues) {
    const row = element('div', 'paragraph'); const original = element('div');
    original.append(element('span', 'cue-time', `${timecode(cue.start)} — ${timecode(cue.end)}`), document.createTextNode(cue.original));
    row.append(original, element('div', cue.translated ? '' : 'pending', cue.translated || cue.error || 'Çeviri henüz yapılmadı')); fragment.append(row);
  }
  $('media-cues').replaceChildren(fragment); $('media-count').textContent = `${mediaCues.length} bölüm`;
}
async function refreshMedia() {
  clearTimeout(mediaTimer);
  try {
    const data = await (await api('/api/media/status')).json(); mediaPhase = data.phase || 'empty'; mediaCues = data.cues || [];
    const busy = ['transcribing', 'translating'].includes(mediaPhase); mediaBusy(busy);
    if (mediaPhase !== 'empty') { show('media-file-summary', true); show('media-dropzone', false); $('media-file-name').textContent = mediaName || 'Yüklü medya'; }
    show('media-progress', busy || mediaPhase === 'error'); show('media-cancel', busy); show('media-result', mediaCues.length > 0);
    $('media-progress-title').textContent = mediaPhase === 'transcribing' ? 'Konuşma algılanıyor' : mediaPhase === 'translating' ? 'Altyazı çevriliyor' : 'İşlem tamamlanamadı';
    $('media-message').textContent = data.message || ''; const percent = Math.max(0, Math.min(100, Math.round(data.percent || 0))); $('media-percent').textContent = `${percent}%`; $('media-bar').style.width = `${percent}%`;
    if (mediaCues.length) drawCues();
    if (busy) mediaTimer = setTimeout(refreshMedia, 1500);
  } catch (error) { mediaBusy(false); mediaNotice(error.message); }
}
async function loadMedia(file) {
  if (!file) return; const extension = (file.name.split('.').pop() || '').toLowerCase();
  if (file.size > 2 * 1024 * 1024 * 1024) { mediaNotice('Medya dosyası 2 GB sınırını aşıyor.'); return; }
  mediaNotice(''); mediaBusy(true); show('media-progress', true); $('media-progress-title').textContent = 'Dosya hazırlanıyor'; $('media-message').textContent = 'Dosya yerel çalışma alanına aktarılıyor…'; $('media-percent').textContent = ''; $('media-bar').style.width = '5%';
  try {
    const result = await (await api('/api/media/upload', { method: 'POST', headers: { 'Content-Type': 'application/octet-stream', 'X-File-Extension': extension }, body: file })).json();
    mediaName = file.name; sessionStorage.setItem('turktranslate-media-name', mediaName); mediaPhase = 'loaded'; mediaCues = [];
    $('media-file-name').textContent = mediaName; $('media-file-info').textContent = `${(result.size / 1024 / 1024).toFixed(1)} MB`; show('media-dropzone', false); show('media-file-summary', true); show('media-progress', false); show('media-result', false);
  } catch (error) { show('media-progress', false); mediaNotice(error.message); }
  mediaBusy(false);
}
function mediaOptions() { return { apiKey: $('media-key').value.trim(), model: $('media-model').value.trim(), source: $('media-source').value, target: $('media-target').value, rpm: Number($('media-rpm').value) }; }
$('media-choose').addEventListener('click', () => $('media-file').click()); $('media-change').addEventListener('click', () => $('media-file').click());
$('media-file').addEventListener('change', (event) => { loadMedia(event.target.files[0]); event.target.value = ''; });
$('media-transcribe').addEventListener('click', async () => { mediaNotice(''); if (!$('media-key').value.trim()) { mediaNotice('Gemini API anahtarını girin.'); return; } mediaBusy(true); try { await api('/api/media/transcribe', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(mediaOptions()) }); mediaPhase = 'transcribing'; refreshMedia(); } catch (error) { mediaBusy(false); mediaNotice(error.message); } });
$('media-translate').addEventListener('click', async () => { mediaNotice(''); if (!$('media-key').value.trim()) { mediaNotice('Gemini API anahtarını girin.'); return; } mediaBusy(true); try { await api('/api/media/translate', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(mediaOptions()) }); mediaPhase = 'translating'; refreshMedia(); } catch (error) { mediaBusy(false); mediaNotice(error.message); } });
$('media-cancel').addEventListener('click', async () => { try { await api('/api/media/cancel', { method: 'POST' }); $('media-message').textContent = 'İşlem durduruluyor…'; } catch (error) { mediaNotice(error.message); } });
$('subtitle-download').addEventListener('click', () => {
  const mode = $('subtitle-mode').value; const format = $('subtitle-format').value;
  if (mode !== 'original' && mediaCues.some((c) => !c.translated)) { mediaNotice('Önce tüm altyazıları çevirin veya orijinal dil seçeneğini kullanın.'); return; }
  const data = format === 'vtt' ? buildVtt(mediaCues, mode) : buildSrt(mediaCues, mode); const url = URL.createObjectURL(new Blob([data], { type: 'text/plain;charset=utf-8' })); const link = document.createElement('a'); link.href = url; link.download = `${mediaName.replace(/\.[^.]+$/, '') || 'altyazi'}-altyazi.${format}`; link.click(); setTimeout(() => URL.revokeObjectURL(url), 1000);
});
let liveTimer;
async function refreshLive() {
  clearTimeout(liveTimer);
  try {
    const data = await (await api('/api/live/status')).json();
    const active = ['starting', 'listening', 'transcribing'].includes(data.state);
    $('live-start').disabled = active; $('live-stop').disabled = !active;
    $('live-status').textContent = active ? (data.state === 'transcribing' ? 'Konuşma işleniyor' : 'Sistem sesi dinleniyor') : data.state === 'error' ? 'Hata' : 'Kapalı';
    $('live-original').textContent = data.original || data.message || '';
    $('live-translation').textContent = data.translated || '';
    if (active) liveTimer = setTimeout(refreshLive, 3000);
  } catch (error) { mediaNotice(error.message); }
}
$('live-start').addEventListener('click', async () => {
  mediaNotice(''); if (!$('media-key').value.trim()) { mediaNotice('Gemini API anahtarını girin.'); return; }
  $('live-start').disabled = true;
  try { await api('/api/live/start', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ ...mediaOptions(), chunkSeconds: Number($('live-seconds').value), overlay: $('live-overlay').checked }) }); await refreshLive(); }
  catch (error) { $('live-start').disabled = false; mediaNotice(error.message); }
});
$('live-stop').addEventListener('click', async () => { try { await api('/api/live/stop', { method: 'POST' }); await refreshLive(); } catch (error) { mediaNotice(error.message); } });
$('live-passthrough').addEventListener('change', async () => { try { await api('/api/live/passthrough', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ enabled: $('live-passthrough').checked }) }); } catch (error) { mediaNotice(error.message); } });
let textTimer;
function textNotice(message) { $('text-notice').textContent = message || ''; show('text-notice', Boolean(message)); }
async function refreshText() {
  clearTimeout(textTimer);
  try {
    const data = await (await api('/api/text/status')).json(); const busy = data.phase === 'translating';
    $('text-start').disabled = busy; show('text-cancel', busy);
    if (data.original && !$('text-input').value) $('text-input').value = data.original;
    $('text-output').value = data.translation || ''; if (data.phase === 'error') textNotice(data.message || 'Çeviri tamamlanamadı.');
    if (busy) { $('text-output').placeholder = data.message || 'Çeviri yapılıyor…'; textTimer = setTimeout(refreshText, 1500); }
  } catch (error) { $('text-start').disabled = false; textNotice(error.message); }
}
$('text-input').addEventListener('input', () => { $('text-length').textContent = `${$('text-input').value.length} karakter`; });
$('text-clear').addEventListener('click', () => { $('text-input').value = ''; $('text-output').value = ''; $('text-length').textContent = '0 karakter'; textNotice(''); });
$('text-start').addEventListener('click', async () => {
  textNotice(''); if (!$('text-input').value.trim() || !$('text-key').value.trim()) { textNotice('Metin ve Gemini API anahtarı girin.'); return; }
  $('text-start').disabled = true;
  try { await api('/api/text/translate', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ text: $('text-input').value, apiKey: $('text-key').value.trim(), model: $('text-model').value.trim(), source: $('text-source').value, target: $('text-target').value }) }); await refreshText(); }
  catch (error) { $('text-start').disabled = false; textNotice(error.message); }
});
$('text-cancel').addEventListener('click', async () => { try { await api('/api/text/cancel', { method: 'POST' }); } catch (error) { textNotice(error.message); } });
$('text-copy').addEventListener('click', async () => { try { await navigator.clipboard.writeText($('text-output').value); } catch (error) { textNotice(`Kopyalanamadı: ${error.message}`); } });
$('text-speak').addEventListener('click', () => { if (!('speechSynthesis' in window)) { textNotice('Tarayıcınız seslendirmeyi desteklemiyor.'); return; } const utterance = new SpeechSynthesisUtterance($('text-output').value); utterance.lang = $('text-target').value; window.speechSynthesis.cancel(); window.speechSynthesis.speak(utterance); });
if (!token) notice('Oturum anahtarı bulunamadı. Sistem tepsisinden “Web arayüzünü aç” seçeneğini kullanın.');
else refresh();

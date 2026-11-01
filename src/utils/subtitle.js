// Subtitle formatting (Phase 4). Pure, dependency-free helpers so they can be unit
// tested without a window/network. Cues come from the Rust `transcribe_media`
// command as { start, end, text } with start/end in SECONDS; the frontend tracks
// each cue as { start, end, original, translated }. SRT/VTT differ only in the
// timestamp decimal separator (',' vs '.') and the leading WEBVTT header.

function clampSeconds(value) {
    const n = Number(value);
    if (!Number.isFinite(n) || n < 0) return 0;
    return n;
}

function pad(num, size) {
    return String(num).padStart(size, '0');
}

// Shared HH:MM:SS + milliseconds split so SRT and VTT stay in lockstep.
function splitTime(seconds) {
    const total = clampSeconds(seconds);
    const ms = Math.round(total * 1000);
    const hours = Math.floor(ms / 3_600_000);
    const minutes = Math.floor((ms % 3_600_000) / 60_000);
    const secs = Math.floor((ms % 60_000) / 1000);
    const millis = ms % 1000;
    return { hours, minutes, secs, millis };
}

export function formatSrtTime(seconds) {
    const { hours, minutes, secs, millis } = splitTime(seconds);
    return `${pad(hours, 2)}:${pad(minutes, 2)}:${pad(secs, 2)},${pad(millis, 3)}`;
}

export function formatVttTime(seconds) {
    const { hours, minutes, secs, millis } = splitTime(seconds);
    return `${pad(hours, 2)}:${pad(minutes, 2)}:${pad(secs, 2)}.${pad(millis, 3)}`;
}

// Resolve the text body for a cue in the requested mode. 'translated' falls back
// to the original when a cue wasn't translated, so the file is never missing a line.
function cueBody(cue, mode) {
    const original = (cue.original ?? '').trim();
    const translated = (cue.translated ?? '').trim();
    if (mode === 'translated') return translated || original;
    if (mode === 'bilingual') return translated ? `${original}\n${translated}` : original;
    return original;
}

function buildSubtitle(cues, mode, kind) {
    const fmt = kind === 'vtt' ? formatVttTime : formatSrtTime;
    const blocks = [];
    let index = 0;
    for (const cue of cues || []) {
        const body = cueBody(cue, mode);
        if (!body) continue; // skip wholly empty cues so we never emit blank entries
        index += 1;
        const timecode = `${fmt(cue.start)} --> ${fmt(cue.end)}`;
        // SRT entries are numbered; WEBVTT cue identifiers are optional, so omit them.
        const header = kind === 'vtt' ? timecode : `${index}\n${timecode}`;
        blocks.push(`${header}\n${body}`);
    }
    const body = blocks.join('\n\n');
    if (kind === 'vtt') {
        return `WEBVTT\n\n${body}${body ? '\n' : ''}`;
    }
    return body ? `${body}\n` : '';
}

// mode: 'original' | 'translated' | 'bilingual'
export function buildSrt(cues, mode = 'original') {
    return buildSubtitle(cues, mode, 'srt');
}

export function buildVtt(cues, mode = 'original') {
    return buildSubtitle(cues, mode, 'vtt');
}

import { Language } from './info';
import { Ollama } from 'ollama/browser';
import { applyGlossaryToPrompt } from '../../../utils/glossary';

export async function translate(text, from, to, options = {}) {
    const { config, setResult, detect } = options;

    let { stream, promptList, requestPath, model } = config;

    if (!/https?:\/\/.+/.test(requestPath)) {
        requestPath = `https://${requestPath}`;
    }
    if (requestPath.endsWith('/')) {
        requestPath = requestPath.slice(0, -1);
    }
    const ollama = new Ollama({ host: requestPath });

    // Glossary injection (Phase 1) — runs before variable substitution.
    const glossaryEntries = options.glossaryEntries ?? [];
    if (glossaryEntries.length > 0) {
        let injIdx = promptList.findIndex((m) => m.role === 'system');
        if (injIdx === -1) injIdx = promptList.findIndex((m) => m.role === 'user');
        if (injIdx !== -1) {
            promptList = promptList.map((m, i) =>
                i === injIdx
                    ? { ...m, content: applyGlossaryToPrompt(m.content ?? '', glossaryEntries) }
                    : m
            );
        }
    }

    promptList = promptList.map((item) => {
        return {
            ...item,
            content: item.content
                .replaceAll('$text', text)
                .replaceAll('$from', from)
                .replaceAll('$to', to)
                .replaceAll('$detect', Language[detect]),
        };
    });

    const response = await ollama.chat({ model, messages: promptList, stream: stream });

    if (stream) {
        let target = '';
        for await (const part of response) {
            target += part.message.content;
            if (setResult) {
                setResult(target + '_');
            } else {
                ollama.abort();
                return '[STREAM]';
            }
        }
        setResult(target.trim());
        return target.trim();
    } else {
        return response.message.content;
    }
}

export * from './Config';
export * from './info';

import React, { useEffect, useState } from 'react';
import { appWindow } from '@tauri-apps/api/window';
import { listen } from '@tauri-apps/api/event';
import { invoke } from '@tauri-apps/api/tauri';
import { useTranslation } from 'react-i18next';
import { Button, Tooltip } from '@nextui-org/react';
import { MdClose, MdAdd, MdRemove, MdTouchApp, MdSubtitles } from 'react-icons/md';

// Transparent, always-on-top, click-through subtitle overlay (Task B). It only
// DISPLAYS: the live-caption session is started/stopped from the Subtitle window
// (the controller that resolves the Gemini engine + glossary). This window listens
// to the events the Rust `media::live_caption` session emits:
//   live_caption          { partial, final, translated, ts }
//   live_caption://state  { state, message }
//   live_caption://passthrough { enabled }   (sent by the Subtitle controller)
export default function LiveCaption() {
    const { t } = useTranslation();
    const [original, setOriginal] = useState('');
    const [translated, setTranslated] = useState('');
    const [state, setState] = useState('listening');
    const [message, setMessage] = useState('');
    const [fontSize, setFontSize] = useState(30);
    const [showOriginal, setShowOriginal] = useState(true);
    const [clickThrough, setClickThrough] = useState(false);

    useEffect(() => {
        if (appWindow.label === 'live_caption') {
            appWindow.show().catch(() => {});
            appWindow.setAlwaysOnTop(true).catch(() => {});
        }
        // Force a transparent backdrop regardless of the global (light/dark) theme.
        const prevHtml = document.documentElement.style.background;
        const prevBody = document.body.style.background;
        document.documentElement.style.background = 'transparent';
        document.body.style.background = 'transparent';
        return () => {
            document.documentElement.style.background = prevHtml;
            document.body.style.background = prevBody;
        };
    }, []);

    const applyClickThrough = async (enabled) => {
        try {
            await appWindow.setIgnoreCursorEvents(enabled);
            setClickThrough(enabled);
        } catch {
            // ignore — older webviews may not support it
        }
    };

    useEffect(() => {
        let unCaption;
        let unState;
        let unPass;
        (async () => {
            unCaption = await listen('live_caption', (event) => {
                const p = event.payload;
                if (!p) return;
                if (typeof p.partial === 'string' && p.partial.length > 0) {
                    setOriginal(p.partial);
                    setTranslated('');
                }
                if (typeof p.final === 'string' && p.final.length > 0) {
                    setOriginal(p.final);
                    setTranslated(p.translated || '');
                }
            });
            unState = await listen('live_caption://state', (event) => {
                const p = event.payload;
                if (!p) return;
                if (p.state) setState(p.state);
                setMessage(p.message || '');
            });
            unPass = await listen('live_caption://passthrough', (event) => {
                applyClickThrough(!!event.payload?.enabled);
            });
        })();
        return () => {
            if (unCaption) unCaption();
            if (unState) unState();
            if (unPass) unPass();
        };
    }, []);

    const handleClose = async () => {
        try {
            await invoke('stop_live_caption');
        } catch {
            // session may already be stopped
        }
        try {
            await appWindow.close();
        } catch {
            // ignore
        }
    };

    const statusLabel = () => {
        if (state === 'error') return message || t('config.subtitle.live.error');
        if (state === 'stopped') return t('config.subtitle.live.stopped');
        if (state === 'transcribing') return t('config.subtitle.live.transcribing');
        return t('config.subtitle.live.overlayListening');
    };

    const hasText = original.length > 0 || translated.length > 0;
    const mainLine = translated || original;
    const subLine = translated && original && showOriginal ? original : '';

    return (
        <div className='w-screen h-screen flex flex-col justify-end items-stretch overflow-hidden p-2 select-none'>
            {/* Hover toolbar (hidden while click-through is on, since it can't receive clicks) */}
            {!clickThrough && (
                <div className='flex justify-end gap-1 mb-1 opacity-0 hover:opacity-100 transition-opacity duration-150'>
                    <Tooltip content='Show/hide original text'>
                        <Button isIconOnly size='sm' radius='full' variant='flat' className='bg-black/60 text-white min-w-7 w-7 h-7' onPress={() => setShowOriginal((v) => !v)}>
                            <MdSubtitles size={16} />
                        </Button>
                    </Tooltip>
                    <Tooltip content='Smaller text'>
                        <Button isIconOnly size='sm' radius='full' variant='flat' className='bg-black/60 text-white min-w-7 w-7 h-7' onPress={() => setFontSize((s) => Math.max(14, s - 2))}>
                            <MdRemove size={16} />
                        </Button>
                    </Tooltip>
                    <Tooltip content='Larger text'>
                        <Button isIconOnly size='sm' radius='full' variant='flat' className='bg-black/60 text-white min-w-7 w-7 h-7' onPress={() => setFontSize((s) => Math.min(72, s + 2))}>
                            <MdAdd size={16} />
                        </Button>
                    </Tooltip>
                    <Tooltip content='Enable click-through (disable it again from the Subtitle window)'>
                        <Button isIconOnly size='sm' radius='full' variant='flat' className='bg-black/60 text-white min-w-7 w-7 h-7' onPress={() => applyClickThrough(true)}>
                            <MdTouchApp size={16} />
                        </Button>
                    </Tooltip>
                    <Tooltip content='Close & stop'>
                        <Button isIconOnly size='sm' radius='full' variant='flat' className='bg-danger/80 text-white min-w-7 w-7 h-7' onPress={handleClose}>
                            <MdClose size={16} />
                        </Button>
                    </Tooltip>
                </div>
            )}

            {/* Caption bar — draggable to reposition (when not click-through) */}
            <div
                data-tauri-drag-region='true'
                className='flex-1 flex flex-col justify-center items-center rounded-xl px-5 py-3 text-center'
                style={{ background: 'rgba(0,0,0,0.62)', backdropFilter: 'blur(2px)', cursor: clickThrough ? 'default' : 'move' }}
            >
                {!hasText ? (
                    <span className='text-white/60' style={{ fontSize: Math.max(14, fontSize - 8), pointerEvents: 'none' }}>
                        {statusLabel()}
                    </span>
                ) : (
                    <>
                        <span
                            className='text-white font-semibold leading-tight'
                            style={{ fontSize, textShadow: '0 1px 4px rgba(0,0,0,0.9)', pointerEvents: 'none' }}
                        >
                            {mainLine}
                        </span>
                        {subLine && (
                            <span
                                className='text-white/65 mt-1 leading-tight'
                                style={{ fontSize: Math.max(12, fontSize - 10), pointerEvents: 'none' }}
                            >
                                {subLine}
                            </span>
                        )}
                        {!translated && original && state === 'transcribing' && (
                            <span className='text-primary-300 text-xs mt-1 animate-pulse' style={{ pointerEvents: 'none' }}>
                                {t('config.subtitle.live.overlayTranslating')}
                            </span>
                        )}
                    </>
                )}
            </div>
        </div>
    );
}

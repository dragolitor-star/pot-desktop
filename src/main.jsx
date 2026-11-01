import { ThemeProvider as NextThemesProvider } from 'next-themes';
import { appWindow } from '@tauri-apps/api/window';
import { NextUIProvider } from '@nextui-org/react';
import ReactDOM from 'react-dom/client';
import React from 'react';

import { initStore } from './utils/store';
import { initEnv } from './utils/env';
import App from './App';

if (import.meta.env.PROD && !window.__TT_BROWSER__) {
    document.addEventListener('contextmenu', (e) => {
        e.preventDefault();
    });
}

function renderApp() {
    const rootElement = document.getElementById('root');
    const root = ReactDOM.createRoot(rootElement);
    root.render(
        <NextUIProvider>
            <NextThemesProvider attribute='class'>
                <App />
            </NextThemesProvider>
        </NextUIProvider>
    );
}

// A rejection here used to be swallowed, leaving every window blank white with
// no clue why. Show what failed, and still render the app: config access is not
// worth losing the whole UI over.
initStore()
    .then(initEnv)
    .catch((err) => {
        console.error('Startup initialization failed:', err);
        const banner = document.createElement('pre');
        banner.style.cssText =
            'margin:0;padding:8px;font:12px/1.4 monospace;color:#b00;background:#fee;' +
            'white-space:pre-wrap;position:fixed;top:0;left:0;right:0;z-index:99999';
        banner.textContent = `Startup initialization failed:\n${err?.message ?? String(err)}`;
        document.body.prepend(banner);
    })
    .finally(renderApp);

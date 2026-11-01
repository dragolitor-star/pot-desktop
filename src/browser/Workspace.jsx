import React, { useEffect, useState } from 'react';
import { Button } from '@nextui-org/react';
import { invoke } from '@tauri-apps/api/tauri';
import { listen } from '@tauri-apps/api/event';
import { appWindow } from '@tauri-apps/api/window';
import toast, { Toaster } from 'react-hot-toast';
import Translate from '../window/Translate';
import Recognize from '../window/Recognize';
import Document from '../window/Document';
import Subtitle from '../window/Subtitle';
import General from '../window/Config/pages/General';
import TranslateSettings from '../window/Config/pages/Translate';
import RecognizeSettings from '../window/Config/pages/Recognize';
import Service from '../window/Config/pages/Service';
import History from '../window/Config/pages/History';
import Glossary from '../window/Config/pages/Glossary';
import Hotkey from '../window/Config/pages/Hotkey';
import Backup from '../window/Config/pages/Backup';
import About from '../window/Config/pages/About';
import Updater from '../window/Updater';
import './workspace.css';

const pages = {
    text: { title: 'Metin çevirisi', hint: 'Çeviri motorlarını karşılaştırın, dinleyin ve kelime koleksiyonunuza ekleyin.', component: Translate, window: 'translate', icon: '文' },
    ocr: { title: 'Görsel ve OCR', hint: 'Görsel yükleyin, panodan yapıştırın veya ekranın bir bölümünü seçin.', component: Recognize, window: 'recognize', icon: '▣' },
    document: { title: 'PDF ve EPUB', hint: 'Belgenizi çevirin; özgün düzeni, görselleri veya iki dilli çıktıyı seçin.', component: Document, window: 'document', icon: '▤' },
    subtitle: { title: 'Ses, video ve altyazı', hint: 'Dosyadan konuşma çıkarın, altyazıları çevirin veya canlı altyazıyı başlatın.', component: Subtitle, window: 'subtitle', icon: '♫' },
    service: { title: 'Servisler ve eklentiler', hint: 'Çeviri, OCR, seslendirme ve koleksiyon servislerinizi yapılandırın.', component: Service, icon: '◇' },
    glossary: { title: 'Terim sözlüğü', hint: 'Dil çiftleri ve alanlara göre tutarlı çeviri kuralları oluşturun.', component: Glossary, icon: 'Aa' },
    history: { title: 'Çeviri geçmişi', hint: 'Yerel çeviri geçmişinizi arayın ve yeniden kullanın.', component: History, icon: '↶' },
    general: { title: 'Genel ayarlar', hint: 'Dil, görünüm, başlangıç, ağ ve uygulama tercihleri.', component: General, icon: '⚙' },
    translateSettings: { title: 'Çeviri ayarları', hint: 'Dil algılama, pano, otomatik çeviri ve masaüstü davranışları.', component: TranslateSettings, icon: '⇄' },
    recognizeSettings: { title: 'OCR ayarları', hint: 'Tanıma dili ve ekran yakalama tercihleri.', component: RecognizeSettings, icon: '⊞' },
    hotkey: { title: 'Klavye kısayolları', hint: 'Sistem genelinde seçim ve ekran çevirisi kısayolları.', component: Hotkey, icon: '⌘' },
    backup: { title: 'Yedekle ve geri yükle', hint: 'Yerel dosya, WebDAV veya Aliyun ile mevcut ayarlarınızı yönetin.', component: Backup, icon: '⇧' },
    about: { title: 'Uygulama hakkında', hint: 'Sürüm, lisans, özgün Pot projesi ve günlükler.', component: About, icon: 'ⓘ' },
    updater: { title: 'Güncellemeler', hint: 'Yapılandırılmış güncelleme kaynağını denetleyin.', component: Updater, window: 'updater', icon: '↻', hidden: true },
};
const initialPage = () => {
    const key = new URLSearchParams(location.search).get('view');
    return key in pages ? key : 'text';
};
class PageBoundary extends React.Component {
    state = { error: null };
    static getDerivedStateFromError(error) { return { error }; }
    render() {
        return this.state.error ? <div role="alert" className="tt-page-error"><h2>Bu ekran açılamadı</h2><p>{String(this.state.error.message)}</p><Button onPress={() => this.setState({ error: null })}>Yeniden dene</Button></div> : this.props.children;
    }
}
export default function Workspace() {
    const [page, setPage] = useState(initialPage);
    const [visited, setVisited] = useState(() => new Set([initialPage()]));
    const [disconnected, setDisconnected] = useState('');
    const [uploading, setUploading] = useState(false);
    window.__TT_ACTIVE_VIEW__ = page;
    const navigate = key => {
        if (!(key in pages)) return;
        setVisited(old => new Set([...old, key])); setPage(key);
        history.replaceState(null, '', `/?view=${encodeURIComponent(key)}`);
    };
    useEffect(() => {
        const local = event => navigate(event.detail.page);
        const offline = event => setDisconnected(event.detail);
        window.addEventListener('tt:navigate', local);
        window.addEventListener('tt:disconnected', offline);
        const unlisten = listen('web:navigate', ({ payload }) => navigate(payload.page));
        return () => { window.removeEventListener('tt:navigate', local); window.removeEventListener('tt:disconnected', offline); unlisten.then(fn => fn()); };
    }, []);
    useEffect(() => {
        appWindow.label = pages[page].window || 'config';
        document.title = `${pages[page].title} · TurkTranslate`;
        window.dispatchEvent(new CustomEvent('tt:active-view', { detail: page }));
    }, [page]);
    async function importImage(file) {
        if (!file) return;
        if (file.size > 25 * 1024 * 1024) { toast.error('Görsel en fazla 25 MB olabilir.'); return; }
        setUploading(true);
        try { await invoke('web_import_image', { data: Array.from(new Uint8Array(await file.arrayBuffer())) }); navigate('ocr'); }
        catch (error) { toast.error(String(error)); }
        finally { setUploading(false); }
    }
    useEffect(() => {
        const paste = event => {
            const file = Array.from(event.clipboardData?.files || []).find(file => file.type.startsWith('image/'));
            if (file) { event.preventDefault(); void importImage(file); }
        };
        window.addEventListener('paste', paste);
        return () => window.removeEventListener('paste', paste);
    }, []);
    const desktopAction = action => invoke('web_desktop_action', { action }).catch(error => toast.error(String(error)));
    async function dropFile(event) {
        event.preventDefault();
        const file = event.dataTransfer.files?.[0];
        if (!file) return;
        if (file.type.startsWith('image/')) { await importImage(file); return; }
        if (!['document', 'subtitle'].includes(page)) { toast.error('Belgenizi PDF ve EPUB ya da Ses, video ve altyazı ekranına bırakın.'); return; }
        setUploading(true);
        try { window.__TT_FILE_DROP__(await window.__TT_UPLOAD__(file)); toast.success('Dosya yüklendi.'); }
        catch (error) { toast.error(String(error)); }
        finally { setUploading(false); }
    }
    return <div className="tt-workspace">
        <Toaster />
        <aside className="tt-sidebar">
            <a className="tt-brand" href="/?view=text" onClick={event => { event.preventDefault(); navigate('text'); }}><img src="/icon.svg" alt="" /><span>TurkTranslate<small>Yerel çeviri çalışma alanı</small></span></a>
            <nav aria-label="Çalışma alanları">{Object.entries(pages).filter(([, item]) => !item.hidden).map(([key, item], index) => <React.Fragment key={key}>{[4, 7].includes(index) && <div className="tt-nav-divider" />}<button aria-current={page === key ? 'page' : undefined} className={page === key ? 'active' : ''} onClick={() => navigate(key)}><span className="tt-nav-icon" aria-hidden="true">{item.icon}</span>{item.title}</button></React.Fragment>)}</nav>
            <p className="tt-local-status"><span /> Bu bilgisayarda çalışıyor<small>Pot · GPLv3</small></p>
        </aside>
        <main className="tt-main" onDragOver={event => { if (event.dataTransfer.types.includes('Files')) event.preventDefault(); }} onDrop={dropFile}>
            <header className="tt-header"><div><h1>{pages[page].title}</h1><p>{pages[page].hint}</p></div><Button size="sm" variant="flat" onPress={() => navigate('service')}>Servis ayarları</Button></header>
            {disconnected && <div className="tt-disconnected" role="alert">{disconnected} <button onClick={() => location.reload()}>Yeniden bağlan</button></div>}
            {uploading && <p role="status" className="tt-toolbar">Dosya bu bilgisayardaki uygulamaya aktarılıyor…</p>}
            {(page === 'text' || page === 'ocr') && <div className="tt-toolbar">
                <Button size="sm" onPress={() => desktopAction(page === 'ocr' ? 'ocr_recognize' : 'ocr_translate')}>Ekrandan alan seç</Button>
                <label className="tt-file-button">{uploading ? 'Görsel yükleniyor…' : 'Görsel yükle'}<input aria-label="OCR için görsel yükle" type="file" accept="image/png,image/jpeg,image/webp,image/bmp,image/gif" disabled={uploading} onChange={event => { void importImage(event.target.files?.[0]); event.target.value = ''; }} /></label>
                <span>Panodaki görseli Ctrl+V ile yapıştırabilirsiniz.</span>
            </div>}
            <div className="tt-content">{[...visited].filter(key => key === page || ['text', 'ocr', 'document', 'subtitle'].includes(key)).map(key => {
                const Component = pages[key].component;
                return <section key={key} hidden={page !== key} aria-label={pages[key].title} className={`tt-panel tt-panel-${key}`}><PageBoundary><Component /></PageBoundary></section>;
            })}</div>
        </main>
    </div>;
}

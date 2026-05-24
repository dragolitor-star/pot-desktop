import { Modal, ModalContent, ModalHeader, ModalBody, ModalFooter, useDisclosure, Progress, Card, CardBody, CardHeader } from '@nextui-org/react';
import { Button, Select, SelectItem, Switch, Textarea, Tooltip } from '@nextui-org/react';
import React, { useState, useEffect, useRef } from 'react';
import { save, open } from '@tauri-apps/api/dialog';
import { writeTextFile } from '@tauri-apps/api/fs';
import { invoke } from '@tauri-apps/api/tauri';
import toast, { Toaster } from 'react-hot-toast';
import { useTranslation } from 'react-i18next';
import { MdInsertDriveFile, MdTranslate, MdCancel, MdFileDownload, MdArrowBack } from 'react-icons/md';
import { listen } from '@tauri-apps/api/event';

import { useToastStyle, useConfig } from '../../hooks';
import * as builtinServices from '../../services/translate';
import { fetchActiveGlossary, applyGlossaryPostTranslate, BUILTIN_LLM_ENGINES } from '../../utils/glossary';
import { getServiceName, whetherPluginService, getInstanceName } from '../../utils/service_instance';

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

    // Config states from App
    const [translateServiceInstanceList] = useConfig('translate_service_list', ['google']);
    const [translateSecondLanguage] = useConfig('translate_second_language', 'en');

    // UI States
    const [filePath, setFilePath] = useState('');
    const [status, setStatus] = useState('idle'); // idle | loading_lib | extracting | translating | done
    const [progressPercent, setProgressPercent] = useState(0);
    const [progressText, setProgressText] = useState('');
    const [pages, setPages] = useState([]); // Array of { index: number, paragraphs: Array<{ original: string, translated: string, translating: boolean, error?: string }> }
    const [sourceLang, setSourceLang] = useState('auto');
    const [targetLang, setTargetLang] = useState('tr');
    const [selectedEngine, setSelectedEngine] = useState('');

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
                if (first.toLowerCase().endsWith('.pdf')) {
                    setFilePath(first);
                    toast.success('PDF File Loaded');
                } else {
                    toast.error('Only PDF files are supported');
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
                filters: [{ name: 'PDF', extensions: ['pdf'] }],
                multiple: false,
            });
            if (selected) {
                setFilePath(selected);
                toast.success('PDF File Loaded');
            }
        } catch (e) {
            toast.error(String(e), { style: toastStyle });
        }
    };

    // Main translation workflow
    const handleStartTranslation = async () => {
        if (!filePath) {
            toast.error('Please load a PDF file first');
            return;
        }

        cancelRef.current = false;
        setStatus('loading_lib');
        setProgressPercent(10);
        setProgressText('Verifying and initializing PDFium dynamic library...');

        try {
            // Step 1: Extract Pages
            setStatus('extracting');
            setProgressPercent(25);
            setProgressText('Loading PDF and extracting text layers...');
            
            const extractedPages = await invoke('extract_pdf_pages', { path: filePath });
            if (!Array.isArray(extractedPages) || extractedPages.length === 0) {
                throw new Error('No text layers extracted or PDF is empty.');
            }

            // Map into structured page-paragraph state
            const mappedPages = extractedPages.map((text, idx) => {
                // Split by double newlines or single newlines that group paragraphs
                const rawParas = text.split(/\n\s*\n/).map(p => p.trim()).filter(p => p.length > 0);
                const paragraphs = rawParas.map(p => ({
                    original: p,
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
            const _glossarySrc = sourceLang === 'auto' ? 'en' : sourceLang; // default fallback if auto
            const _glossaryEntries = await fetchActiveGlossary(_glossarySrc, targetLang, 'document');

            let totalParagraphs = mappedPages.reduce((acc, p) => acc + p.paragraphs.length, 0);
            let translatedCount = 0;

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

                    try {
                        let finalResult = '';
                        await new Promise((resolve, reject) => {
                            builtinServices[engineName].translate(
                                paragraphText,
                                LanguageEnum[sourceLang],
                                LanguageEnum[targetLang],
                                {
                                    config: {}, // empty context config
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
                        }).then((translatedText) => {
                            setPages(prev => {
                                const copy = [...prev];
                                copy[pageIdx].paragraphs[paraIdx].translated = String(translatedText);
                                copy[pageIdx].paragraphs[paraIdx].translating = false;
                                return copy;
                            });
                        });
                    } catch (err) {
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

            if (cancelRef.current) {
                setStatus('idle');
                toast.error('Translation cancelled.');
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

    // Cancel translation
    const handleCancel = () => {
        cancelRef.current = true;
    };

    // Reset View
    const handleReset = () => {
        setFilePath('');
        setPages([]);
        setStatus('idle');
        setProgressPercent(0);
        setProgressText('');
    };

    // Export Bilingual Markdown
    const handleExportMarkdown = async () => {
        try {
            let markdown = `# Bilingual PDF Translation: ${filePath.split(/[\\/]/).pop()}\n\n`;
            for (const page of pages) {
                markdown += `## Page ${page.index}\n\n`;
                for (const para of page.paragraphs) {
                    markdown += `**Original:**\n${para.original}\n\n`;
                    markdown += `**Translation:**\n${para.translated || '*[Not Translated]*'}\n\n`;
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
            let htmlContent = `
<!DOCTYPE html>
<html>
<head>
<meta charset="utf-8">
<title>Bilingual PDF Translation</title>
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
                htmlContent += `  <div class="page-section">\n    <div class="page-title">Page ${page.index}</div>\n`;
                for (const para of page.paragraphs) {
                    htmlContent += `    <div class="grid-row">\n`;
                    htmlContent += `      <div class="source">${para.original.replace(/\n/g, '<br>')}</div>\n`;
                    htmlContent += `      <div class="target">${(para.translated || '').replace(/\n/g, '<br>')}</div>\n`;
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

    return (
        <div className="flex flex-col h-screen w-screen bg-background text-foreground overflow-hidden">
            <Toaster />

            {/* Header / Navigation bar */}
            <div className="flex justify-between items-center h-[50px] px-4 border-b border-default-100 bg-content1 shadow-sm select-none" data-tauri-drag-region="true">
                <div className="flex items-center gap-2">
                    <MdInsertDriveFile className="text-[24px] text-primary" />
                    <span className="font-semibold text-lg">Bilingual Document Translation (PDF)</span>
                </div>
            </div>

            {/* Main Area */}
            <div className="flex-1 flex flex-col p-4 overflow-hidden gap-4">
                
                {/* Landing File Drop Zone / Setup if Idle */}
                {status === 'idle' && pages.length === 0 && (
                    <div className="flex-1 flex flex-col justify-center items-center border-2 border-dashed border-default-300 rounded-xl bg-content1/50 backdrop-blur-md p-8 gap-6 transition-all hover:border-primary">
                        <MdInsertDriveFile className="text-[72px] text-default-400 animate-bounce" />
                        <div className="text-center">
                            <p className="text-xl font-medium">Drag & Drop your PDF file here</p>
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
                            </div>
                        </div>

                        {/* Split Bilingual Paragraph Grid list */}
                        <div className="flex-1 overflow-y-auto pr-1 flex flex-col gap-4">
                            {pages.map((page) => (
                                <Card key={page.index} shadow="sm" className="border border-default-100">
                                    <CardHeader className="bg-content2/50 py-1.5 px-4 font-semibold text-sm text-primary">
                                        Page {page.index}
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

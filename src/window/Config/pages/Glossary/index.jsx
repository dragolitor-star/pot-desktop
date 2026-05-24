import { Modal, ModalContent, ModalHeader, ModalBody, ModalFooter, useDisclosure } from '@nextui-org/react';
import { Table, TableHeader, TableColumn, TableBody, TableRow, TableCell } from '@nextui-org/react';
import { Button, Input, Switch, Textarea, Tooltip, Select, SelectItem, Chip } from '@nextui-org/react';
import React, { useCallback, useEffect, useMemo, useState } from 'react';
import { save, open } from '@tauri-apps/api/dialog';
import { writeTextFile, readTextFile } from '@tauri-apps/api/fs';
import { invoke } from '@tauri-apps/api/tauri';
import toast, { Toaster } from 'react-hot-toast';
import { useTranslation } from 'react-i18next';

import { useToastStyle } from '../../../../hooks';
import { osType } from '../../../../utils/env';

// Short ISO-ish lang codes used across Pot
const LANG_OPTIONS = [
    { value: '*', label: '* (Wildcard)' },
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
    { value: 'pt_br', label: 'Português (BR)' },
    { value: 'vi', label: 'Tiếng Việt' },
    { value: 'id', label: 'Indonesia' },
    { value: 'th', label: 'ไทย' },
    { value: 'ar', label: 'العربية' },
    { value: 'hi', label: 'हिन्दी' },
    { value: 'nl', label: 'Nederlands' },
    { value: 'pl', label: 'Polski' },
    { value: 'uk', label: 'Українська' },
    { value: 'sv', label: 'Svenska' },
    { value: 'fa', label: 'فارسی' },
];

const EMPTY_FORM = {
    source_term: '',
    target_term: '',
    source_lang: 'en',
    target_lang: 'tr',
    scope: '',
    case_sensitive: false,
    active: true,
    notes: '',
};

export default function Glossary() {
    const { t } = useTranslation();
    const toastStyle = useToastStyle();

    // ── State ────────────────────────────────────────────────────────
    const [entries, setEntries] = useState([]);
    const [loading, setLoading] = useState(true);

    // Filters
    const [filterSourceLang, setFilterSourceLang] = useState('');
    const [filterTargetLang, setFilterTargetLang] = useState('');
    const [filterSearch, setFilterSearch] = useState('');
    const [filterOnlyActive, setFilterOnlyActive] = useState(false);

    // Add / Edit modal
    const { isOpen: isFormOpen, onOpen: onFormOpen, onOpenChange: onFormOpenChange } = useDisclosure();
    const [editingId, setEditingId] = useState(null); // null = add mode
    const [form, setForm] = useState({ ...EMPTY_FORM });

    // Delete confirmation modal
    const { isOpen: isDeleteOpen, onOpen: onDeleteOpen, onOpenChange: onDeleteOpenChange } = useDisclosure();
    const [deletingId, setDeletingId] = useState(null);

    // ── Data loading ─────────────────────────────────────────────────
    const loadEntries = useCallback(async () => {
        setLoading(true);
        try {
            const filter = {};
            if (filterSourceLang) filter.source_lang = filterSourceLang;
            if (filterTargetLang) filter.target_lang = filterTargetLang;
            if (filterSearch.trim()) filter.search = filterSearch.trim();
            if (filterOnlyActive) filter.only_active = true;
            const hasFilter = Object.keys(filter).length > 0;
            const result = await invoke('list_glossaries', {
                filter: hasFilter ? filter : null,
            });
            setEntries(Array.isArray(result) ? result : []);
        } catch (e) {
            toast.error(String(e), { style: toastStyle });
        } finally {
            setLoading(false);
        }
    }, [filterSourceLang, filterTargetLang, filterSearch, filterOnlyActive]);

    useEffect(() => {
        loadEntries();
    }, [loadEntries]);

    // ── Add / Edit ──────────────────────────────────────────────────
    const openAddModal = () => {
        setEditingId(null);
        setForm({ ...EMPTY_FORM });
        onFormOpen();
    };

    const openEditModal = (entry) => {
        setEditingId(entry.id);
        setForm({
            source_term: entry.source_term,
            target_term: entry.target_term,
            source_lang: entry.source_lang,
            target_lang: entry.target_lang,
            scope: entry.scope ?? '',
            case_sensitive: !!entry.case_sensitive,
            active: !!entry.active,
            notes: entry.notes ?? '',
        });
        onFormOpen();
    };

    const handleSave = async (onClose) => {
        const payload = {
            source_term: form.source_term,
            target_term: form.target_term,
            source_lang: form.source_lang,
            target_lang: form.target_lang,
            scope: form.scope.trim() || null,
            case_sensitive: form.case_sensitive,
            active: form.active,
            notes: form.notes.trim() || null,
        };
        try {
            if (editingId !== null) {
                await invoke('update_glossary_entry', { id: editingId, entry: payload });
                toast.success(t('config.glossary.update_success'), { style: toastStyle });
            } else {
                await invoke('add_glossary_entry', { entry: payload });
                toast.success(t('config.glossary.add_success'), { style: toastStyle });
            }
            onClose();
            await loadEntries();
        } catch (e) {
            toast.error(String(e), { style: toastStyle });
        }
    };

    // ── Delete ───────────────────────────────────────────────────────
    const confirmDelete = (id) => {
        setDeletingId(id);
        onDeleteOpen();
    };

    const handleDelete = async (onClose) => {
        try {
            await invoke('delete_glossary_entry', { id: deletingId });
            toast.success(t('config.glossary.delete_success'), { style: toastStyle });
            onClose();
            await loadEntries();
        } catch (e) {
            toast.error(String(e), { style: toastStyle });
        }
    };

    // ── Import / Export ──────────────────────────────────────────────
    const handleExport = async () => {
        try {
            const all = await invoke('list_glossaries', { filter: null });
            const filePath = await save({
                filters: [{ name: 'JSON', extensions: ['json'] }],
                defaultPath: 'glossary_export.json',
            });
            if (!filePath) return;
            await writeTextFile(filePath, JSON.stringify(all, null, 2));
            toast.success(t('config.glossary.export_success'), { style: toastStyle });
        } catch (e) {
            toast.error(String(e), { style: toastStyle });
        }
    };

    const handleImport = async () => {
        try {
            const filePath = await open({
                filters: [{ name: 'JSON', extensions: ['json'] }],
                multiple: false,
            });
            if (!filePath) return;
            const text = await readTextFile(filePath);
            const data = JSON.parse(text);
            if (!Array.isArray(data)) throw new Error('JSON must be an array');
            let imported = 0;
            for (const item of data) {
                const entry = {
                    source_term: String(item.source_term ?? ''),
                    target_term: String(item.target_term ?? ''),
                    source_lang: String(item.source_lang ?? 'en'),
                    target_lang: String(item.target_lang ?? 'tr'),
                    scope: item.scope ? String(item.scope) : null,
                    case_sensitive: !!item.case_sensitive,
                    active: item.active !== false,
                    notes: item.notes ? String(item.notes) : null,
                };
                try {
                    await invoke('add_glossary_entry', { entry });
                    imported++;
                } catch (_) {
                    // skip invalid entries silently
                }
            }
            toast.success(
                t('config.glossary.import_success', { count: imported }),
                { style: toastStyle }
            );
            await loadEntries();
        } catch (e) {
            toast.error(t('config.glossary.import_failed') + ': ' + String(e), {
                style: toastStyle,
            });
        }
    };

    // ── Toggle active inline ─────────────────────────────────────────
    const toggleActive = async (entry) => {
        try {
            const payload = {
                source_term: entry.source_term,
                target_term: entry.target_term,
                source_lang: entry.source_lang,
                target_lang: entry.target_lang,
                scope: entry.scope ?? null,
                case_sensitive: !!entry.case_sensitive,
                active: !entry.active,
                notes: entry.notes ?? null,
            };
            await invoke('update_glossary_entry', { id: entry.id, entry: payload });
            await loadEntries();
        } catch (e) {
            toast.error(String(e), { style: toastStyle });
        }
    };

    // ── Render ────────────────────────────────────────────────────────
    return (
        <>
            <Toaster />

            {/* ── Toolbar ── */}
            <div className='flex gap-2 mb-[10px] flex-wrap'>
                <Input
                    size='sm'
                    variant='bordered'
                    placeholder={t('config.glossary.search_placeholder')}
                    value={filterSearch}
                    onValueChange={setFilterSearch}
                    className='max-w-[200px]'
                    isClearable
                    onClear={() => setFilterSearch('')}
                />
                <Select
                    size='sm'
                    variant='bordered'
                    placeholder={t('config.glossary.source_lang')}
                    selectedKeys={filterSourceLang ? [filterSourceLang] : []}
                    onSelectionChange={(keys) => setFilterSourceLang([...keys][0] ?? '')}
                    className='max-w-[140px]'
                >
                    {LANG_OPTIONS.map((l) => (
                        <SelectItem key={l.value} value={l.value}>
                            {l.label}
                        </SelectItem>
                    ))}
                </Select>
                <Select
                    size='sm'
                    variant='bordered'
                    placeholder={t('config.glossary.target_lang')}
                    selectedKeys={filterTargetLang ? [filterTargetLang] : []}
                    onSelectionChange={(keys) => setFilterTargetLang([...keys][0] ?? '')}
                    className='max-w-[140px]'
                >
                    {LANG_OPTIONS.map((l) => (
                        <SelectItem key={l.value} value={l.value}>
                            {l.label}
                        </SelectItem>
                    ))}
                </Select>
                <Switch
                    size='sm'
                    isSelected={filterOnlyActive}
                    onValueChange={setFilterOnlyActive}
                >
                    {t('config.glossary.only_active')}
                </Switch>
                <div className='flex-grow' />
                <Button size='sm' color='primary' onPress={openAddModal}>
                    {t('config.glossary.add')}
                </Button>
                <Button size='sm' variant='flat' onPress={handleImport}>
                    {t('config.glossary.import')}
                </Button>
                <Button size='sm' variant='flat' onPress={handleExport}>
                    {t('config.glossary.export')}
                </Button>
            </div>

            {/* ── Table ── */}
            <Table
                fullWidth
                aria-label='Glossary Table'
                selectionMode='none'
                classNames={{
                    base: `${
                        osType === 'Linux' ? 'h-[calc(100vh-145px)]' : 'h-[calc(100vh-115px)]'
                    } overflow-y-auto`,
                }}
            >
                <TableHeader>
                    <TableColumn key='source_term'>{t('config.glossary.source_term')}</TableColumn>
                    <TableColumn key='target_term'>{t('config.glossary.target_term')}</TableColumn>
                    <TableColumn key='langs'>{t('config.glossary.lang_pair')}</TableColumn>
                    <TableColumn key='scope'>{t('config.glossary.scope')}</TableColumn>
                    <TableColumn key='active'>{t('config.glossary.active')}</TableColumn>
                    <TableColumn key='actions'>{t('config.glossary.actions')}</TableColumn>
                </TableHeader>
                <TableBody
                    emptyContent={t('config.glossary.empty_state')}
                    items={entries}
                >
                    {(item) => (
                        <TableRow key={item.id}>
                            <TableCell>
                                <span className='font-medium'>{item.source_term}</span>
                                {item.case_sensitive && (
                                    <Chip size='sm' variant='flat' className='ml-1 text-[10px]'>Aa</Chip>
                                )}
                            </TableCell>
                            <TableCell>{item.target_term}</TableCell>
                            <TableCell>
                                <span className='text-xs text-default-500'>
                                    {item.source_lang} → {item.target_lang}
                                </span>
                            </TableCell>
                            <TableCell>
                                {item.scope ? (
                                    <Chip size='sm' variant='flat'>{item.scope}</Chip>
                                ) : (
                                    <span className='text-default-300'>—</span>
                                )}
                            </TableCell>
                            <TableCell>
                                <Switch
                                    size='sm'
                                    isSelected={!!item.active}
                                    onValueChange={() => toggleActive(item)}
                                />
                            </TableCell>
                            <TableCell>
                                <div className='flex gap-1'>
                                    <Button
                                        size='sm'
                                        variant='light'
                                        onPress={() => openEditModal(item)}
                                    >
                                        {t('config.glossary.edit')}
                                    </Button>
                                    <Button
                                        size='sm'
                                        variant='light'
                                        color='danger'
                                        onPress={() => confirmDelete(item.id)}
                                    >
                                        {t('config.glossary.delete')}
                                    </Button>
                                </div>
                            </TableCell>
                        </TableRow>
                    )}
                </TableBody>
            </Table>

            {/* ── Add / Edit Modal ── */}
            <Modal
                isOpen={isFormOpen}
                onOpenChange={onFormOpenChange}
                scrollBehavior='inside'
                size='lg'
            >
                <ModalContent>
                    {(onClose) => (
                        <>
                            <ModalHeader>
                                {editingId !== null
                                    ? t('config.glossary.edit_title')
                                    : t('config.glossary.add_title')}
                            </ModalHeader>
                            <ModalBody>
                                <Input
                                    label={t('config.glossary.source_term')}
                                    variant='bordered'
                                    value={form.source_term}
                                    onValueChange={(v) => setForm({ ...form, source_term: v })}
                                    isRequired
                                />
                                <Input
                                    label={t('config.glossary.target_term')}
                                    variant='bordered'
                                    value={form.target_term}
                                    onValueChange={(v) => setForm({ ...form, target_term: v })}
                                    isRequired
                                />
                                <div className='flex gap-2'>
                                    <Select
                                        label={t('config.glossary.source_lang')}
                                        variant='bordered'
                                        selectedKeys={[form.source_lang]}
                                        onSelectionChange={(keys) =>
                                            setForm({ ...form, source_lang: [...keys][0] ?? 'en' })
                                        }
                                        className='flex-1'
                                    >
                                        {LANG_OPTIONS.map((l) => (
                                            <SelectItem key={l.value} value={l.value}>
                                                {l.label}
                                            </SelectItem>
                                        ))}
                                    </Select>
                                    <Select
                                        label={t('config.glossary.target_lang')}
                                        variant='bordered'
                                        selectedKeys={[form.target_lang]}
                                        onSelectionChange={(keys) =>
                                            setForm({ ...form, target_lang: [...keys][0] ?? 'tr' })
                                        }
                                        className='flex-1'
                                    >
                                        {LANG_OPTIONS.map((l) => (
                                            <SelectItem key={l.value} value={l.value}>
                                                {l.label}
                                            </SelectItem>
                                        ))}
                                    </Select>
                                </div>
                                <Input
                                    label={t('config.glossary.scope')}
                                    variant='bordered'
                                    placeholder={t('config.glossary.scope_placeholder')}
                                    value={form.scope}
                                    onValueChange={(v) => setForm({ ...form, scope: v })}
                                />
                                <div className='flex gap-4'>
                                    <Switch
                                        isSelected={form.case_sensitive}
                                        onValueChange={(v) => setForm({ ...form, case_sensitive: v })}
                                    >
                                        {t('config.glossary.case_sensitive')}
                                    </Switch>
                                    <Switch
                                        isSelected={form.active}
                                        onValueChange={(v) => setForm({ ...form, active: v })}
                                    >
                                        {t('config.glossary.active')}
                                    </Switch>
                                </div>
                                <Textarea
                                    label={t('config.glossary.notes')}
                                    variant='bordered'
                                    value={form.notes}
                                    onValueChange={(v) => setForm({ ...form, notes: v })}
                                />
                            </ModalBody>
                            <ModalFooter>
                                <Button variant='light' onPress={onClose}>
                                    {t('common.cancel')}
                                </Button>
                                <Button color='primary' onPress={() => handleSave(onClose)}>
                                    {t('common.save')}
                                </Button>
                            </ModalFooter>
                        </>
                    )}
                </ModalContent>
            </Modal>

            {/* ── Delete Confirmation Modal ── */}
            <Modal
                isOpen={isDeleteOpen}
                onOpenChange={onDeleteOpenChange}
                size='sm'
            >
                <ModalContent>
                    {(onClose) => (
                        <>
                            <ModalHeader>{t('config.glossary.confirm_delete')}</ModalHeader>
                            <ModalBody>
                                <p>{t('config.glossary.confirm_delete_body')}</p>
                            </ModalBody>
                            <ModalFooter>
                                <Button variant='light' onPress={onClose}>
                                    {t('common.cancel')}
                                </Button>
                                <Button color='danger' onPress={() => handleDelete(onClose)}>
                                    {t('config.glossary.delete')}
                                </Button>
                            </ModalFooter>
                        </>
                    )}
                </ModalContent>
            </Modal>
        </>
    );
}

import { INSTANCE_NAME_CONFIG_KEY } from '../../../utils/service_instance';
import { Input, Button, Switch, Textarea, Select, SelectItem, Chip, Tooltip } from '@nextui-org/react';
import { MdDeleteOutline } from 'react-icons/md';
import toast, { Toaster } from 'react-hot-toast';
import { useTranslation } from 'react-i18next';
import { open } from '@tauri-apps/api/shell';
import React, { useState, useEffect, useRef } from 'react';

import { useConfig } from '../../../hooks/useConfig';
import { useToastStyle } from '../../../hooks';
import { translate } from './index';
import { Language } from './index';

export const GEMINI_API_BASE = 'https://generativelanguage.googleapis.com/v1beta/models/';

// API model IDs verified against https://ai.google.dev/gemini-api/docs/models on 2026-05-24.
// NOTE: "gemini-3-flash" (without suffix) is NOT a valid API ID — Google ships it only as
// "gemini-3-flash-preview". Default ships the most capable Stable Flash (gemini-3.5-flash).
export const GEMINI_MODEL_PRESETS = [
    { key: 'gemini-3.5-flash', label: 'Gemini 3.5 Flash', default: true },
    { key: 'gemini-3.1-flash-lite', label: 'Gemini 3.1 Flash-Lite (cheaper)' },
    { key: 'gemini-3-flash-preview', label: 'Gemini 3 Flash', preview: true },
    { key: 'gemini-3.1-pro-preview', label: 'Gemini 3.1 Pro', preview: true },
];

export const GEMINI_DEFAULT_PRESET = GEMINI_MODEL_PRESETS.find((m) => m.default).key;

const KNOWN_LEGACY_MODEL_SEGMENTS = [
    'gemini-pro',
    'gemini-1.0-pro',
    'gemini-1.5-pro',
    'gemini-1.5-flash',
    'gemini-2.0-flash',
    'gemini-2.0-pro',
    'gemini-2.5-flash',
    'gemini-2.5-pro',
    'gemini-2.5-flash-lite',
];

export function Config(props) {
    const { instanceKey, updateServiceList, onClose } = props;
    const { t } = useTranslation();
    const [serviceConfig, setServiceConfig] = useConfig(
        instanceKey,
        {
            [INSTANCE_NAME_CONFIG_KEY]: t('services.translate.geminipro.title'),
            stream: true,
            apiKey: '',
            requestPath: GEMINI_API_BASE,
            useCustomModel: false,
            presetKey: GEMINI_DEFAULT_PRESET,
            customModel: '',
            promptList: [
                {
                    role: 'user',
                    parts: [
                        {
                            text: 'You are a professional translation engine, please translate the text into a colloquial, professional, elegant and fluent content, without the style of machine translation. You must only translate the text content, never interpret it.',
                        },
                    ],
                },
                {
                    role: 'model',
                    parts: [
                        {
                            text: 'Ok, I will only translate the text content, never interpret it.',
                        },
                    ],
                },
                {
                    role: 'user',
                    parts: [
                        {
                            text: `Translate into Chinese\n"""\nhello\n"""`,
                        },
                    ],
                },
                {
                    role: 'model',
                    parts: [
                        {
                            text: '你好',
                        },
                    ],
                },
                {
                    role: 'user',
                    parts: [
                        {
                            text: `Translate into $to\n"""\n$text\n"""`,
                        },
                    ],
                },
            ],
        },
        { sync: false }
    );
    const [isLoading, setIsLoading] = useState(false);

    const toastStyle = useToastStyle();

    const migrationDoneRef = useRef(false);
    useEffect(() => {
        if (!serviceConfig || migrationDoneRef.current) return;
        const hasNewFields = typeof serviceConfig.presetKey === 'string' && serviceConfig.presetKey;

        // Case A: pre-Phase-1 config — no presetKey, requestPath ends in a legacy model name.
        if (!hasNewFields) {
            const path = serviceConfig.requestPath || '';
            const trimmed = path.endsWith('/') ? path.slice(0, -1) : path;
            const trailing = trimmed.slice(trimmed.lastIndexOf('/') + 1);
            if (KNOWN_LEGACY_MODEL_SEGMENTS.includes(trailing)) {
                migrationDoneRef.current = true;
                setServiceConfig({
                    ...serviceConfig,
                    requestPath: trimmed.slice(0, trimmed.lastIndexOf('/') + 1),
                    useCustomModel: false,
                    presetKey: GEMINI_DEFAULT_PRESET,
                    customModel: serviceConfig.customModel || '',
                });
                toast.success(
                    t('services.translate.geminipro.model_upgraded', { model: GEMINI_DEFAULT_PRESET }),
                    { style: toastStyle, duration: 5000 }
                );
                return;
            }
            migrationDoneRef.current = true;
            return;
        }

        // Case B: presetKey was set but is no longer in the preset list (we removed an entry
        // in a subsequent preset-table update). Heal by snapping to the current default so the
        // user doesn't keep hitting 404 against a model name Google no longer publishes.
        const presetKeyValid = GEMINI_MODEL_PRESETS.some((p) => p.key === serviceConfig.presetKey);
        if (!presetKeyValid && !serviceConfig.useCustomModel) {
            migrationDoneRef.current = true;
            setServiceConfig({
                ...serviceConfig,
                presetKey: GEMINI_DEFAULT_PRESET,
            });
            toast.success(
                t('services.translate.geminipro.model_upgraded', { model: GEMINI_DEFAULT_PRESET }),
                { style: toastStyle, duration: 5000 }
            );
            return;
        }

        migrationDoneRef.current = true;
    }, [serviceConfig]);

    return (
        serviceConfig !== null && (
            <form
                onSubmit={(e) => {
                    e.preventDefault();
                    setIsLoading(true);
                    translate('hello', Language.auto, Language.zh_cn, { config: serviceConfig }).then(
                        () => {
                            setIsLoading(false);
                            setServiceConfig(serviceConfig, true);
                            updateServiceList(instanceKey);
                            onClose();
                        },
                        (e) => {
                            setIsLoading(false);
                            toast.error(t('config.service.test_failed') + e.toString(), { style: toastStyle });
                        }
                    );
                }}
            >
                <Toaster />
                <div className='config-item'>
                    <Input
                        label={t('services.instance_name')}
                        labelPlacement='outside-left'
                        value={serviceConfig[INSTANCE_NAME_CONFIG_KEY]}
                        variant='bordered'
                        classNames={{
                            base: 'justify-between',
                            label: 'text-[length:--nextui-font-size-medium]',
                            mainWrapper: 'max-w-[50%]',
                        }}
                        onValueChange={(value) => {
                            setServiceConfig({
                                ...serviceConfig,
                                [INSTANCE_NAME_CONFIG_KEY]: value,
                            });
                        }}
                    />
                </div>
                <div className='config-item'>
                    <Switch
                        isSelected={serviceConfig['useCustomModel']}
                        onValueChange={(value) => {
                            setServiceConfig({
                                ...serviceConfig,
                                useCustomModel: value,
                            });
                        }}
                        classNames={{
                            base: 'flex flex-row-reverse justify-between w-full max-w-full',
                        }}
                    >
                        {t('services.translate.geminipro.custom_model')}
                    </Switch>
                </div>
                {serviceConfig['useCustomModel'] ? (
                    <div className='config-item'>
                        <Input
                            label={t('services.translate.geminipro.custom_model_label')}
                            labelPlacement='outside-left'
                            value={serviceConfig['customModel'] || ''}
                            placeholder='gemini-...'
                            variant='bordered'
                            classNames={{
                                base: 'justify-between',
                                label: 'text-[length:--nextui-font-size-medium]',
                                mainWrapper: 'max-w-[50%]',
                            }}
                            onValueChange={(value) => {
                                setServiceConfig({
                                    ...serviceConfig,
                                    customModel: value,
                                });
                            }}
                        />
                    </div>
                ) : (
                    <div className='config-item'>
                        <Select
                            label={t('services.translate.geminipro.model')}
                            labelPlacement='outside-left'
                            selectedKeys={[serviceConfig['presetKey'] || GEMINI_DEFAULT_PRESET]}
                            variant='bordered'
                            classNames={{
                                base: 'justify-between',
                                label: 'text-[length:--nextui-font-size-medium]',
                                mainWrapper: 'max-w-[50%]',
                            }}
                            onSelectionChange={(keys) => {
                                const k = Array.from(keys)[0];
                                if (!k) return;
                                setServiceConfig({
                                    ...serviceConfig,
                                    presetKey: k,
                                });
                            }}
                        >
                            {GEMINI_MODEL_PRESETS.map((m) => (
                                <SelectItem
                                    key={m.key}
                                    textValue={m.label}
                                >
                                    <div className='flex items-center gap-2'>
                                        <span>{m.label}</span>
                                        {m.preview && (
                                            <Tooltip content={t('services.translate.geminipro.preview_tooltip')}>
                                                <Chip
                                                    color='warning'
                                                    size='sm'
                                                >
                                                    Preview
                                                </Chip>
                                            </Tooltip>
                                        )}
                                    </div>
                                </SelectItem>
                            ))}
                        </Select>
                    </div>
                )}
                <div className='config-item'>
                    <h3 className='my-auto'>{t('services.help')}</h3>
                    <Button
                        onPress={() => {
                            open('https://pot-app.com/docs/api/translate/geminipro.html');
                        }}
                    >
                        {t('services.help')}
                    </Button>
                </div>
                <div className='config-item'>
                    <Switch
                        isSelected={serviceConfig['stream']}
                        onValueChange={(value) => {
                            setServiceConfig({
                                ...serviceConfig,
                                stream: value,
                            });
                        }}
                        classNames={{
                            base: 'flex flex-row-reverse justify-between w-full max-w-full',
                        }}
                    >
                        {t('services.translate.geminipro.stream')}
                    </Switch>
                </div>
                <div className='config-item'>
                    <Input
                        label={t('services.translate.geminipro.request_path')}
                        labelPlacement='outside-left'
                        value={serviceConfig['requestPath']}
                        variant='bordered'
                        classNames={{
                            base: 'justify-between',
                            label: 'text-[length:--nextui-font-size-medium]',
                            mainWrapper: 'max-w-[50%]',
                        }}
                        onValueChange={(value) => {
                            setServiceConfig({
                                ...serviceConfig,
                                requestPath: value,
                            });
                        }}
                    />
                </div>
                <div className='config-item'>
                    <Input
                        label={t('services.translate.geminipro.api_key')}
                        labelPlacement='outside-left'
                        type='password'
                        value={serviceConfig['apiKey']}
                        variant='bordered'
                        classNames={{
                            base: 'justify-between',
                            label: 'text-[length:--nextui-font-size-medium]',
                            mainWrapper: 'max-w-[50%]',
                        }}
                        onValueChange={(value) => {
                            setServiceConfig({
                                ...serviceConfig,
                                apiKey: value,
                            });
                        }}
                    />
                </div>
                <h3 className='my-auto'>Prompt List</h3>
                <p className='text-[10px] text-default-700'>{t('services.translate.geminipro.prompt_description')}</p>

                <div className='bg-content2 rounded-[10px] p-3'>
                    {serviceConfig.promptList &&
                        serviceConfig.promptList.map((prompt, index) => {
                            return (
                                <div className='config-item'>
                                    <Textarea
                                        label={prompt.role}
                                        labelPlacement='outside'
                                        variant='faded'
                                        value={prompt.parts[0].text}
                                        placeholder={`Input Some ${prompt.role} Prompt`}
                                        onValueChange={(value) => {
                                            setServiceConfig({
                                                ...serviceConfig,
                                                promptList: serviceConfig.promptList.map((p, i) => {
                                                    if (i === index) {
                                                        return {
                                                            role: index % 2 !== 0 ? 'model' : 'user',
                                                            parts: [
                                                                {
                                                                    text: value,
                                                                },
                                                            ],
                                                        };
                                                    } else {
                                                        return p;
                                                    }
                                                }),
                                            });
                                        }}
                                    />
                                    <Button
                                        isIconOnly
                                        color='danger'
                                        className='my-auto mx-1'
                                        variant='flat'
                                        onPress={() => {
                                            setServiceConfig({
                                                ...serviceConfig,
                                                promptList: serviceConfig.promptList.filter((p, i) => i !== index),
                                            });
                                        }}
                                    >
                                        <MdDeleteOutline className='text-[18px]' />
                                    </Button>
                                </div>
                            );
                        })}
                    <Button
                        fullWidth
                        onPress={() => {
                            setServiceConfig({
                                ...serviceConfig,
                                promptList: [
                                    ...serviceConfig.promptList,
                                    {
                                        role: serviceConfig.promptList.length % 2 === 0 ? 'user' : 'model',
                                        parts: [
                                            {
                                                text: '',
                                            },
                                        ],
                                    },
                                ],
                            });
                        }}
                    >
                        {t('services.translate.geminipro.add')}
                    </Button>
                </div>
                <br />
                <Button
                    type='submit'
                    isLoading={isLoading}
                    fullWidth
                    color='primary'
                >
                    {t('common.save')}
                </Button>
            </form>
        )
    );
}

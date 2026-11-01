import { useCallback } from 'react';
let audioContext = null;
let source = null;

export const useVoice = () => {
    const playOrStop = useCallback(async (data) => {
        audioContext ??= new (window.AudioContext || window.webkitAudioContext)();
        await audioContext.resume();
        if (source) {
            // 如果正在播放，停止播放
            source.stop();
            source.disconnect();
            source = null;
        } else {
            // 如果没在播放，开始播放
            const buffer = await audioContext.decodeAudioData(new Uint8Array(data).buffer);
                const playing = audioContext.createBufferSource();
                source = playing;
                playing.buffer = buffer;
                playing.connect(audioContext.destination);
                playing.start();
                playing.onended = () => {
                    playing.disconnect();
                    if (source === playing) source = null;
                };
        }
    }, []);

    return playOrStop;
};

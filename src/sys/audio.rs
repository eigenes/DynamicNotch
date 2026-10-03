//! Microphone capture via WASAPI (shared mode, default recording device).
//!
//! The audio engine converts to 16 kHz mono 16-bit PCM for us
//! (AUTOCONVERTPCM), which is exactly what speech-to-text wants, and wakes
//! the capture thread through an event — no polling.

use std::sync::atomic::{AtomicBool, Ordering};

use windows::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{
    eCapture, eConsole, IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator, MMDeviceEnumerator,
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
    AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, WAVEFORMATEX, WAVE_FORMAT_PCM,
};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

pub const SAMPLE_RATE: u32 = 16_000;
pub const MUTED: &str = "Your microphone is muted in Windows";
/// Loudness is reported this often (samples).
const LEVEL_WINDOW: usize = SAMPLE_RATE as usize / 25;

/// Record until `stop` is set or `max_secs` is reached. `on_level` gets a
/// 0..1 loudness about 25 times per second. COM must be initialised on the
/// calling thread.
pub fn capture(stop: &AtomicBool, max_secs: u32, mut on_level: impl FnMut(f32)) -> Result<Vec<i16>, String> {
    let max = (max_secs * SAMPLE_RATE) as usize;
    let mut out: Vec<i16> = Vec::with_capacity(SAMPLE_RATE as usize * 30);
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).map_err(|e| format!("audio: {e}"))?;
        let device =
            enumerator.GetDefaultAudioEndpoint(eCapture, eConsole).map_err(|_| "No microphone found".to_string())?;
        // a muted endpoint records pure silence; say so instead
        if let Ok(vol) = device.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None) {
            if vol.GetMute().map_or(false, |m| m.as_bool()) {
                return Err(MUTED.to_string());
            }
        }
        let client: IAudioClient = device.Activate(CLSCTX_ALL, None).map_err(|e| format!("microphone: {e}"))?;
        let fmt = WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_PCM as u16,
            nChannels: 1,
            nSamplesPerSec: SAMPLE_RATE,
            nAvgBytesPerSec: SAMPLE_RATE * 2,
            nBlockAlign: 2,
            wBitsPerSample: 16,
            cbSize: 0,
        };
        let flags = AUDCLNT_STREAMFLAGS_EVENTCALLBACK
            | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
            | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
        client
            .Initialize(AUDCLNT_SHAREMODE_SHARED, flags, 2_000_000, 0, &fmt, None)
            .map_err(|e| format!("microphone unavailable: {e}"))?;
        let event = CreateEventW(None, false, false, None).map_err(|e| e.to_string())?;
        let result = (|| -> Result<(), String> {
            client.SetEventHandle(event).map_err(|e| e.to_string())?;
            let cap: IAudioCaptureClient = client.GetService().map_err(|e| e.to_string())?;
            client.Start().map_err(|e| format!("microphone: {e}"))?;
            let mut window_start = 0usize;
            while !stop.load(Ordering::Relaxed) && out.len() < max {
                if WaitForSingleObject(event, 200) != WAIT_OBJECT_0 {
                    continue;
                }
                loop {
                    let n = cap.GetNextPacketSize().map_err(|e| e.to_string())?;
                    if n == 0 {
                        break;
                    }
                    let mut data: *mut u8 = std::ptr::null_mut();
                    let (mut frames, mut bflags) = (0u32, 0u32);
                    cap.GetBuffer(&mut data, &mut frames, &mut bflags, None, None).map_err(|e| e.to_string())?;
                    if bflags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null() {
                        out.extend(std::iter::repeat(0).take(frames as usize));
                    } else {
                        out.extend_from_slice(std::slice::from_raw_parts(data as *const i16, frames as usize));
                    }
                    cap.ReleaseBuffer(frames).map_err(|e| e.to_string())?;
                }
                while out.len() - window_start >= LEVEL_WINDOW {
                    on_level(level(&out[window_start..window_start + LEVEL_WINDOW]));
                    window_start += LEVEL_WINDOW;
                }
            }
            let _ = client.Stop();
            Ok(())
        })();
        let _ = CloseHandle(event);
        result?;
    }
    out.truncate(max);
    Ok(out)
}

/// Root mean square of 16-bit samples, 0..1.
pub fn rms(s: &[i16]) -> f32 {
    if s.is_empty() {
        return 0.0;
    }
    let sum: f64 = s.iter().map(|&v| (v as f64 / 32768.0).powi(2)).sum();
    (sum / s.len() as f64).sqrt() as f32
}

/// Perceptual 0..1 level: -50 dBFS (room noise) .. -12 dBFS (loud speech).
pub fn level(s: &[i16]) -> f32 {
    let db = 20.0 * rms(s).max(1e-6).log10();
    ((db + 50.0) / 38.0).clamp(0.0, 1.0)
}

/// 16-bit mono PCM wrapped in a RIFF/WAVE header.
pub fn wav(samples: &[i16]) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut w = Vec::with_capacity(44 + data_len as usize);
    w.extend(b"RIFF");
    w.extend((36 + data_len).to_le_bytes());
    w.extend(b"WAVEfmt ");
    w.extend(16u32.to_le_bytes());
    w.extend(1u16.to_le_bytes()); // PCM
    w.extend(1u16.to_le_bytes()); // mono
    w.extend(SAMPLE_RATE.to_le_bytes());
    w.extend((SAMPLE_RATE * 2).to_le_bytes());
    w.extend(2u16.to_le_bytes());
    w.extend(16u16.to_le_bytes());
    w.extend(b"data");
    w.extend(data_len.to_le_bytes());
    for s in samples {
        w.extend(s.to_le_bytes());
    }
    w
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_header_and_levels() {
        let w = wav(&[0, 1, -1]);
        assert_eq!(w.len(), 44 + 6);
        assert_eq!(&w[0..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(w[40..44].try_into().unwrap()), 6);
        assert_eq!(level(&[0; 100]), 0.0);
        assert!(level(&[16_000, -16_000]) > 0.9);
        assert!((rms(&[16384, -16384]) - 0.5).abs() < 1e-3);
    }
}

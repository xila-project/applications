//! Alarm and timer tone through the generic `/devices/audio/playback` device.
//!
//! [`AlarmTone`] is a stateless, allocation-free pattern generator: two short
//! 880 Hz beeps per second, as interleaved little-endian stereo F32 PCM at the
//! device's fixed 48 kHz rate. Sample `n` depends only on `n`, so a short write
//! needs no retained block: the next call resumes at the first rejected frame.
//!
//! On `wasm32`, [`AlarmSound`] owns the playback descriptor while an alert rings.
//! Call [`AlarmSound::update`] on every event-loop turn and, while
//! [`AlarmSound::is_active`] is true, wait only a few milliseconds between turns:
//! the device queue holds a few milliseconds of PCM and never waits for a writer.

/// Device sample rate; the audio service mixes at a fixed 48 kHz.
pub const SAMPLE_RATE: u32 = 48_000;
pub const CHANNELS: u8 = 2;
/// F32 in the `device::audio::SampleFormat` numbering.
pub const FORMAT_F32: u8 = 4;
pub const FRAME_BYTES: usize = CHANNELS as usize * size_of::<f32>();

const FREQUENCY: u64 = 880;
const PERIOD_FRAMES: u64 = SAMPLE_RATE as u64;
const BEEP_FRAMES: u64 = SAMPLE_RATE as u64 * 150 / 1000;
const SECOND_BEEP_START: u64 = SAMPLE_RATE as u64 * 250 / 1000;
const RAMP_FRAMES: u64 = SAMPLE_RATE as u64 * 5 / 1000;
const AMPLITUDE: f32 = 0.5;

const fn control_identifier(direction: u32, payload_size: u32, number: u8) -> u32 {
    direction | (payload_size << 16) | ((b'A' as u32) << 8) | number as u32
}

pub const GET_CONFIGURATION: u32 = control_identifier(1 << 30, 8, 0x02);
pub const GET_WRITABLE: u32 = control_identifier(1 << 30, 4, 0x03);
pub const SET_VOLUME: u32 = control_identifier(2 << 30, 1, 0x04);
pub const START: u32 = control_identifier(2 << 30, 0, 0x06);
pub const STOP: u32 = control_identifier(2 << 30, 0, 0x07);

/// The device's eight-byte `Configuration` payload.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioConfiguration {
    pub sample_rate: u32,
    pub channels: u8,
    pub format: u8,
    pub reserved: [u8; 2],
}

impl AudioConfiguration {
    /// Whether the tone can be written to this stream without conversion.
    pub fn is_supported(&self) -> bool {
        self.sample_rate == SAMPLE_RATE
            && self.channels == CHANNELS
            && self.format == FORMAT_F32
            && self.reserved == [0; 2]
    }
}

/// Position in the repeating two-beep pattern.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct AlarmTone {
    frame: u64,
}

impl AlarmTone {
    /// Restarts at the first beep.
    pub fn reset(&mut self) {
        self.frame = 0;
    }

    /// Frames rendered and consumed since the last reset.
    pub fn position(&self) -> u64 {
        self.frame
    }

    /// One mono sample of the pattern, in -1.0..=1.0.
    pub fn sample(frame: u64) -> f32 {
        let within = frame % PERIOD_FRAMES;
        let beep = if within < BEEP_FRAMES {
            within
        } else if (SECOND_BEEP_START..SECOND_BEEP_START + BEEP_FRAMES).contains(&within) {
            within - SECOND_BEEP_START
        } else {
            return 0.0;
        };
        // Short linear ramps avoid clicks at each beep edge.
        let ramp = beep.min(BEEP_FRAMES - 1 - beep).min(RAMP_FRAMES) as f32 / RAMP_FRAMES as f32;
        // Exact integer phase: no drift, no trigonometry in `no_std`.
        let phase = (frame * FREQUENCY % SAMPLE_RATE as u64) as f32 / SAMPLE_RATE as f32;
        // Parabolic sine approximation; its harmonics are below 1%.
        let wave = if phase < 0.5 {
            16.0 * phase * (0.5 - phase)
        } else {
            -16.0 * (phase - 0.5) * (1.0 - phase)
        };
        wave * ramp * AMPLITUDE
    }

    /// Renders whole frames from the current position into `output` without
    /// consuming them. Returns the number of bytes written.
    pub fn render(&self, output: &mut [u8]) -> usize {
        let (frames, _) = output.as_chunks_mut::<FRAME_BYTES>();
        for (index, frame) in frames.iter_mut().enumerate() {
            let sample = Self::sample(self.frame + index as u64).to_le_bytes();
            frame[..4].copy_from_slice(&sample);
            frame[4..].copy_from_slice(&sample);
        }
        frames.len() * FRAME_BYTES
    }

    /// Marks `bytes` of rendered PCM as accepted by the device.
    pub fn consume(&mut self, bytes: usize) {
        self.frame += (bytes / FRAME_BYTES) as u64;
    }
}

#[cfg(target_arch = "wasm32")]
pub use device::AlarmSound;

#[cfg(target_arch = "wasm32")]
mod device {
    use super::{
        AlarmTone, AudioConfiguration, FRAME_BYTES, GET_CONFIGURATION, GET_WRITABLE, SET_VOLUME,
        START, STOP,
    };
    use std::{
        fs::{File, OpenOptions},
        io::{self, Write},
        os::fd::AsRawFd,
    };

    /// Bytes rendered per write: 128 frames, the mixer's block size.
    const CHUNK_BYTES: usize = 128 * FRAME_BYTES;
    /// Bounds work per turn even if the device reports a large queue.
    const MAX_CHUNKS_PER_UPDATE: usize = 16;
    const VOLUME: u8 = 80;

    #[repr(align(16))]
    struct Aligned<T>(T);

    fn control<I, O>(
        file: &File,
        command: u32,
        input: I,
        output: &mut Aligned<O>,
    ) -> io::Result<()> {
        let input = Aligned(input);
        let result = unsafe {
            wasm::fd_control(
                file.as_raw_fd(),
                command,
                core::ptr::from_ref(&input).cast(),
                size_of::<I>() as u32,
                core::ptr::from_mut(output).cast(),
                size_of::<O>() as u32,
            )
        };
        if result == 0 {
            Ok(())
        } else {
            Err(io::Error::from_raw_os_error(result))
        }
    }

    fn open() -> io::Result<File> {
        let file = OpenOptions::new()
            .write(true)
            .open("/devices/audio/playback")?;
        let mut configuration = Aligned(AudioConfiguration {
            sample_rate: 0,
            channels: 0,
            format: 0,
            reserved: [0; 2],
        });
        control(&file, GET_CONFIGURATION, (), &mut configuration)?;
        if !configuration.0.is_supported() {
            return Err(io::ErrorKind::Unsupported.into());
        }
        control(&file, SET_VOLUME, VOLUME, &mut Aligned(()))?;
        control(&file, START, (), &mut Aligned(()))?;
        Ok(file)
    }

    /// Plays [`AlarmTone`] while an alert rings; silent when no device exists.
    #[derive(Default)]
    pub struct AlarmSound {
        file: Option<File>,
        tone: AlarmTone,
        /// Opening failed for the current alert; retried at the next alert.
        unavailable: bool,
    }

    impl AlarmSound {
        /// Opens, feeds or releases the playback stream. Never waits.
        pub fn update(&mut self, ringing: bool) {
            if !ringing {
                self.release();
                self.unavailable = false;
                return;
            }
            if self.file.is_none() && !self.unavailable {
                match open() {
                    Ok(file) => {
                        self.tone.reset();
                        self.file = Some(file);
                    }
                    Err(_) => self.unavailable = true,
                }
            }
            if self.pump().is_err() {
                // The stream failed; stay silent until the next alert.
                self.release();
                self.unavailable = true;
            }
        }

        /// True while the stream must be fed every few milliseconds.
        pub fn is_active(&self) -> bool {
            self.file.is_some()
        }

        fn pump(&mut self) -> io::Result<()> {
            let Some(file) = &mut self.file else {
                return Ok(());
            };
            let mut chunk = [0u8; CHUNK_BYTES];
            for _ in 0..MAX_CHUNKS_PER_UPDATE {
                let mut writable = Aligned(0u32);
                match control(file, GET_WRITABLE, (), &mut writable) {
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
                    result => result?,
                }
                let bytes = (writable.0 as usize).min(CHUNK_BYTES) / FRAME_BYTES * FRAME_BYTES;
                if bytes == 0 {
                    return Ok(());
                }
                let rendered = self.tone.render(&mut chunk[..bytes]);
                let written = match file.write(&chunk[..rendered]) {
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
                    result => result?,
                };
                if written % FRAME_BYTES != 0 || written > rendered {
                    return Err(io::ErrorKind::InvalidData.into());
                }
                self.tone.consume(written);
                if written < rendered {
                    return Ok(());
                }
            }
            Ok(())
        }

        fn release(&mut self) {
            if let Some(file) = self.file.take() {
                let _ = control(&file, STOP, (), &mut Aligned(()));
            }
        }
    }

    impl Drop for AlarmSound {
        fn drop(&mut self) {
            self.release();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use audio::{
        GET_CONFIGURATION as DEVICE_GET_CONFIGURATION, GET_WRITABLE as DEVICE_GET_WRITABLE,
        SET_VOLUME as DEVICE_SET_VOLUME, START as DEVICE_START, STOP as DEVICE_STOP,
    };
    use file_system::ControlCommand;
    use std::vec::Vec;

    fn rendered(tone: &AlarmTone, frames: usize) -> Vec<f32> {
        let mut bytes = std::vec![0u8; frames * FRAME_BYTES];
        assert_eq!(tone.render(&mut bytes), bytes.len());
        bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|sample| f32::from_le_bytes(*sample))
            .collect()
    }

    fn rms(samples: &[f32]) -> f32 {
        (samples.iter().map(|sample| sample * sample).sum::<f32>() / samples.len() as f32).sqrt()
    }

    #[test]
    fn control_ids_and_configuration_match_the_audio_device_contract() {
        assert_eq!(
            GET_CONFIGURATION,
            DEVICE_GET_CONFIGURATION::IDENTIFIER.raw()
        );
        assert_eq!(GET_WRITABLE, DEVICE_GET_WRITABLE::IDENTIFIER.raw());
        assert_eq!(SET_VOLUME, DEVICE_SET_VOLUME::IDENTIFIER.raw());
        assert_eq!(START, DEVICE_START::IDENTIFIER.raw());
        assert_eq!(STOP, DEVICE_STOP::IDENTIFIER.raw());
        assert_eq!(
            size_of::<AudioConfiguration>(),
            size_of::<audio::Configuration>()
        );
        let mixer = audio::Configuration::new(SAMPLE_RATE, CHANNELS, audio::SampleFormat::F32);
        // SAFETY: both are eight-byte `repr(C)` payloads without padding.
        let payload: AudioConfiguration = unsafe { core::mem::transmute(mixer) };
        assert!(payload.is_supported());
        assert!(
            !AudioConfiguration {
                sample_rate: 44_100,
                ..payload
            }
            .is_supported()
        );
    }

    #[test]
    fn pattern_has_two_beeps_then_silence_each_second() {
        let samples = rendered(&AlarmTone::default(), SAMPLE_RATE as usize);
        let left: Vec<f32> = samples.iter().step_by(2).copied().collect();
        let window = |from_ms: usize, to_ms: usize| rms(&left[from_ms * 48..to_ms * 48]);
        assert!(window(20, 130) > 0.3, "first beep");
        assert_eq!(window(160, 240), 0.0, "gap");
        assert!(window(270, 380) > 0.3, "second beep");
        assert_eq!(window(410, 1000), 0.0, "pause");
        assert!(left.iter().all(|sample| sample.abs() <= AMPLITUDE));
        // Stereo channels are identical.
        assert!(samples.chunks(2).all(|frame| frame[0] == frame[1]));
    }

    #[test]
    fn beep_frequency_is_880_hz() {
        let samples = rendered(&AlarmTone::default(), 7_200);
        let left: Vec<f32> = samples.iter().step_by(2).copied().collect();
        // Count rising zero crossings across 100 ms inside the first beep.
        let crossings = left[240..5_040]
            .windows(2)
            .filter(|pair| pair[0] < 0.0 && pair[1] >= 0.0)
            .count();
        assert!((87..=89).contains(&crossings), "{crossings} crossings");
    }

    #[test]
    fn partial_consumption_resumes_at_the_first_rejected_frame() {
        let whole = rendered(&AlarmTone::default(), 512);
        let mut tone = AlarmTone::default();
        let mut joined = Vec::new();
        for accepted in [100, 0, 37, 256, 119] {
            let chunk = rendered(&tone, 256);
            joined.extend_from_slice(&chunk[..accepted * 2]);
            tone.consume(accepted * FRAME_BYTES);
        }
        assert_eq!(tone.position(), 512);
        assert_eq!(joined, whole);
        tone.reset();
        assert_eq!(tone.position(), 0);
    }

    #[test]
    fn render_ignores_a_trailing_partial_frame() {
        let mut bytes = [0xAAu8; FRAME_BYTES * 2 + 3];
        assert_eq!(AlarmTone::default().render(&mut bytes), FRAME_BYTES * 2);
        assert_eq!(bytes[FRAME_BYTES * 2..], [0xAA; 3]);
    }
}

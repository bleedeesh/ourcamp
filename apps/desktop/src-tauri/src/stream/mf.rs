//! H.264 through Media Foundation (ADR-0049): the one interface Windows has for NVIDIA,
//! Intel and AMD encoders alike, with a software encoder always there as a fallback.
//!
//! Hardware encoders are asynchronous: they say by events when they want a frame and
//! when one is ready. The software one simply answers calls. [`H264Encoder`] hides the
//! difference. COM objects are tied to the thread that made them, so the encoder lives
//! on the stream's own thread and is deliberately not `Send`.

use std::mem::ManuallyDrop;
use std::time::{Duration, Instant};

use windows::core::{w, Interface, GUID};
use windows::Win32::Media::MediaFoundation::{
    eAVEncCommonRateControlMode_CBR, eAVEncH264VProfile_ConstrainedBase,
    CODECAPI_AVEncCommonMeanBitRate, CODECAPI_AVEncCommonRateControlMode, CODECAPI_AVEncMPVGOPSize,
    CODECAPI_AVEncVideoForceKeyFrame, CODECAPI_AVLowLatencyMode, ICodecAPI, IMFActivate,
    IMFMediaEventGenerator, IMFMediaType, IMFSample, IMFTransform, METransformHaveOutput,
    METransformNeedInput, MFCreateMediaType, MFCreateMemoryBuffer, MFCreateSample,
    MFMediaType_Video, MFSampleExtension_CleanPoint, MFShutdown, MFStartup, MFTEnumEx,
    MFT_FRIENDLY_NAME_Attribute, MFVideoFormat_H264, MFVideoFormat_NV12,
    MFVideoInterlace_Progressive, MFSTARTUP_LITE, MFT_CATEGORY_VIDEO_ENCODER, MFT_ENUM_FLAG,
    MFT_ENUM_FLAG_ASYNCMFT, MFT_ENUM_FLAG_HARDWARE, MFT_ENUM_FLAG_SORTANDFILTER,
    MFT_ENUM_FLAG_SYNCMFT, MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, MFT_MESSAGE_NOTIFY_END_STREAMING,
    MFT_MESSAGE_NOTIFY_START_OF_STREAM, MFT_OUTPUT_DATA_BUFFER, MFT_OUTPUT_STREAM_PROVIDES_SAMPLES,
    MFT_REGISTER_TYPE_INFO, MF_EVENT_FLAG_NO_WAIT, MF_E_NO_EVENTS_AVAILABLE,
    MF_E_TRANSFORM_NEED_MORE_INPUT, MF_E_TRANSFORM_STREAM_CHANGE, MF_MT_AVG_BITRATE,
    MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE, MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE,
    MF_MT_MPEG2_PROFILE, MF_MT_MPEG_SEQUENCE_HEADER, MF_MT_PIXEL_ASPECT_RATIO, MF_MT_SUBTYPE,
    MF_TRANSFORM_ASYNC, MF_TRANSFORM_ASYNC_UNLOCK, MF_VERSION,
};
use windows::Win32::System::Com::{
    CoInitializeEx, CoTaskMemFree, CoUninitialize, COINIT_MULTITHREADED,
};
use windows::Win32::System::LibraryLoader::LoadLibraryW;
use windows::Win32::System::Variant::VARIANT;

use super::h264::with_parameter_sets;

/// A hardware encoder that does not ask for a frame within this has stopped working.
const STALL: Duration = Duration::from_secs(2);
/// A key frame this often at the latest, so a new viewer starts soon even if no request
/// reaches us; in seconds of video.
const KEY_FRAME_EVERY_S: u32 = 4;

/// What to make.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncoderSettings {
    /// Even.
    pub width: u32,
    /// Even.
    pub height: u32,
    pub fps: u32,
    pub bitrate: u32,
}

/// Which encoders to consider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncoderChoice {
    /// A hardware one if any works, else software.
    Best,
    /// The software one (tests: machines without a video card have only this).
    #[cfg_attr(not(test), allow(dead_code))]
    Software,
}

/// One encoded frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Encoded {
    /// Annex B; a key frame carries its SPS and PPS.
    pub data: Vec<u8>,
    pub key: bool,
    /// As given to [`H264Encoder::encode`], microseconds.
    pub timestamp_us: i64,
}

/// Whether this Windows has Media Foundation at all: editions without media ("N") and
/// servers may not. It is linked lazily (`build.rs`), so the app starts either way, and
/// this must be asked before the first call into it.
pub fn media_foundation_available() -> bool {
    // SAFETY: loading a system library by name, kept loaded for the process.
    unsafe { LoadLibraryW(w!("mfplat.dll")) }.is_ok()
}

/// COM and Media Foundation for this thread, for as long as the encoder lives.
struct Runtime;

impl Runtime {
    fn start() -> Result<Self, String> {
        if !media_foundation_available() {
            return Err("в этой версии Windows нет Media Foundation —                         установите Media Feature Pack в «Дополнительных компонентах»"
                .to_owned());
        }
        // SAFETY: paired with `CoUninitialize` and `MFShutdown` in `drop`, on this thread:
        // the encoder holding this is not `Send`.
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED)
                .ok()
                .map_err(|e| format!("COM: {e}"))?;
            MFStartup(MF_VERSION, MFSTARTUP_LITE).map_err(|e| {
                CoUninitialize();
                format!("Media Foundation: {e}")
            })?;
        }
        Ok(Self)
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        // SAFETY: see `start`.
        unsafe {
            let _ = MFShutdown();
            CoUninitialize();
        }
    }
}

/// H.264 frames from NV12 frames.
pub struct H264Encoder {
    transform: IMFTransform,
    codec: Option<ICodecAPI>,
    /// `Some` for an asynchronous (hardware) encoder.
    events: Option<IMFMediaEventGenerator>,
    /// Frames an asynchronous encoder has asked for and not yet got.
    wanted: u32,
    provides_samples: bool,
    output_size: u32,
    /// SPS and PPS in Annex B, from the output type: for key frames without them.
    parameter_sets: Vec<u8>,
    name: String,
    settings: EncoderSettings,
    /// Fields of `Self` drop in order: the transform goes before the runtime.
    _runtime: Runtime,
}

/// Packs two numbers the way Media Foundation attributes keep sizes and rates.
const fn pair(high: u32, low: u32) -> u64 {
    ((high as u64) << 32) | low as u64
}

fn media_type(subtype: &GUID, s: EncoderSettings) -> windows::core::Result<IMFMediaType> {
    // SAFETY: plain attribute calls on an object we just made.
    unsafe {
        let t = MFCreateMediaType()?;
        t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        t.SetGUID(&MF_MT_SUBTYPE, subtype)?;
        t.SetUINT64(&MF_MT_FRAME_SIZE, pair(s.width, s.height))?;
        t.SetUINT64(&MF_MT_FRAME_RATE, pair(s.fps, 1))?;
        t.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pair(1, 1))?;
        t.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
        Ok(t)
    }
}

/// The encoders of a kind, best first.
fn encoders(flags: MFT_ENUM_FLAG) -> Vec<IMFActivate> {
    let output = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: MFVideoFormat_H264,
    };
    let mut list = std::ptr::null_mut();
    let mut count = 0u32;
    // SAFETY: on success `list` holds `count` activates, which we take over one by one
    // and whose array we free with the allocator the call documents.
    unsafe {
        if MFTEnumEx(
            MFT_CATEGORY_VIDEO_ENCODER,
            flags,
            None,
            Some(&raw const output),
            &raw mut list,
            &raw mut count,
        )
        .is_err()
            || list.is_null()
        {
            return Vec::new();
        }
        let found = (0..count as usize)
            .filter_map(|i| list.add(i).read())
            .collect();
        CoTaskMemFree(Some(list.cast()));
        found
    }
}

fn friendly_name(activate: &IMFActivate) -> String {
    let mut text = windows::core::PWSTR::null();
    let mut length = 0u32;
    // SAFETY: the string is ours to free once read.
    unsafe {
        if activate
            .GetAllocatedString(&MFT_FRIENDLY_NAME_Attribute, &raw mut text, &raw mut length)
            .is_err()
        {
            return "неизвестный".to_owned();
        }
        let name = text.to_string().unwrap_or_default();
        CoTaskMemFree(Some(text.as_ptr().cast()));
        name
    }
}

impl H264Encoder {
    /// Starts the best encoder of `choice` that takes `settings`.
    ///
    /// # Errors
    /// None of them works, as text for the UI.
    pub fn new(settings: EncoderSettings, choice: EncoderChoice) -> Result<Self, String> {
        let runtime = Runtime::start()?;
        let hardware = match choice {
            EncoderChoice::Best => encoders(MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER),
            EncoderChoice::Software => Vec::new(),
        };
        let software =
            encoders(MFT_ENUM_FLAG_SYNCMFT | MFT_ENUM_FLAG_ASYNCMFT | MFT_ENUM_FLAG_SORTANDFILTER);
        let mut last = "на этом компьютере нет кодировщика H.264".to_owned();
        for activate in hardware.iter().chain(&software) {
            let name = friendly_name(activate);
            match Self::open(activate, settings) {
                Ok(parts) => return Ok(parts.into_encoder(name, settings, runtime)),
                Err(e) => {
                    eprintln!("[oc-stream] {name}: {e}");
                    last = format!("{name}: {e}");
                }
            }
        }
        Err(format!("Не удалось запустить кодировщик видео — {last}"))
    }

    fn open(activate: &IMFActivate, settings: EncoderSettings) -> windows::core::Result<Opened> {
        // SAFETY: COM calls in the order the encoder documentation asks: unlock, codec
        // settings, output type, then input type, then start streaming.
        unsafe {
            let transform: IMFTransform = activate.ActivateObject()?;
            let attributes = transform.GetAttributes()?;
            let asynchronous = attributes.GetUINT32(&MF_TRANSFORM_ASYNC).unwrap_or(0) != 0;
            if asynchronous {
                attributes.SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1)?;
            }
            let codec = transform.cast::<ICodecAPI>().ok();
            if let Some(codec) = &codec {
                set_codec_values(codec, settings);
            }

            let output = media_type(&MFVideoFormat_H264, settings)?;
            output.SetUINT32(&MF_MT_AVG_BITRATE, settings.bitrate)?;
            output.SetUINT32(
                &MF_MT_MPEG2_PROFILE,
                eAVEncH264VProfile_ConstrainedBase.0 as u32,
            )?;
            transform.SetOutputType(0, &output, 0)?;
            let input = media_type(&MFVideoFormat_NV12, settings)?;
            transform.SetInputType(0, &input, 0)?;

            let info = transform.GetOutputStreamInfo(0)?;
            let parameter_sets = sequence_header(&transform.GetOutputCurrentType(0)?);
            let events = if asynchronous {
                Some(transform.cast::<IMFMediaEventGenerator>()?)
            } else {
                None
            };
            transform.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
            transform.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;
            Ok(Opened {
                transform,
                codec,
                events,
                provides_samples: info.dwFlags & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32 != 0,
                output_size: info.cbSize.max(settings.width * settings.height * 2),
                parameter_sets,
            })
        }
    }

    /// The encoder's name, for the log.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Sends one NV12 frame (`width × height × 3 / 2` bytes) in; whatever is ready comes out.
    /// With `key`, the next frame out is a key frame.
    ///
    /// # Errors
    /// The encoder failed or stalled, as text for the log.
    pub fn encode(
        &mut self,
        nv12: &[u8],
        timestamp_us: i64,
        key: bool,
    ) -> Result<Vec<Encoded>, String> {
        if key {
            self.force_key_frame();
        }
        let sample = self
            .input_sample(nv12, timestamp_us)
            .map_err(|e| format!("кадр: {e}"))?;
        let mut out = Vec::new();
        if self.events.is_some() {
            self.feed_async(&sample, &mut out)?;
        } else {
            // SAFETY: the sample stays alive for the call.
            unsafe { self.transform.ProcessInput(0, &sample, 0) }
                .map_err(|e| format!("вход: {e}"))?;
            while let Some(frame) = self.take_output()? {
                out.push(frame);
            }
        }
        Ok(out)
    }

    /// Changes the target bitrate, as `WebRTC` asks when the network changes.
    pub fn set_bitrate(&mut self, bitrate: u32) {
        if bitrate == self.settings.bitrate {
            return;
        }
        self.settings.bitrate = bitrate;
        if let Some(codec) = &self.codec {
            // SAFETY: a value of the documented type for this setting.
            let result = unsafe {
                codec.SetValue(&CODECAPI_AVEncCommonMeanBitRate, &VARIANT::from(bitrate))
            };
            if let Err(e) = result {
                eprintln!("[oc-stream] bitrate: {e}");
            }
        }
    }

    fn force_key_frame(&self) {
        if let Some(codec) = &self.codec {
            // SAFETY: a value of the documented type for this setting.
            if let Err(e) =
                unsafe { codec.SetValue(&CODECAPI_AVEncVideoForceKeyFrame, &VARIANT::from(1u32)) }
            {
                eprintln!("[oc-stream] key frame: {e}");
            }
        }
    }

    /// Waits until the encoder wants a frame, gives it, and collects what is ready.
    fn feed_async(&mut self, sample: &IMFSample, out: &mut Vec<Encoded>) -> Result<(), String> {
        let deadline = Instant::now() + STALL;
        while self.wanted == 0 {
            self.pump_events(out)?;
            if self.wanted == 0 {
                if Instant::now() > deadline {
                    return Err("кодировщик перестал принимать кадры".to_owned());
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        // SAFETY: the sample stays alive for the call.
        unsafe { self.transform.ProcessInput(0, sample, 0) }.map_err(|e| format!("вход: {e}"))?;
        self.wanted -= 1;
        // A frame is ready within milliseconds: wait a little rather than send it a frame
        // late. Never longer than a frame's time, or capture falls behind.
        let until =
            Instant::now() + Duration::from_millis(u64::from(1000 / self.settings.fps.max(1)));
        let before = out.len();
        loop {
            self.pump_events(out)?;
            if out.len() > before || Instant::now() > until {
                return Ok(());
            }
            std::thread::sleep(Duration::from_micros(500));
        }
    }

    /// Handles every event an asynchronous encoder has queued.
    fn pump_events(&mut self, out: &mut Vec<Encoded>) -> Result<(), String> {
        let Some(events) = self.events.clone() else {
            return Ok(());
        };
        loop {
            // SAFETY: the no-wait flag makes this return at once.
            let event = match unsafe { events.GetEvent(MF_EVENT_FLAG_NO_WAIT) } {
                Ok(event) => event,
                Err(e) if e.code() == MF_E_NO_EVENTS_AVAILABLE => return Ok(()),
                Err(e) => return Err(format!("события: {e}")),
            };
            // SAFETY: reading the type of an event we own.
            let kind = unsafe { event.GetType() }.map_err(|e| format!("событие: {e}"))?;
            if kind == METransformNeedInput.0 as u32 {
                self.wanted += 1;
            } else if kind == METransformHaveOutput.0 as u32 {
                if let Some(frame) = self.take_output()? {
                    out.push(frame);
                }
            }
        }
    }

    /// Takes one frame out; `None` when the encoder has nothing yet.
    fn take_output(&mut self) -> Result<Option<Encoded>, String> {
        let own = if self.provides_samples {
            None
        } else {
            Some(output_sample(self.output_size).map_err(|e| format!("выход: {e}"))?)
        };
        let mut buffer = [MFT_OUTPUT_DATA_BUFFER {
            dwStreamID: 0,
            pSample: ManuallyDrop::new(own),
            dwStatus: 0,
            pEvents: ManuallyDrop::new(None),
        }];
        let mut status = 0u32;
        // SAFETY: `buffer` is valid for the call; afterwards we own what it points at and
        // drop both fields exactly once below.
        let result = unsafe {
            self.transform
                .ProcessOutput(0, &mut buffer, &raw mut status)
        };
        let [mut slot] = buffer;
        // SAFETY: see above.
        let (sample, _events) = unsafe {
            (
                ManuallyDrop::take(&mut slot.pSample),
                ManuallyDrop::take(&mut slot.pEvents),
            )
        };
        match result {
            Ok(()) => {}
            Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(None),
            Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                self.renegotiate_output()
                    .map_err(|e| format!("смена формата: {e}"))?;
                return Ok(None);
            }
            Err(e) => return Err(format!("выход: {e}")),
        }
        let Some(sample) = sample else {
            return Ok(None);
        };
        self.read_sample(&sample)
            .map(Some)
            .map_err(|e| format!("выход: {e}"))
    }

    fn renegotiate_output(&mut self) -> windows::core::Result<()> {
        // SAFETY: the encoder asked for this: take the first type it offers.
        unsafe {
            let offered = self.transform.GetOutputAvailableType(0, 0)?;
            self.transform.SetOutputType(0, &offered, 0)?;
            let sets = sequence_header(&offered);
            if !sets.is_empty() {
                self.parameter_sets = sets;
            }
        }
        Ok(())
    }

    fn read_sample(&self, sample: &IMFSample) -> windows::core::Result<Encoded> {
        // SAFETY: the buffer is locked for exactly the copy, and unlocked before any `?`.
        unsafe {
            let buffer = sample.ConvertToContiguousBuffer()?;
            let mut pointer = std::ptr::null_mut();
            let mut length = 0u32;
            buffer.Lock(&raw mut pointer, None, Some(&raw mut length))?;
            let data = std::slice::from_raw_parts(pointer, length as usize).to_vec();
            buffer.Unlock()?;
            let key = sample.GetUINT32(&MFSampleExtension_CleanPoint).unwrap_or(0) != 0;
            let time = sample.GetSampleTime().unwrap_or(0);
            Ok(Encoded {
                data: if key {
                    with_parameter_sets(data, &self.parameter_sets)
                } else {
                    data
                },
                key,
                timestamp_us: time / 10,
            })
        }
    }

    fn input_sample(&self, nv12: &[u8], timestamp_us: i64) -> windows::core::Result<IMFSample> {
        // SAFETY: the buffer is at least `nv12.len()` bytes, locked for the copy.
        unsafe {
            let buffer = MFCreateMemoryBuffer(nv12.len() as u32)?;
            let mut pointer = std::ptr::null_mut();
            buffer.Lock(&raw mut pointer, None, None)?;
            std::ptr::copy_nonoverlapping(nv12.as_ptr(), pointer, nv12.len());
            buffer.Unlock()?;
            buffer.SetCurrentLength(nv12.len() as u32)?;
            let sample = MFCreateSample()?;
            sample.AddBuffer(&buffer)?;
            sample.SetSampleTime(timestamp_us * 10)?;
            sample.SetSampleDuration(10_000_000 / i64::from(self.settings.fps.max(1)))?;
            Ok(sample)
        }
    }
}

impl Drop for H264Encoder {
    fn drop(&mut self) {
        // SAFETY: the transform is alive; a failure here has nobody left to tell.
        unsafe {
            let _ = self
                .transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_END_STREAMING, 0);
        }
    }
}

/// An encoder that opened, before it has its name and runtime.
struct Opened {
    transform: IMFTransform,
    codec: Option<ICodecAPI>,
    events: Option<IMFMediaEventGenerator>,
    provides_samples: bool,
    output_size: u32,
    parameter_sets: Vec<u8>,
}

impl Opened {
    fn into_encoder(
        self,
        name: String,
        settings: EncoderSettings,
        runtime: Runtime,
    ) -> H264Encoder {
        H264Encoder {
            transform: self.transform,
            codec: self.codec,
            events: self.events,
            wanted: 0,
            provides_samples: self.provides_samples,
            output_size: self.output_size,
            parameter_sets: self.parameter_sets,
            name,
            settings,
            _runtime: runtime,
        }
    }
}

/// Low latency (which also means no B frames), constant bitrate. An encoder may refuse
/// some of these; it then does its best, and the stream still works.
fn set_codec_values(codec: &ICodecAPI, s: EncoderSettings) {
    let values = [
        (CODECAPI_AVLowLatencyMode, VARIANT::from(true)),
        (
            CODECAPI_AVEncCommonRateControlMode,
            VARIANT::from(eAVEncCommonRateControlMode_CBR.0 as u32),
        ),
        (CODECAPI_AVEncCommonMeanBitRate, VARIANT::from(s.bitrate)),
        (
            CODECAPI_AVEncMPVGOPSize,
            VARIANT::from(s.fps * KEY_FRAME_EVERY_S),
        ),
    ];
    for (api, value) in &values {
        // SAFETY: values of the documented types for these settings.
        if let Err(e) = unsafe { codec.SetValue(api, value) } {
            eprintln!("[oc-stream] encoder setting {api:?}: {e}");
        }
    }
}

fn sequence_header(output: &IMFMediaType) -> Vec<u8> {
    // SAFETY: the blob is copied into a buffer of the size just asked for.
    unsafe {
        let Ok(size) = output.GetBlobSize(&MF_MT_MPEG_SEQUENCE_HEADER) else {
            return Vec::new();
        };
        let mut blob = vec![0u8; size as usize];
        if output
            .GetBlob(&MF_MT_MPEG_SEQUENCE_HEADER, &mut blob, None)
            .is_err()
        {
            return Vec::new();
        }
        blob
    }
}

fn output_sample(size: u32) -> windows::core::Result<IMFSample> {
    // SAFETY: making two objects and joining them.
    unsafe {
        let sample = MFCreateSample()?;
        sample.AddBuffer(&MFCreateMemoryBuffer(size)?)?;
        Ok(sample)
    }
}

#[cfg(test)]
mod tests {
    use super::super::h264::is_annex_b;
    use super::*;

    /// Moving NV12 stripes: something to compress, different in every frame.
    pub fn stripes(width: u32, height: u32, frame: u32) -> Vec<u8> {
        let (w, h) = (width as usize, height as usize);
        let mut nv12 = vec![128u8; w * h * 3 / 2];
        for y in 0..h {
            for x in 0..w {
                nv12[y * w + x] = (((x + frame as usize * 8) / 16) % 2 * 160 + 40) as u8;
            }
        }
        nv12
    }

    /// Windows Server images of CI may come without Media Foundation.
    fn skip_without_media_foundation() -> bool {
        let missing = !media_foundation_available();
        if missing {
            eprintln!("no Media Foundation here: nothing to test");
        }
        missing
    }

    fn encode_frames(settings: EncoderSettings, choice: EncoderChoice) -> Vec<Encoded> {
        let mut encoder = H264Encoder::new(settings, choice).unwrap();
        eprintln!("encoder: {}", encoder.name());
        let mut out = Vec::new();
        for i in 0..30u32 {
            let time = i64::from(i) * 1_000_000 / i64::from(settings.fps);
            out.extend(
                encoder
                    .encode(&stripes(settings.width, settings.height, i), time, i == 20)
                    .unwrap(),
            );
        }
        out
    }

    const SMALL: EncoderSettings = EncoderSettings {
        width: 640,
        height: 360,
        fps: 30,
        bitrate: 1_000_000,
    };

    #[test]
    fn the_software_encoder_makes_annex_b_with_key_frames_on_request() {
        if skip_without_media_foundation() {
            return;
        }
        let frames = encode_frames(SMALL, EncoderChoice::Software);
        assert!(
            frames.len() >= 25,
            "a low-latency encoder keeps up: {}",
            frames.len()
        );
        let first = &frames[0];
        assert!(first.key, "a stream starts with a key frame");
        assert!(is_annex_b(&first.data));
        assert!(
            first
                .data
                .windows(4)
                .any(|w| w[..3] == [0, 0, 1] && w[3] & 0x1f == 7),
            "with its SPS"
        );
        assert!(frames.iter().all(|f| is_annex_b(&f.data)));
        assert!(
            frames
                .windows(2)
                .all(|p| p[0].timestamp_us < p[1].timestamp_us),
            "in order: no B frames"
        );
        assert!(
            frames
                .iter()
                .skip(1)
                .any(|f| f.key && f.timestamp_us >= 20 * 1_000_000 / 30),
            "the requested key frame"
        );
    }

    /// Whatever this machine has. On a machine without a video card it is the same as above.
    #[test]
    fn the_best_encoder_works_too() {
        if skip_without_media_foundation() {
            return;
        }
        let frames = encode_frames(SMALL, EncoderChoice::Best);
        assert!(frames.len() >= 20, "{}", frames.len());
        assert!(frames[0].key && is_annex_b(&frames[0].data));
        assert!(
            frames
                .windows(2)
                .all(|p| p[0].timestamp_us < p[1].timestamp_us),
            "in order: no B frames"
        );
    }
}

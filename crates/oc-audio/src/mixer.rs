//! The output mixer: every remote voice and stream audio track ends up here.
//!
//! ```text
//! decoder thread ──SourceInput──► ring ─┐
//! decoder thread ──SourceInput──► ring ─┼─► MixerCore::process (audio thread)
//! decoder thread ──SourceInput──► ring ─┘        │ per-source gain ramp, level, speech
//!                                                ▼
//!                                   Σ ─► master gain ─► limiter ─► device
//!
//! UI thread ──MixerControl──► atomics (gain, mute) + command queue (add/remove)
//! ```
//!
//! # Real-time rules on the audio thread
//! [`MixerCore::process`] never blocks, never allocates and never frees:
//! * gains/mutes are atomics, read once per block;
//! * add/remove travel through a bounded (pre-allocated) channel drained with `try_recv`;
//! * removed sources are handed back to the control side to be dropped there;
//! * all scratch memory is allocated up front from [`MixerConfig`].

use std::collections::HashMap;
use std::fmt;
use std::hash::Hash;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TryRecvError, TrySendError};
use std::sync::Arc;

use crate::gain::{sanitize, AtomicGain, GainRamp};
use crate::limiter::Limiter;
use crate::meter::{rms_dbfs, SpeechDetector, SpeechDetectorConfig, SILENCE_DB};
use crate::ring::{ring_buffer, Consumer, Producer};

/// Channel layout of a source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// One channel (voice).
    Mono,
    /// Two interleaved channels (e.g. stream audio).
    Stereo,
}

impl Layout {
    /// Samples per frame.
    #[must_use]
    pub fn channels(self) -> usize {
        match self {
            Self::Mono => 1,
            Self::Stereo => 2,
        }
    }
}

/// Mixer tunables. All buffers are sized from these values at construction time.
#[derive(Debug, Clone)]
pub struct MixerConfig {
    /// Device sample rate (OurCamp runs everything at 48 kHz, Opus's native rate).
    pub sample_rate: u32,
    /// Output layout of the device stream.
    pub output: Layout,
    /// Largest block the device will ask for in one call; bigger requests are
    /// processed in chunks of this size.
    pub max_block_frames: usize,
    /// Maximum number of simultaneous sources.
    pub max_sources: usize,
    /// Per-source buffer capacity in milliseconds.
    pub source_buffer_ms: u32,
    /// If a source has more than this buffered, the oldest audio is dropped to keep
    /// latency bounded.
    pub max_latency_ms: u32,
    /// Length of the volume glide in milliseconds.
    pub gain_ramp_ms: f32,
    /// Limiter ceiling in dBFS.
    pub limiter_ceiling_db: f32,
    /// Limiter release time in milliseconds.
    pub limiter_release_ms: f32,
    /// Speech detection tunables.
    pub speech: SpeechDetectorConfig,
}

impl MixerConfig {
    /// Defaults for a 48 kHz stereo device.
    #[must_use]
    pub fn default_48k_stereo() -> Self {
        let sample_rate = 48_000;
        Self {
            sample_rate,
            output: Layout::Stereo,
            max_block_frames: 2048,
            max_sources: 64,
            source_buffer_ms: 500,
            max_latency_ms: 120,
            gain_ramp_ms: 15.0,
            limiter_ceiling_db: -1.0,
            limiter_release_ms: 80.0,
            speech: SpeechDetectorConfig::voice(sample_rate),
        }
    }

    fn ms_to_frames(&self, ms: u32) -> usize {
        (u64::from(self.sample_rate) * u64::from(ms) / 1000) as usize
    }
}

/// Errors returned by [`MixerControl`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MixerError {
    /// No source is registered under this key.
    UnknownSource,
    /// A source with this key is already registered.
    DuplicateSource,
    /// [`MixerConfig::max_sources`] reached.
    TooManySources,
    /// The audio thread has not drained earlier commands yet (it is stalled or not
    /// running). Retry after the device has processed a block.
    QueueFull,
}

impl fmt::Display for MixerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::UnknownSource => "unknown audio source",
            Self::DuplicateSource => "audio source already registered",
            Self::TooManySources => "too many audio sources",
            Self::QueueFull => "mixer command queue is full",
        })
    }
}

impl std::error::Error for MixerError {}

/// Live state of one source, readable from any thread.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SourceStatus {
    /// Pre-volume level of the last block in dBFS (how loud the person is, regardless
    /// of the local slider).
    pub level_db: f32,
    /// Whether the speech detector currently considers the source active.
    pub speaking: bool,
    /// Number of blocks in which the source had less audio than needed.
    pub underruns: u64,
}

/// State shared between the control side and the audio thread for one source.
#[derive(Debug)]
struct SourceShared {
    gain: AtomicGain,
    muted: AtomicBool,
    level_db: AtomicU32,
    speaking: AtomicBool,
    underruns: AtomicU64,
}

impl SourceShared {
    fn new(gain: f32) -> Self {
        Self {
            gain: AtomicGain::new(gain),
            muted: AtomicBool::new(false),
            level_db: AtomicU32::new(SILENCE_DB.to_bits()),
            speaking: AtomicBool::new(false),
            underruns: AtomicU64::new(0),
        }
    }
}

type SourceId = u32;

/// Everything the audio thread needs for one source. Allocated on the control side.
struct SourceSlot {
    id: SourceId,
    layout: Layout,
    consumer: Consumer,
    shared: Arc<SourceShared>,
    ramp: GainRamp,
    detector: SpeechDetector,
}

enum Command {
    Add(Box<SourceSlot>),
    Remove(SourceId),
    Master { gain: f32, muted: bool },
}

/// Producer end handed to whoever decodes a track (network/decoder thread).
pub struct SourceInput {
    producer: Producer,
    layout: Layout,
    dropped: u64,
}

impl SourceInput {
    /// Layout the samples must be written in.
    #[must_use]
    pub fn layout(&self) -> Layout {
        self.layout
    }

    /// Writes interleaved samples. Samples that do not fit (consumer far behind) are
    /// dropped and counted; live audio must not block the network thread.
    pub fn write(&mut self, samples: &[f32]) {
        let written = self.producer.push_slice(samples);
        self.dropped += (samples.len() - written) as u64;
    }

    /// Samples dropped because the buffer was full.
    #[must_use]
    pub fn dropped_samples(&self) -> u64 {
        self.dropped
    }
}

struct Registered {
    id: SourceId,
    shared: Arc<SourceShared>,
}

/// Control half of the mixer. Lives on the UI/engine thread.
pub struct MixerControl<K> {
    config: MixerConfig,
    sources: HashMap<K, Registered>,
    next_id: SourceId,
    commands: SyncSender<Command>,
    garbage: Receiver<Box<SourceSlot>>,
    master_gain: f32,
    master_muted: bool,
}

/// Audio-thread half of the mixer. Move it into the device callback.
pub struct MixerCore {
    channels: usize,
    max_block_frames: usize,
    max_latency_samples: [usize; 2],
    // Boxes on purpose: slots are allocated on the control thread and travel back there
    // to be freed, so the audio thread only ever moves pointers.
    #[allow(clippy::vec_box)]
    slots: Vec<Box<SourceSlot>>,
    #[allow(clippy::vec_box)]
    graveyard: Vec<Box<SourceSlot>>,
    commands: Receiver<Command>,
    garbage: SyncSender<Box<SourceSlot>>,
    scratch: Vec<f32>,
    master: GainRamp,
    limiter: Limiter,
}

/// Creates a connected mixer pair.
///
/// # Panics
/// Panics if `max_block_frames`, `max_sources` or `source_buffer_ms` is zero.
#[must_use]
pub fn mixer<K: Eq + Hash + Clone>(config: MixerConfig) -> (MixerControl<K>, MixerCore) {
    assert!(config.max_block_frames > 0, "max_block_frames must be > 0");
    assert!(config.max_sources > 0, "max_sources must be > 0");
    assert!(config.source_buffer_ms > 0, "source_buffer_ms must be > 0");

    // Every source can be added and removed once before the core drains the queue,
    // plus room for master updates.
    let (cmd_tx, cmd_rx) = sync_channel(config.max_sources * 2 + 16);
    let (garbage_tx, garbage_rx) = sync_channel(config.max_sources);
    let latency_frames = config.ms_to_frames(config.max_latency_ms);
    let ramp_len = GainRamp::len_for(config.gain_ramp_ms, config.sample_rate);

    let core = MixerCore {
        channels: config.output.channels(),
        max_block_frames: config.max_block_frames,
        max_latency_samples: [latency_frames, latency_frames * 2],
        slots: Vec::with_capacity(config.max_sources),
        graveyard: Vec::with_capacity(config.max_sources),
        commands: cmd_rx,
        garbage: garbage_tx,
        scratch: vec![0.0; config.max_block_frames * 2],
        master: GainRamp::new(1.0, ramp_len),
        limiter: Limiter::new(
            config.limiter_ceiling_db,
            config.limiter_release_ms,
            config.sample_rate,
        ),
    };
    let control = MixerControl {
        config,
        sources: HashMap::new(),
        next_id: 0,
        commands: cmd_tx,
        garbage: garbage_rx,
        master_gain: 1.0,
        master_muted: false,
    };
    (control, core)
}

impl<K: Eq + Hash + Clone> MixerControl<K> {
    /// Registers a source and returns the input its decoder should write to.
    ///
    /// # Errors
    /// [`MixerError::DuplicateSource`], [`MixerError::TooManySources`] or
    /// [`MixerError::QueueFull`].
    pub fn add_source(
        &mut self,
        key: K,
        layout: Layout,
        gain: f32,
    ) -> Result<SourceInput, MixerError> {
        self.collect_garbage();
        if self.sources.contains_key(&key) {
            return Err(MixerError::DuplicateSource);
        }
        if self.sources.len() >= self.config.max_sources {
            return Err(MixerError::TooManySources);
        }
        let frames = self.config.ms_to_frames(self.config.source_buffer_ms);
        let (producer, consumer) = ring_buffer(frames * layout.channels());
        let shared = Arc::new(SourceShared::new(gain));
        let id = self.next_id;
        let ramp_len = GainRamp::len_for(self.config.gain_ramp_ms, self.config.sample_rate);
        let slot = Box::new(SourceSlot {
            id,
            layout,
            consumer,
            shared: Arc::clone(&shared),
            ramp: GainRamp::new(sanitize(gain), ramp_len),
            detector: SpeechDetector::new(self.config.speech),
        });
        self.send(Command::Add(slot))?;
        self.next_id = self.next_id.wrapping_add(1);
        self.sources.insert(key, Registered { id, shared });
        Ok(SourceInput {
            producer,
            layout,
            dropped: 0,
        })
    }

    /// Unregisters a source. Its [`SourceInput`] keeps accepting writes, which go nowhere.
    ///
    /// # Errors
    /// [`MixerError::UnknownSource`] or [`MixerError::QueueFull`].
    pub fn remove_source(&mut self, key: &K) -> Result<(), MixerError> {
        let id = self.sources.get(key).ok_or(MixerError::UnknownSource)?.id;
        self.send(Command::Remove(id))?;
        self.sources.remove(key);
        self.collect_garbage();
        Ok(())
    }

    /// Sets a source's local volume (linear, clamped to `0.0..=MAX_GAIN`).
    ///
    /// # Errors
    /// [`MixerError::UnknownSource`].
    pub fn set_gain(&self, key: &K, gain: f32) -> Result<(), MixerError> {
        self.shared(key)?.gain.set(gain);
        Ok(())
    }

    /// Current local volume of a source.
    ///
    /// # Errors
    /// [`MixerError::UnknownSource`].
    pub fn gain(&self, key: &K) -> Result<f32, MixerError> {
        Ok(self.shared(key)?.gain.get())
    }

    /// Locally mutes or unmutes a source without forgetting its volume.
    ///
    /// # Errors
    /// [`MixerError::UnknownSource`].
    pub fn set_muted(&self, key: &K, muted: bool) -> Result<(), MixerError> {
        self.shared(key)?.muted.store(muted, Ordering::Relaxed);
        Ok(())
    }

    /// Level, speaking state and underrun count of a source.
    ///
    /// # Errors
    /// [`MixerError::UnknownSource`].
    pub fn status(&self, key: &K) -> Result<SourceStatus, MixerError> {
        let s = self.shared(key)?;
        Ok(SourceStatus {
            level_db: f32::from_bits(s.level_db.load(Ordering::Relaxed)),
            speaking: s.speaking.load(Ordering::Relaxed),
            underruns: s.underruns.load(Ordering::Relaxed),
        })
    }

    /// Keys of all registered sources.
    pub fn keys(&self) -> impl Iterator<Item = &K> {
        self.sources.keys()
    }

    /// Sets the overall output volume (linear, clamped).
    ///
    /// # Errors
    /// [`MixerError::QueueFull`].
    pub fn set_master_gain(&mut self, gain: f32) -> Result<(), MixerError> {
        let gain = sanitize(gain);
        self.send(Command::Master {
            gain,
            muted: self.master_muted,
        })?;
        self.master_gain = gain;
        Ok(())
    }

    /// Silences all output ("deafen") while keeping every per-source volume.
    ///
    /// # Errors
    /// [`MixerError::QueueFull`].
    pub fn set_master_muted(&mut self, muted: bool) -> Result<(), MixerError> {
        self.send(Command::Master {
            gain: self.master_gain,
            muted,
        })?;
        self.master_muted = muted;
        Ok(())
    }

    /// Drops source slots the audio thread has released. Called automatically by
    /// add/remove; call it periodically if sources are removed rarely.
    pub fn collect_garbage(&mut self) {
        while self.garbage.try_recv().is_ok() {}
    }

    fn shared(&self, key: &K) -> Result<&SourceShared, MixerError> {
        self.sources
            .get(key)
            .map(|r| &*r.shared)
            .ok_or(MixerError::UnknownSource)
    }

    fn send(&self, cmd: Command) -> Result<(), MixerError> {
        match self.commands.try_send(cmd) {
            Ok(()) => Ok(()),
            // A disconnected core means the device stream is gone; treat the mixer as
            // stalled rather than panicking in UI code.
            Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => {
                Err(MixerError::QueueFull)
            }
        }
    }
}

impl MixerCore {
    /// Output channel count; `out` passed to [`process`](Self::process) is interleaved
    /// with this many channels.
    #[must_use]
    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Fills `out` (interleaved) with the next block of mixed audio.
    ///
    /// Real-time safe: no locks, no allocation, no deallocation.
    ///
    /// # Panics
    /// Panics if `out.len()` is not a multiple of the channel count (a caller bug).
    pub fn process(&mut self, out: &mut [f32]) {
        assert_eq!(
            out.len() % self.channels,
            0,
            "partial frame in output buffer"
        );
        self.apply_commands();
        self.release_graveyard();
        let chunk_len = self.max_block_frames * self.channels;
        for chunk in out.chunks_mut(chunk_len) {
            self.process_chunk(chunk);
        }
    }

    fn apply_commands(&mut self) {
        loop {
            match self.commands.try_recv() {
                Ok(Command::Add(slot)) => {
                    // Capacity was reserved for max_sources and the control side enforces
                    // the limit, so this push never reallocates.
                    debug_assert!(self.slots.len() < self.slots.capacity());
                    self.slots.push(slot);
                }
                Ok(Command::Remove(id)) => {
                    if let Some(pos) = self.slots.iter().position(|s| s.id == id) {
                        let slot = self.slots.swap_remove(pos);
                        self.graveyard.push(slot);
                    }
                }
                Ok(Command::Master { gain, muted }) => {
                    self.master.set_target(if muted { 0.0 } else { gain });
                }
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
    }

    /// Hands removed slots back to the control thread so they are freed there.
    fn release_graveyard(&mut self) {
        while let Some(slot) = self.graveyard.pop() {
            if let Err(TrySendError::Full(slot) | TrySendError::Disconnected(slot)) =
                self.garbage.try_send(slot)
            {
                // Control side is not collecting right now; keep the slot and retry next block.
                self.graveyard.push(slot);
                break;
            }
        }
    }

    fn process_chunk(&mut self, out: &mut [f32]) {
        out.fill(0.0);
        let frames = out.len() / self.channels;

        for slot in &mut self.slots {
            let src_ch = slot.layout.channels();
            let needed = frames * src_ch;
            let input = &mut self.scratch[..needed];

            // Bound latency: if the decoder ran ahead, drop the oldest audio.
            let cap = self.max_latency_samples[src_ch - 1];
            let available = slot.consumer.available();
            if available > cap + needed {
                slot.consumer.skip(available - cap);
            }

            let got = slot.consumer.pop_slice(input);
            if got < needed {
                input[got..].fill(0.0);
                slot.shared.underruns.fetch_add(1, Ordering::Relaxed);
            }

            let level = rms_dbfs(input);
            slot.shared
                .level_db
                .store(level.to_bits(), Ordering::Relaxed);
            let speaking = slot.detector.update(level, frames as u32);
            slot.shared.speaking.store(speaking, Ordering::Relaxed);

            let muted = slot.shared.muted.load(Ordering::Relaxed);
            slot.ramp
                .set_target(if muted { 0.0 } else { slot.shared.gain.get() });
            if slot.ramp.is_settled() && slot.ramp.target() == 0.0 {
                continue; // silent source: skip the multiply-add entirely
            }

            mix_into(out, self.channels, input, src_ch, &mut slot.ramp);
        }

        for frame in out.chunks_mut(self.channels) {
            let g = self.master.next_gain();
            for s in frame {
                *s *= g;
            }
        }
        self.limiter.process_interleaved(out, self.channels);
    }
}

/// Adds `input` (interleaved, `in_ch` channels) to `out` (interleaved, `out_ch`
/// channels), applying the ramp per frame. Mono is spread to every output channel;
/// stereo folds down to mono by averaging.
#[inline]
fn mix_into(out: &mut [f32], out_ch: usize, input: &[f32], in_ch: usize, ramp: &mut GainRamp) {
    match (in_ch, out_ch) {
        (1, _) => {
            for (frame, &s) in out.chunks_mut(out_ch).zip(input) {
                let v = s * ramp.next_gain();
                for o in frame {
                    *o += v;
                }
            }
        }
        (2, 1) => {
            for (o, pair) in out.iter_mut().zip(input.chunks_exact(2)) {
                *o += (pair[0] + pair[1]) * 0.5 * ramp.next_gain();
            }
        }
        _ => {
            // Stereo into stereo (or wider): channel-for-channel on the first two.
            for (frame, pair) in out.chunks_mut(out_ch).zip(input.chunks_exact(2)) {
                let g = ramp.next_gain();
                frame[0] += pair[0] * g;
                frame[1] += pair[1] * g;
            }
        }
    }
}

#[cfg(test)]
// Exact float comparisons are intentional here: the expected values (silence, untouched
// samples) are produced without any arithmetic rounding.
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    fn test_config() -> MixerConfig {
        MixerConfig {
            gain_ramp_ms: 0.0, // instant gains keep assertions exact
            limiter_ceiling_db: 0.0,
            ..MixerConfig::default_48k_stereo()
        }
    }

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-5
    }

    #[test]
    fn empty_mixer_outputs_silence() {
        let (_c, mut core) = mixer::<&str>(test_config());
        let mut out = [1.0_f32; 64];
        core.process(&mut out);
        assert!(out.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn mono_source_is_spread_to_both_channels_with_gain() {
        let (mut ctl, mut core) = mixer(test_config());
        let mut input = ctl.add_source("lena", Layout::Mono, 0.5).unwrap();
        input.write(&[0.4, 0.8]);
        let mut out = [0.0_f32; 4];
        core.process(&mut out);
        assert!(approx(out[0], 0.2) && approx(out[1], 0.2));
        assert!(approx(out[2], 0.4) && approx(out[3], 0.4));
    }

    #[test]
    fn sources_are_summed_with_individual_gains() {
        let (mut ctl, mut core) = mixer(test_config());
        let mut a = ctl.add_source("artem", Layout::Mono, 1.0).unwrap();
        let mut b = ctl.add_source("lena", Layout::Mono, 2.0).unwrap();
        a.write(&[0.1; 4]);
        b.write(&[0.1; 4]);
        let mut out = [0.0_f32; 8];
        core.process(&mut out);
        assert!(out.iter().all(|&s| approx(s, 0.3)), "{out:?}");
    }

    #[test]
    fn changing_gain_affects_next_block() {
        let (mut ctl, mut core) = mixer(test_config());
        let mut a = ctl.add_source(1, Layout::Mono, 1.0).unwrap();
        core.process(&mut [0.0; 2]); // core picks up the source
        ctl.set_gain(&1, 0.25).unwrap();
        assert!(approx(ctl.gain(&1).unwrap(), 0.25));
        a.write(&[0.8]);
        let mut out = [0.0_f32; 2];
        core.process(&mut out);
        assert!(approx(out[0], 0.2));
    }

    #[test]
    fn stereo_stream_source_keeps_channels() {
        let (mut ctl, mut core) = mixer(test_config());
        let mut s = ctl.add_source("stream", Layout::Stereo, 1.0).unwrap();
        s.write(&[0.1, -0.1, 0.2, -0.2]);
        let mut out = [0.0_f32; 4];
        core.process(&mut out);
        assert_eq!(out, [0.1, -0.1, 0.2, -0.2]);
    }

    #[test]
    fn stereo_source_folds_down_into_mono_output() {
        let cfg = MixerConfig {
            output: Layout::Mono,
            ..test_config()
        };
        let (mut ctl, mut core) = mixer(cfg);
        let mut s = ctl.add_source("stream", Layout::Stereo, 1.0).unwrap();
        s.write(&[0.2, 0.4]);
        let mut out = [0.0_f32; 1];
        core.process(&mut out);
        assert!(approx(out[0], 0.3));
    }

    #[test]
    fn underrun_is_padded_with_silence_and_counted() {
        let (mut ctl, mut core) = mixer(test_config());
        let mut a = ctl.add_source("a", Layout::Mono, 1.0).unwrap();
        a.write(&[0.5]);
        let mut out = [9.0_f32; 6];
        core.process(&mut out);
        assert!(approx(out[0], 0.5));
        assert!(out[2..].iter().all(|&s| s == 0.0));
        assert_eq!(ctl.status(&"a").unwrap().underruns, 1);
    }

    #[test]
    fn local_mute_silences_without_losing_volume() {
        let (mut ctl, mut core) = mixer(test_config());
        let mut a = ctl.add_source("a", Layout::Mono, 1.5).unwrap();
        ctl.set_muted(&"a", true).unwrap();
        a.write(&[0.5; 2]);
        let mut out = [0.0_f32; 4];
        core.process(&mut out);
        assert!(out.iter().all(|&s| s == 0.0));
        assert!(approx(ctl.gain(&"a").unwrap(), 1.5));
        ctl.set_muted(&"a", false).unwrap();
        a.write(&[0.5; 2]);
        core.process(&mut out);
        assert!(approx(out[0], 0.75));
    }

    #[test]
    fn master_mute_and_gain() {
        let (mut ctl, mut core) = mixer(test_config());
        let mut a = ctl.add_source("a", Layout::Mono, 1.0).unwrap();
        ctl.set_master_gain(0.5).unwrap();
        a.write(&[0.4]);
        let mut out = [0.0_f32; 2];
        core.process(&mut out);
        assert!(approx(out[0], 0.2));
        ctl.set_master_muted(true).unwrap();
        a.write(&[0.4]);
        core.process(&mut out);
        assert_eq!(out, [0.0, 0.0]);
        ctl.set_master_muted(false).unwrap();
        a.write(&[0.4]);
        core.process(&mut out);
        assert!(approx(out[0], 0.2), "master gain survives deafen");
    }

    #[test]
    fn removed_source_stops_and_slot_is_returned() {
        let (mut ctl, mut core) = mixer(test_config());
        let mut a = ctl.add_source("a", Layout::Mono, 1.0).unwrap();
        core.process(&mut [0.0; 2]);
        ctl.remove_source(&"a").unwrap();
        a.write(&[0.9; 4]);
        let mut out = [0.0_f32; 8];
        core.process(&mut out);
        assert!(out.iter().all(|&s| s == 0.0));
        ctl.collect_garbage();
        assert_eq!(ctl.status(&"a"), Err(MixerError::UnknownSource));
        assert!(core.slots.is_empty() && core.graveyard.is_empty());
    }

    #[test]
    fn errors_for_duplicates_unknown_and_limit() {
        let cfg = MixerConfig {
            max_sources: 2,
            ..test_config()
        };
        let (mut ctl, _core) = mixer(cfg);
        ctl.add_source("a", Layout::Mono, 1.0).unwrap();
        assert_eq!(
            ctl.add_source("a", Layout::Mono, 1.0).err(),
            Some(MixerError::DuplicateSource)
        );
        ctl.add_source("b", Layout::Mono, 1.0).unwrap();
        assert_eq!(
            ctl.add_source("c", Layout::Mono, 1.0).err(),
            Some(MixerError::TooManySources)
        );
        assert_eq!(ctl.set_gain(&"zzz", 1.0), Err(MixerError::UnknownSource));
        assert_eq!(ctl.remove_source(&"zzz"), Err(MixerError::UnknownSource));
    }

    #[test]
    fn speaking_and_level_are_reported_pre_volume() {
        let (mut ctl, mut core) = mixer(test_config());
        let mut a = ctl.add_source("a", Layout::Mono, 0.0).unwrap();
        a.write(&[0.3; 480]);
        let mut out = vec![0.0_f32; 960];
        core.process(&mut out);
        let st = ctl.status(&"a").unwrap();
        assert!(st.speaking, "speaking even though the local volume is 0");
        assert!((st.level_db + 10.457).abs() < 0.01, "level {}", st.level_db);
    }

    #[test]
    fn latency_is_bounded_when_producer_runs_ahead() {
        let cfg = MixerConfig {
            max_latency_ms: 10, // 480 frames
            ..test_config()
        };
        let (mut ctl, mut core) = mixer(cfg);
        let mut a = ctl.add_source("a", Layout::Mono, 1.0).unwrap();
        let old = vec![0.1_f32; 4800];
        a.write(&old);
        a.write(&[0.9; 480]);
        let mut out = vec![0.0_f32; 960];
        core.process(&mut out); // 480 frames: oldest audio skipped, newest 480 remain
        assert!(
            approx(out[0], 0.9),
            "should play recent audio, got {}",
            out[0]
        );
    }

    #[test]
    fn loud_mix_is_limited() {
        let (mut ctl, mut core) = mixer(test_config());
        let mut a = ctl.add_source("a", Layout::Mono, 2.0).unwrap();
        let mut b = ctl.add_source("b", Layout::Mono, 2.0).unwrap();
        a.write(&[0.9; 256]);
        b.write(&[0.9; 256]);
        let mut out = vec![0.0_f32; 512];
        core.process(&mut out);
        assert!(out.iter().all(|s| s.abs() <= 1.0 + 1e-6));
    }

    #[test]
    fn large_device_blocks_are_processed_in_chunks() {
        let cfg = MixerConfig {
            max_block_frames: 16,
            ..test_config()
        };
        let (mut ctl, mut core) = mixer(cfg);
        let mut a = ctl.add_source("a", Layout::Mono, 1.0).unwrap();
        let ramp: Vec<f32> = (0..100).map(|i| i as f32 / 1000.0).collect();
        a.write(&ramp);
        let mut out = vec![0.0_f32; 200];
        core.process(&mut out);
        for (i, frame) in out.chunks(2).enumerate() {
            assert!(approx(frame[0], i as f32 / 1000.0), "frame {i}");
        }
    }

    #[test]
    fn gain_change_glides_instead_of_jumping() {
        let cfg = MixerConfig {
            gain_ramp_ms: 1.0, // 48 samples
            ..test_config()
        };
        let (mut ctl, mut core) = mixer(cfg);
        let mut a = ctl.add_source("a", Layout::Mono, 1.0).unwrap();
        core.process(&mut [0.0; 2]);
        ctl.set_gain(&"a", 0.0).unwrap();
        a.write(&[0.5; 64]);
        let mut out = vec![0.0_f32; 128];
        core.process(&mut out);
        let left: Vec<f32> = out.chunks(2).map(|f| f[0]).collect();
        assert!(left[0] > 0.45, "first sample still near full volume");
        assert!(
            left.windows(2).all(|w| w[1] <= w[0] + 1e-6),
            "monotonic fade"
        );
        assert_eq!(left[63], 0.0);
    }

    #[test]
    fn works_across_threads() {
        let (mut ctl, mut core) = mixer::<u8>(test_config());
        let mut input = ctl.add_source(7, Layout::Mono, 1.0).unwrap();
        let writer = std::thread::spawn(move || {
            for _ in 0..100 {
                input.write(&[0.25; 480]);
            }
            input.dropped_samples()
        });
        let dropped = writer.join().unwrap();
        let audio = std::thread::spawn(move || {
            let mut out = vec![0.0_f32; 960];
            core.process(&mut out);
            out[0]
        });
        assert!(approx(audio.join().unwrap(), 0.25));
        assert!(
            dropped > 0,
            "500 ms buffer cannot hold 1 s of audio; excess is dropped"
        );
    }
}

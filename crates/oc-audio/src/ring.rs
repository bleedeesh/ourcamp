//! Single-producer / single-consumer lock-free ring buffer for `f32` samples.
//!
//! Decoded audio arrives on a network/decoder thread and is consumed on the audio
//! device thread. The audio thread must never block or allocate, so the hand-off is a
//! classic Lamport queue: the producer owns the write index, the consumer owns the
//! read index, and each publishes its index with `Release` and reads the other's with
//! `Acquire`. Indices grow monotonically (wrapping) and are masked into a power-of-two
//! buffer, so "full" and "empty" are never ambiguous.
//!
//! The SPSC discipline is enforced by the type system: [`Producer`] and [`Consumer`]
//! are not `Clone`, and every mutating method takes `&mut self`.

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// Keeps the two indices on separate cache lines so the producer and consumer
/// threads do not invalidate each other's line on every write (false sharing).
#[repr(align(64))]
struct CachePadded<T>(T);

struct Shared {
    buf: Box<[UnsafeCell<f32>]>,
    mask: usize,
    /// Total samples ever written. Written only by the producer.
    head: CachePadded<AtomicUsize>,
    /// Total samples ever read. Written only by the consumer.
    tail: CachePadded<AtomicUsize>,
}

// SAFETY: `buf` cells are only touched by the producer in the free region
// [head, tail + capacity) and by the consumer in the filled region [tail, head).
// The regions never overlap, and the Release/Acquire pairs on `head`/`tail` order the
// sample writes before the index that makes them visible to the other side.
unsafe impl Sync for Shared {}
unsafe impl Send for Shared {}

impl Shared {
    fn capacity(&self) -> usize {
        self.mask + 1
    }
}

/// Creates a ring buffer holding at least `min_capacity` samples.
///
/// The real capacity is rounded up to the next power of two.
///
/// # Panics
/// Panics if `min_capacity` is zero.
#[must_use]
pub fn ring_buffer(min_capacity: usize) -> (Producer, Consumer) {
    assert!(min_capacity > 0, "ring buffer capacity must be non-zero");
    let capacity = min_capacity.next_power_of_two();
    let buf = (0..capacity).map(|_| UnsafeCell::new(0.0)).collect();
    let shared = Arc::new(Shared {
        buf,
        mask: capacity - 1,
        head: CachePadded(AtomicUsize::new(0)),
        tail: CachePadded(AtomicUsize::new(0)),
    });
    (
        Producer {
            shared: Arc::clone(&shared),
        },
        Consumer { shared },
    )
}

/// Writing half of the ring buffer.
pub struct Producer {
    shared: Arc<Shared>,
}

impl Producer {
    /// Total capacity in samples.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.shared.capacity()
    }

    /// Samples that can be written right now without overwriting unread data.
    #[must_use]
    pub fn free(&self) -> usize {
        let head = self.shared.head.0.load(Ordering::Relaxed);
        let tail = self.shared.tail.0.load(Ordering::Acquire);
        self.capacity() - head.wrapping_sub(tail)
    }

    /// Writes as many samples from `data` as fit and returns how many were written.
    ///
    /// Never blocks. A short write means the consumer has fallen behind; the caller
    /// decides whether to drop the rest (the usual choice for live audio).
    pub fn push_slice(&mut self, data: &[f32]) -> usize {
        let shared = &*self.shared;
        let head = shared.head.0.load(Ordering::Relaxed);
        let tail = shared.tail.0.load(Ordering::Acquire);
        let free = shared.capacity() - head.wrapping_sub(tail);
        let n = free.min(data.len());
        for (i, &sample) in data[..n].iter().enumerate() {
            let idx = head.wrapping_add(i) & shared.mask;
            // SAFETY: `idx` lies in the free region owned by the producer (see `Shared`).
            unsafe { *shared.buf[idx].get() = sample };
        }
        shared.head.0.store(head.wrapping_add(n), Ordering::Release);
        n
    }
}

/// Reading half of the ring buffer.
pub struct Consumer {
    shared: Arc<Shared>,
}

impl Consumer {
    /// Total capacity in samples.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.shared.capacity()
    }

    /// Samples available to read right now.
    #[must_use]
    pub fn available(&self) -> usize {
        let tail = self.shared.tail.0.load(Ordering::Relaxed);
        let head = self.shared.head.0.load(Ordering::Acquire);
        head.wrapping_sub(tail)
    }

    /// Reads up to `out.len()` samples into `out` and returns how many were read.
    pub fn pop_slice(&mut self, out: &mut [f32]) -> usize {
        let shared = &*self.shared;
        let tail = shared.tail.0.load(Ordering::Relaxed);
        let head = shared.head.0.load(Ordering::Acquire);
        let n = head.wrapping_sub(tail).min(out.len());
        for (i, slot) in out[..n].iter_mut().enumerate() {
            let idx = tail.wrapping_add(i) & shared.mask;
            // SAFETY: `idx` lies in the filled region owned by the consumer (see `Shared`).
            *slot = unsafe { *shared.buf[idx].get() };
        }
        shared.tail.0.store(tail.wrapping_add(n), Ordering::Release);
        n
    }

    /// Discards up to `n` of the oldest samples and returns how many were discarded.
    ///
    /// Used to cap latency when a producer delivers faster than the device consumes.
    pub fn skip(&mut self, n: usize) -> usize {
        let shared = &*self.shared;
        let tail = shared.tail.0.load(Ordering::Relaxed);
        let head = shared.head.0.load(Ordering::Acquire);
        let n = head.wrapping_sub(tail).min(n);
        shared.tail.0.store(tail.wrapping_add(n), Ordering::Release);
        n
    }
}

#[cfg(test)]
// Samples pass through untouched, so exact float comparison is the right check.
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    #[test]
    fn capacity_rounds_up_to_power_of_two() {
        let (p, c) = ring_buffer(1000);
        assert_eq!(p.capacity(), 1024);
        assert_eq!(c.capacity(), 1024);
        assert_eq!(p.free(), 1024);
        assert_eq!(c.available(), 0);
    }

    #[test]
    #[should_panic(expected = "non-zero")]
    fn zero_capacity_panics() {
        let _ = ring_buffer(0);
    }

    #[test]
    fn push_then_pop_preserves_order() {
        let (mut p, mut c) = ring_buffer(8);
        assert_eq!(p.push_slice(&[1.0, 2.0, 3.0]), 3);
        let mut out = [0.0; 3];
        assert_eq!(c.pop_slice(&mut out), 3);
        assert_eq!(out, [1.0, 2.0, 3.0]);
    }

    #[test]
    fn push_stops_when_full_and_pop_stops_when_empty() {
        let (mut p, mut c) = ring_buffer(4);
        assert_eq!(p.push_slice(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]), 4);
        assert_eq!(p.free(), 0);
        let mut out = [9.0; 6];
        assert_eq!(c.pop_slice(&mut out), 4);
        assert_eq!(
            out,
            [1.0, 2.0, 3.0, 4.0, 9.0, 9.0],
            "unread tail stays untouched"
        );
        assert_eq!(c.pop_slice(&mut out), 0);
    }

    #[test]
    fn wraps_around_many_times() {
        let (mut p, mut c) = ring_buffer(4);
        let mut next = 0.0_f32;
        let mut expected = 0.0_f32;
        for _ in 0..1000 {
            let chunk = [next, next + 1.0, next + 2.0];
            assert_eq!(p.push_slice(&chunk), 3);
            next += 3.0;
            let mut out = [0.0; 3];
            assert_eq!(c.pop_slice(&mut out), 3);
            for v in out {
                assert!((v - expected).abs() < f32::EPSILON);
                expected += 1.0;
            }
        }
    }

    #[test]
    fn skip_discards_oldest_samples() {
        let (mut p, mut c) = ring_buffer(8);
        p.push_slice(&[1.0, 2.0, 3.0, 4.0]);
        assert_eq!(c.skip(3), 3);
        let mut out = [0.0; 2];
        assert_eq!(c.pop_slice(&mut out), 1);
        assert!((out[0] - 4.0).abs() < f32::EPSILON);
        assert_eq!(c.skip(10), 0);
    }

    #[test]
    fn concurrent_transfer_is_lossless_and_ordered() {
        // Miri interprets every instruction; a smaller run still exercises wrap-around.
        const TOTAL: usize = if cfg!(miri) { 2_000 } else { 200_000 };
        let (mut p, mut c) = ring_buffer(256);
        let producer = std::thread::spawn(move || {
            let mut sent = 0usize;
            while sent < TOTAL {
                let end = (sent + 37).min(TOTAL);
                let chunk: Vec<f32> = (sent..end).map(|v| v as f32).collect();
                let mut offset = 0;
                while offset < chunk.len() {
                    offset += p.push_slice(&chunk[offset..]);
                    std::hint::spin_loop();
                }
                sent = end;
            }
        });
        let mut received = 0usize;
        let mut buf = [0.0_f32; 64];
        while received < TOTAL {
            let n = c.pop_slice(&mut buf);
            for &v in &buf[..n] {
                assert_eq!(v as usize, received, "sample out of order or lost");
                received += 1;
            }
            if n == 0 {
                std::hint::spin_loop();
            }
        }
        producer.join().expect("producer thread panicked");
    }
}

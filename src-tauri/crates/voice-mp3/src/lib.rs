//! A bounded, synchronous voice encoder shared by Android and iOS.
//!
//! Call from a blocking worker. Input is raw 16 kHz little-endian PCM16 mono,
//! never a WAV container. Output is 64 kbps CBR MP3 with the final frames flushed.

use std::ffi::{c_int, c_short, c_void};
use std::fmt;
use std::ptr::NonNull;
use std::sync::{Mutex, MutexGuard, TryLockError};
use std::time::Duration;

const SAMPLE_RATE: usize = 16_000;
const MAX_SAMPLES: usize = SAMPLE_RATE * 30;
const CHUNK_SAMPLES: usize = 2_048;
// Upstream lame.h: 1.25 * input samples + 7200, also sufficient for flush.
const ENCODE_BUFFER_BYTES: usize = CHUNK_SAMPLES * 5 / 4 + 7_200;

// LAME rewrites global quantization tables in iteration_init for each handle.
// Protect initialization AND encoding: another handle must not initialize while
// an encoder reads those tables. This also protects init_log_table's static flag.
static ENCODER_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EncodeError {
    EmptyInput,
    InvalidPcmLength,
    DurationExceeded,
    Cancelled,
    AllocationFailed,
    InitializationFailed,
    ConfigurationFailed(&'static str),
    EncodingFailed(i32),
    OutputLimitExceeded,
}

impl fmt::Display for EncodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyInput => f.write_str("PCM input is empty"),
            Self::InvalidPcmLength => f.write_str("PCM16 input has an incomplete sample"),
            Self::DurationExceeded => f.write_str("PCM input exceeds 30 seconds"),
            Self::Cancelled => f.write_str("MP3 encoding cancelled"),
            Self::AllocationFailed => f.write_str("Unable to allocate MP3 output"),
            Self::InitializationFailed => f.write_str("Unable to initialize the MP3 encoder"),
            Self::ConfigurationFailed(option) => write!(f, "Unable to configure MP3 {option}"),
            Self::EncodingFailed(code) => write!(f, "MP3 encoding failed ({code})"),
            Self::OutputLimitExceeded => f.write_str("MP3 output exceeds its bounded buffer"),
        }
    }
}

impl std::error::Error for EncodeError {}

/// Encode up to 30 seconds of 16 kHz LE PCM16 mono to 64 kbps CBR MP3.
///
/// Cancellation is checked before initialization, while waiting for the native
/// encoder, between 128 ms PCM blocks, and around the final flush. Cancellation
/// discards all partial output. The callback must not recursively call this
/// function; it runs while the process-wide native encoder lock is held.
pub fn encode_pcm16_mono(
    pcm: &[u8],
    mut is_cancelled: impl FnMut() -> bool,
) -> Result<Vec<u8>, EncodeError> {
    if pcm.is_empty() {
        return Err(EncodeError::EmptyInput);
    }
    if !pcm.len().is_multiple_of(2) {
        return Err(EncodeError::InvalidPcmLength);
    }
    let sample_count = pcm.len() / 2;
    if sample_count > MAX_SAMPLES {
        return Err(EncodeError::DurationExceeded);
    }

    let _lock = lock_encoder(&mut is_cancelled)?;
    check_cancelled(&mut is_cancelled)?;
    // Encoder is declared after the lock so it closes before unlocking on every
    // return and during Rust unwinding, including a panicking cancellation probe.
    let encoder = Encoder::new()?;
    let output_limit = sample_count * 5 / 4 + 7_200;
    let mut output = Vec::new();
    output
        .try_reserve_exact(output_limit)
        .map_err(|_| EncodeError::AllocationFailed)?;
    let mut samples = [0 as c_short; CHUNK_SAMPLES];
    let mut encoded = [0_u8; ENCODE_BUFFER_BYTES];

    for chunk in pcm.chunks(CHUNK_SAMPLES * 2) {
        check_cancelled(&mut is_cancelled)?;
        let count = chunk.len() / 2;
        for (sample, bytes) in samples.iter_mut().zip(chunk.chunks_exact(2)) {
            *sample = i16::from_le_bytes([bytes[0], bytes[1]]);
        }
        // SAFETY: the live, uniquely owned handle is configured for mono;
        // both read-only channel pointers reference `count` initialized shorts.
        // The C API receives the actual output allocation size, never zero.
        let written = unsafe {
            ffi::lame_encode_buffer(
                encoder.0.as_ptr(),
                samples.as_ptr(),
                samples.as_ptr(),
                count as c_int,
                encoded.as_mut_ptr(),
                encoded.len() as c_int,
            )
        };
        append_output(&mut output, &encoded, written, output_limit)?;
    }

    check_cancelled(&mut is_cancelled)?;
    // SAFETY: handle is still live; the output buffer exceeds lame.h's 7200-byte
    // flush requirement. This emits encoder delay/padding and the complete tail.
    let written = unsafe {
        ffi::lame_encode_flush(
            encoder.0.as_ptr(),
            encoded.as_mut_ptr(),
            encoded.len() as c_int,
        )
    };
    append_output(&mut output, &encoded, written, output_limit)?;
    check_cancelled(&mut is_cancelled)?;
    Ok(output)
}

fn check_cancelled(is_cancelled: &mut impl FnMut() -> bool) -> Result<(), EncodeError> {
    if is_cancelled() {
        Err(EncodeError::Cancelled)
    } else {
        Ok(())
    }
}

fn lock_encoder(
    is_cancelled: &mut impl FnMut() -> bool,
) -> Result<MutexGuard<'static, ()>, EncodeError> {
    loop {
        check_cancelled(is_cancelled)?;
        match ENCODER_LOCK.try_lock() {
            Ok(guard) => return Ok(guard),
            // Only Rust callbacks can unwind while locked, between C calls; RAII
            // has already closed that handle. The global tables remain valid.
            Err(TryLockError::Poisoned(error)) => return Ok(error.into_inner()),
            Err(TryLockError::WouldBlock) => std::thread::sleep(Duration::from_millis(2)),
        }
    }
}

fn append_output(
    output: &mut Vec<u8>,
    buffer: &[u8],
    written: c_int,
    output_limit: usize,
) -> Result<(), EncodeError> {
    if written < 0 {
        return Err(EncodeError::EncodingFailed(written));
    }
    let count = written as usize;
    if count > buffer.len() || count > output_limit - output.len() {
        return Err(EncodeError::OutputLimitExceeded);
    }
    output.extend_from_slice(&buffer[..count]);
    Ok(())
}

struct Encoder(NonNull<c_void>);

impl Encoder {
    fn new() -> Result<Self, EncodeError> {
        // SAFETY: called while ENCODER_LOCK protects all LAME global state.
        let pointer = unsafe { ffi::lame_init() };
        let encoder = Self(NonNull::new(pointer).ok_or(EncodeError::InitializationFailed)?);
        let handle = encoder.0.as_ptr();
        // SAFETY: setters receive this newly initialized, uniquely owned handle.
        // Constants match the upstream public enums: MONO=3, vbr_off=0.
        unsafe {
            for (option, result) in [
                (
                    "input rate",
                    ffi::lame_set_in_samplerate(handle, SAMPLE_RATE as c_int),
                ),
                (
                    "output rate",
                    ffi::lame_set_out_samplerate(handle, SAMPLE_RATE as c_int),
                ),
                ("channels", ffi::lame_set_num_channels(handle, 1)),
                ("mono mode", ffi::lame_set_mode(handle, 3)),
                ("CBR mode", ffi::lame_set_VBR(handle, 0)),
                ("bitrate", ffi::lame_set_brate(handle, 64)),
                ("quality", ffi::lame_set_quality(handle, 5)),
                ("ReplayGain", ffi::lame_set_findReplayGain(handle, 0)),
                ("VBR tag", ffi::lame_set_bWriteVbrTag(handle, 0)),
            ] {
                if result != 0 {
                    return Err(EncodeError::ConfigurationFailed(option));
                }
            }
            ffi::lame_set_write_id3tag_automatic(handle, 0);
            if ffi::lame_init_params(handle) != 0 {
                return Err(EncodeError::InitializationFailed);
            }
        }
        Ok(encoder)
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        // SAFETY: this is the sole owner; the lock outlives the handle and close
        // runs exactly once on successful completion, cancellation or failure.
        unsafe { ffi::lame_close(self.0.as_ptr()) };
    }
}

mod ffi {
    use super::{c_int, c_short, c_void};

    extern "C" {
        pub fn lame_init() -> *mut c_void;
        pub fn lame_close(handle: *mut c_void) -> c_int;
        pub fn lame_init_params(handle: *mut c_void) -> c_int;
        pub fn lame_set_in_samplerate(handle: *mut c_void, value: c_int) -> c_int;
        pub fn lame_set_out_samplerate(handle: *mut c_void, value: c_int) -> c_int;
        pub fn lame_set_num_channels(handle: *mut c_void, value: c_int) -> c_int;
        pub fn lame_set_mode(handle: *mut c_void, value: c_int) -> c_int;
        pub fn lame_set_VBR(handle: *mut c_void, value: c_int) -> c_int;
        pub fn lame_set_brate(handle: *mut c_void, value: c_int) -> c_int;
        pub fn lame_set_quality(handle: *mut c_void, value: c_int) -> c_int;
        pub fn lame_set_findReplayGain(handle: *mut c_void, value: c_int) -> c_int;
        pub fn lame_set_bWriteVbrTag(handle: *mut c_void, value: c_int) -> c_int;
        pub fn lame_set_write_id3tag_automatic(handle: *mut c_void, value: c_int);
        pub fn lame_encode_buffer(
            handle: *mut c_void,
            left: *const c_short,
            right: *const c_short,
            samples: c_int,
            output: *mut u8,
            output_size: c_int,
        ) -> c_int;
        pub fn lame_encode_flush(handle: *mut c_void, output: *mut u8, output_size: c_int)
            -> c_int;
    }
}

#[cfg(test)]
mod tests;

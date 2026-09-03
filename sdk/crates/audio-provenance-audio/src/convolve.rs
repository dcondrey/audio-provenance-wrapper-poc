use crate::error::AudioError;
use crate::fft::{MAX_TRANSFORM_LEN, RealFft};

/// Below this many taps, overlap-add costs more than the direct sum it replaces.
const DIRECT_TAP_LIMIT: usize = 64;

/// Linear convolution of `signal` with `ir`, returning `signal.len() + ir.len() - 1` samples.
///
/// PERF: a large-room impulse response runs to tens of thousands of taps, so anything past
/// [`DIRECT_TAP_LIMIT`] goes through overlap-add rather than the direct sum.
///
/// IMPORTANT: the transform runs in f64 regardless of the f32 interface. An acoustic tail is
/// summed across thousands of blocks and f32 accumulation visibly colours the decay.
pub fn convolve(signal: &[f32], ir: &[f32]) -> Result<Vec<f32>, AudioError> {
    if signal.is_empty() || ir.is_empty() {
        return Ok(Vec::new());
    }
    let out_len = signal
        .len()
        .checked_add(ir.len())
        .and_then(|sum| sum.checked_sub(1))
        .ok_or(AudioError::SampleLimitExceeded {
            limit: usize::MAX,
            requested: u64::MAX,
        })?;

    if ir.len() <= DIRECT_TAP_LIMIT {
        let mut out = vec![0.0f32; out_len];
        for (i, &x) in signal.iter().enumerate() {
            if x == 0.0 {
                continue;
            }
            for (j, &h) in ir.iter().enumerate() {
                out[i + j] += x * h;
            }
        }
        return Ok(out);
    }

    let fft_size = ir
        .len()
        .checked_mul(4)
        .map(usize::next_power_of_two)
        .map(|size| size.max(1024))
        .ok_or(AudioError::InvalidTransformLength {
            found: ir.len(),
            reason: "impulse response is too long to transform",
        })?;
    if fft_size > MAX_TRANSFORM_LEN {
        return Err(AudioError::InvalidTransformLength {
            found: fft_size,
            reason: "impulse response exceeds the maximum transform length",
        });
    }
    let fft = RealFft::<f64>::new(fft_size)?;
    let bins = fft.bins();
    let block = fft_size - ir.len() + 1;

    let mut kernel_time = fft.scratch_input();
    for (dst, &src) in kernel_time.iter_mut().zip(ir.iter()) {
        *dst = f64::from(src);
    }
    let mut kernel = fft.scratch_spectrum();
    fft.forward(&mut kernel_time, &mut kernel)?;

    let mut out = vec![0.0f64; out_len];
    let mut time = fft.scratch_input();
    let mut spectrum = fft.scratch_spectrum();
    let mut offset = 0usize;
    while offset < signal.len() {
        let end = (offset + block).min(signal.len());
        time.iter_mut().for_each(|v| *v = 0.0);
        for (dst, &src) in time.iter_mut().zip(signal[offset..end].iter()) {
            *dst = f64::from(src);
        }
        fft.forward(&mut time, &mut spectrum)?;
        for k in 0..bins {
            spectrum[k] *= kernel[k];
        }
        fft.inverse(&mut spectrum, &mut time)?;
        for (k, &value) in time.iter().enumerate() {
            if let Some(slot) = out.get_mut(offset + k) {
                *slot += value;
            }
        }
        offset = end;
    }
    Ok(out.into_iter().map(|v| v as f32).collect())
}

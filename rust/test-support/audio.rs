//! Audio containers for the integration tests, `#[path]`-included by each crate that needs them.
#![allow(dead_code)]

/// 44100 Hz as the 80-bit extended float that AIFF's COMM chunk requires.
pub const RATE_44100_EXT80: [u8; 10] = [0x40, 0x0E, 0xAC, 0x44, 0, 0, 0, 0, 0, 0];

/// A canonical 44-byte-header RIFF/WAVE file around `data`.
pub fn riff_wav(format_tag: u16, channels: u16, rate: u32, bits: u16, data: &[u8]) -> Vec<u8> {
    let block_align = channels * bits / 8;
    let mut out = b"RIFF".to_vec();
    out.extend(((36 + data.len()) as u32).to_le_bytes());
    out.extend(b"WAVEfmt ");
    out.extend(16u32.to_le_bytes());
    out.extend(format_tag.to_le_bytes());
    out.extend(channels.to_le_bytes());
    out.extend(rate.to_le_bytes());
    out.extend((rate * u32::from(block_align)).to_le_bytes());
    out.extend(block_align.to_le_bytes());
    out.extend(bits.to_le_bytes());
    out.extend(b"data");
    out.extend((data.len() as u32).to_le_bytes());
    out.extend(data);
    out
}

/// A FORM file of `form_type` ("AIFF" or "AIFC") at 44.1 kHz; `comm_tail` is the AIFC compression id.
pub fn aiff(form_type: &[u8; 4], channels: u16, frames: u32, bits: u16, comm_tail: &[u8], pcm: &[u8]) -> Vec<u8> {
    let mut comm = channels.to_be_bytes().to_vec();
    comm.extend(frames.to_be_bytes());
    comm.extend(bits.to_be_bytes());
    comm.extend(RATE_44100_EXT80);
    comm.extend(comm_tail);
    let mut ssnd = vec![0; 8];
    ssnd.extend(pcm);
    let mut body = form_type.to_vec();
    body.extend(b"COMM");
    body.extend((comm.len() as u32).to_be_bytes());
    body.extend(comm);
    body.extend(b"SSND");
    body.extend((ssnd.len() as u32).to_be_bytes());
    body.extend(ssnd);
    let mut out = b"FORM".to_vec();
    out.extend((body.len() as u32).to_be_bytes());
    out.extend(body);
    out
}

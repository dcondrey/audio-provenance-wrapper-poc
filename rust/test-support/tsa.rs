//! Loopback TSA/HTTP plumbing for the integration tests, `#[path]`-included by each crate that needs it.
#![allow(dead_code, clippy::unwrap_used, clippy::indexing_slicing)]

/// `(body_start, body_len)` once `received` holds a whole HTTP request.
pub fn complete_request(received: &[u8]) -> Option<(usize, usize)> {
    let split = received.windows(4).position(|w| w == b"\r\n\r\n")?;
    let head = String::from_utf8_lossy(&received[..split]).to_ascii_lowercase();
    let length: usize = head
        .lines()
        .find_map(|line| line.strip_prefix("content-length: "))
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(0);
    (received.len() >= split + 4 + length).then_some((split + 4, length))
}

pub fn der(tag: u8, content: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    if content.len() < 0x80 {
        out.push(content.len() as u8);
    } else {
        out.extend([0x82, (content.len() >> 8) as u8, content.len() as u8]);
    }
    out.extend_from_slice(content);
    out
}

pub fn der_uint(magnitude: &[u8]) -> Vec<u8> {
    let mut body: Vec<u8> = magnitude.iter().copied().skip_while(|b| *b == 0).collect();
    if body.first().is_none_or(|b| b & 0x80 != 0) {
        body.insert(0, 0);
    }
    der(0x02, &body)
}

/// A granted TimeStampResp bound to the request's hash and nonce.
pub fn echoing_reply(request: &[u8]) -> Vec<u8> {
    let (hash, nonce) = apw_core::parse_timestamp_request(request).unwrap();
    let sha256 = [0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01];
    let mut algorithm = sha256.to_vec();
    algorithm.extend(der(0x05, &[]));
    let mut imprint = der(0x30, &algorithm);
    imprint.extend(der(0x04, &apw_core::python_from_hex(&hash).unwrap()));
    let mut tst = der(0x02, &[1]);
    tst.extend([0x06, 0x03, 0x2a, 0x03, 0x04]);
    tst.extend(der(0x30, &imprint));
    tst.extend(der_uint(&[9, 9]));
    tst.extend(der(0x18, b"20260928123000Z"));
    tst.extend(der_uint(&nonce));
    let mut encap = vec![0x06, 0x0b, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x09, 0x10, 0x01, 0x04];
    encap.extend(der(0xA0, &der(0x04, &der(0x30, &tst))));
    let mut signed = der_uint(&[3]);
    signed.extend(der(0x31, &[]));
    signed.extend(der(0x30, &encap));
    let mut token = vec![0x06, 0x09, 0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x07, 0x02];
    token.extend(der(0xA0, &der(0x30, &signed)));
    let mut response = der(0x30, &der_uint(&[0]));
    response.extend(der(0x30, &token));
    der(0x30, &response)
}

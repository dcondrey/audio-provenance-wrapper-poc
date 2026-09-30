import struct
from pathlib import Path


def write_wav(path: Path, frames: int = 4000, rate: int = 44100, channels: int = 1,
              float32: bool = False, pcm: bytes | None = None) -> Path:
    width = 4 if float32 else 2
    if pcm is None:
        pcm = b"".join(
            struct.pack("<f", (i % 100) / 100.0) if float32 else struct.pack("<h", (i * 37) % 3000 - 1500)
            for i in range(frames * channels)
        )
    fmt = struct.pack("<HHIIHH", 3 if float32 else 1, channels, rate, rate * channels * width, channels * width, width * 8)
    body = b"WAVEfmt " + struct.pack("<I", len(fmt)) + fmt + b"data" + struct.pack("<I", len(pcm)) + pcm
    path.write_bytes(b"RIFF" + struct.pack("<I", len(body)) + body)
    return path


def write_aiff(path: Path, frames: int = 4000, rate: int = 44100, channels: int = 1) -> Path:
    exponent = 16383 + rate.bit_length() - 1
    comm = struct.pack(">hIh", channels, frames, 16) + struct.pack(">HQ", exponent, rate << (64 - rate.bit_length()))
    pcm = b"".join(struct.pack(">h", (i * 37) % 3000 - 1500) for i in range(frames * channels))
    ssnd = struct.pack(">II", 0, 0) + pcm
    body = b"AIFF" + b"COMM" + struct.pack(">I", len(comm)) + comm + b"SSND" + struct.pack(">I", len(ssnd)) + ssnd
    path.write_bytes(b"FORM" + struct.pack(">I", len(body)) + body)
    return path

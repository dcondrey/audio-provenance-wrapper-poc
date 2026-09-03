#!/usr/bin/env python3
"""Fail when a forbidden operation enters the plug-in audio-thread surface.

This is deliberately a small source gate, not a C++ parser and not proof about
third-party host code. The executable C++ test complements it by counting
operator-new calls on the callback thread while exercising both precisions,
layouts, bypass, and changing block sizes.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

SURFACE: dict[Path, tuple[str, ...]] = {
    ROOT / "src" / "PluginProcessor.cpp": (
        "AudioProvenanceCaptureAudioProcessor::processBlock",
        "AudioProvenanceCaptureAudioProcessor::processBlockBypassed",
        "AudioProvenanceCaptureAudioProcessor::passThrough",
        "AudioProvenanceCaptureAudioProcessor::observeAudioBuffer",
        "AudioProvenanceCaptureAudioProcessor::handleBypassedBuffer",
    ),
    ROOT / "src" / "AudioObserver.cpp": (
        "AudioObserver::pushAudioBlock",
        "AudioObserver::pushMidiMessages",
        "AudioObserver::updateTransportState",
        "AudioObserver::updateSessionConfig",
        "AudioObserver::recordExternalAudioDrop",
        "AudioObserver::recordBypassedBlock",
        "AudioObserver::setBypassActive",
        "AudioObserver::markLifecycleDiscontinuity",
    ),
}

FORBIDDEN: tuple[tuple[str, re.Pattern[str]], ...] = (
    ("heap allocation", re.compile(r"\b(new|delete|malloc|calloc|realloc|free)\b")),
    ("lock", re.compile(r"\b(ScopedLock|ScopedTryLock|SpinLock|CriticalSection|mutex|lock_guard|unique_lock)\b")),
    ("filesystem", re.compile(r"\b(juce::File|FileInputStream|FileOutputStream|fstream|ifstream|ofstream|fopen|open)\b")),
    ("JSON", re.compile(r"\b(JSON::|DynamicObject|buildJsonEvent|parseBounded)")),
    ("network", re.compile(r"\b(DatagramSocket|StreamingSocket|socket|sendto|recvfrom)\b")),
    ("logging", re.compile(r"\b(Logger|DBG|std::cout|std::cerr|printf|fprintf|syslog)\b")),
    ("SDK", re.compile(r"\b(c2pa|audio_provenance_sdk|genotrace|RegistryBackend)\b", re.IGNORECASE)),
    ("known allocating JUCE call", re.compile(r"\.(setSize|add|insert|append|toString|getMessage)\s*\(")),
)


def mask_comments_and_literals(source: str) -> str:
    """Preserve offsets/braces while blanking comments and quoted contents."""
    out = list(source)
    index = 0
    state = "code"
    quote = ""
    while index < len(source):
        pair = source[index : index + 2]
        char = source[index]
        if state == "code":
            if pair == "//":
                out[index] = out[index + 1] = " "
                index += 2
                state = "line_comment"
                continue
            if pair == "/*":
                out[index] = out[index + 1] = " "
                index += 2
                state = "block_comment"
                continue
            if char in {'"', "'"}:
                quote = char
                out[index] = " "
                index += 1
                state = "literal"
                continue
        elif state == "line_comment":
            if char == "\n":
                state = "code"
            else:
                out[index] = " "
            index += 1
            continue
        elif state == "block_comment":
            out[index] = " "
            if pair == "*/":
                out[index + 1] = " "
                index += 2
                state = "code"
                continue
            index += 1
            continue
        elif state == "literal":
            out[index] = " "
            if char == "\\" and index + 1 < len(source):
                out[index + 1] = " "
                index += 2
                continue
            if char == quote:
                state = "code"
            index += 1
            continue
        index += 1
    return "".join(out)


def function_bodies(source: str, qualified_name: str) -> list[tuple[int, str]]:
    masked = mask_comments_and_literals(source)
    bodies: list[tuple[int, str]] = []
    pattern = re.compile(re.escape(qualified_name) + r"\s*\(")
    for match in pattern.finditer(masked):
        opening = masked.find("{", match.end())
        semicolon = masked.find(";", match.end())
        if opening < 0 or (0 <= semicolon < opening):
            continue
        depth = 0
        for cursor in range(opening, len(masked)):
            if masked[cursor] == "{":
                depth += 1
            elif masked[cursor] == "}":
                depth -= 1
                if depth == 0:
                    line = source.count("\n", 0, match.start()) + 1
                    bodies.append((line, masked[opening : cursor + 1]))
                    break
    return bodies


def main() -> int:
    violations: list[str] = []
    audited = 0
    for path, names in SURFACE.items():
        source = path.read_text(encoding="utf-8")
        for name in names:
            bodies = function_bodies(source, name)
            if not bodies:
                violations.append(f"{path.relative_to(ROOT)}: missing audited function {name}")
                continue
            for line, body in bodies:
                audited += 1
                for label, pattern in FORBIDDEN:
                    hit = pattern.search(body)
                    if hit:
                        violations.append(
                            f"{path.relative_to(ROOT)}:{line}: {name}: forbidden {label}: {hit.group(0)}"
                        )

    if violations:
        print("real-time source audit FAILED", file=sys.stderr)
        for violation in violations:
            print(f"  {violation}", file=sys.stderr)
        return 1
    print(f"real-time source audit passed ({audited} audio-thread function bodies)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

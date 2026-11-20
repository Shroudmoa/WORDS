#!/usr/bin/env python3
"""Small synthesis helper for WORDS.

Rust owns the terminal, the cache and playback. This script does one thing:
turn text into an MP3 plus a list of word boundaries using Microsoft Edge's
neural voices through `edge-tts`. It is embedded into the binary at build time
with include_str!, so no source file is needed on the installed machine.

Install once with:  python3 -m pip install edge-tts
"""

import argparse
import asyncio
import json
import sys


def fail(message):
    print(message, file=sys.stderr)
    raise SystemExit(2)


async def synthesise(args):
    try:
        import edge_tts
    except ImportError:
        fail(
            "edge-tts is missing — install it with: "
            "python3 -m pip install edge-tts"
        )

    communicate = edge_tts.Communicate(
        args.text,
        args.voice,
        rate=args.rate,
        volume=args.volume,
        pitch=args.pitch,
    )
    words = []
    with open(args.out, "wb") as audio:
        try:
            async for chunk in communicate.stream():
                kind = chunk.get("type")
                if kind == "audio":
                    audio.write(chunk.get("data", b""))
                elif kind == "WordBoundary":
                    words.append(
                        {
                            "text": chunk.get("text", ""),
                            "offset": int(chunk.get("offset", 0) or 0),
                            "duration": int(chunk.get("duration", 0) or 0),
                        }
                    )
        except Exception as exc:  # noqa: BLE001 - reported to the TUI, not swallowed
            fail(f"edge-tts synthesis failed: {exc}")

    # TSV rather than JSON: Rust has no serde dependency, and a word boundary
    # never contains a tab or newline.
    with open(args.timings, "w", encoding="utf-8") as file:
        for word in words:
            file.write(
                f"{word['offset']}\t{word['duration']}\t{word['text']}\n"
            )
    return 0


async def list_voices():
    try:
        import edge_tts
    except ImportError:
        fail("edge-tts is missing — python3 -m pip install edge-tts")
    try:
        voices = await edge_tts.list_voices()
    except Exception as exc:  # noqa: BLE001
        fail(f"could not list voices: {exc}")
    for voice in voices:
        print(
            json.dumps(
                {
                    "name": voice.get("ShortName", ""),
                    "locale": voice.get("Locale", ""),
                    "gender": voice.get("Gender", ""),
                }
            )
        )
    return 0


def check():
    try:
        import edge_tts  # noqa: F401
    except ImportError:
        fail("edge-tts is missing — python3 -m pip install edge-tts")
    print("edge-tts ready")
    return 0


def main():
    parser = argparse.ArgumentParser(description="WORDS voice helper")
    sub = parser.add_subparsers(dest="command", required=True)

    synth = sub.add_parser("synthesize")
    synth.add_argument("--text", required=True)
    synth.add_argument("--voice", required=True)
    synth.add_argument("--rate", default="+0%")
    synth.add_argument("--volume", default="+0%")
    synth.add_argument("--pitch", default="+0Hz")
    synth.add_argument("--out", required=True)
    synth.add_argument("--timings", required=True)

    sub.add_parser("list-voices")
    sub.add_parser("check")

    args = parser.parse_args()
    if args.command == "synthesize":
        return asyncio.run(synthesise(args))
    if args.command == "list-voices":
        return asyncio.run(list_voices())
    if args.command == "check":
        return check()
    return 2


if __name__ == "__main__":
    raise SystemExit(main())

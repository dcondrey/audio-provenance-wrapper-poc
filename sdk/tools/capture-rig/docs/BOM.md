# Bill of materials — GenoMark-N Stage 0

Everything needed to run the campaign in `RUNBOOK.md`. Prices are indicative mid-2026 street prices in
GBP including VAT; substitute freely, but keep the *spread* — the point of three speakers and three
microphones is to span the real range, not to find the best pair.

## Must have

| # | Item | Why this one | ~GBP |
|---|---|---|---|
| 1 | USB audio interface, 2-in/2-out, 48 kHz, ≥ 2 ms round-trip at 128 samples | Drives the powered monitor and captures the USB condenser on one clock. Must appear as a CoreAudio device with both directions. Focusrite Scarlett 2i2 4th gen or MOTU M2. | 130–190 |
| 2 | Powered nearfield monitor, single | Speaker A: the "good" end of the range. Any 4–5 inch active monitor. | 90–150 |
| 3 | Small Bluetooth speaker | Speaker B: the honest middle of the consumer range. Must pair as a macOS output device. | 30–60 |
| 4 | *(no purchase)* Laptop internal speakers | Speaker C: the bottom of the range, where the 211 Hz band floor is barely reproduced. Spec 11 names this as an expected failure; measure it anyway so the failure is measured rather than assumed. | 0 |
| 5 | Large-diaphragm USB condenser microphone | Microphone A, the only *live* capture device: the phones cannot be CoreAudio inputs. Audio-Technica AT2020USB-X or equivalent. | 90–130 |
| 6 | *(likely owned)* Current iPhone | Microphone B, `ingest` mode. Voice Memos records 48 kHz; set Settings → Voice Memos → Audio Quality to Lossless. | 0 |
| 7 | *(likely owned)* Current Android phone | Microphone C, `ingest` mode. Any recorder that writes 48 kHz WAV. | 0 |
| 8 | **Sound level meter, IEC 61672 class 2, A-weighting, fast** | **The single most important item on this list.** Without it every SPL and background-dBA field in the campaign is `null` and the campaign reproduces the exact defect spec section 10 exists to fix. | 40–90 |
| 9 | Two microphone stands with boom | One for the USB condenser, one to hold a phone at a repeatable height. | 40–70 |
| 10 | Tape measure, 5 m, and low-tack floor tape | Distances are a reported condition. Mark 0.15, 0.5, 1.0, 2.0 and 3.0 m on the floor once per room and leave the tape down. | 10 |
| 11 | Phone mount for a microphone stand | A phone lying on a desk is a different acoustic object from a phone on a stand. Pick one, keep it, record which. | 10–20 |
| 12 | Two 3 m XLR cables and one 3 m TRS pair | Long enough to reach 3 m without moving the interface. | 30 |
| 13 | Portable SSD, ≥ 1 TB | ~46 GB of captures at 24-bit/48 kHz mono for the campaign below, plus room to keep everything. | 70–110 |

**Total, buying items 1, 2, 3, 5, 8–13: roughly £550–850.**

## Worth having

| Item | Why | ~GBP |
|---|---|---|
| Acoustic panels, 4 × 60×60 cm, plus two corner traps | Turns an ordinary small room into the "treated" room. Without them, room 1's RT60 may exceed 0.6 s and K0 fires on a room-hire problem rather than a physics one. | 120–200 |
| Class 1 sound level meter, or class 2 with a calibrator | A 94 dB acoustic calibrator removes the meter's own drift from every SPL number. | +60–200 |
| Second identical USB condenser | Lets the false-positive arm be captured simultaneously with the marked arm, roughly halving the campaign's wall clock. | 90–130 |
| Laser distance measure | Faster and more repeatable than a tape at 2–3 m. | 25–40 |

## Deliberately not on this list

- **A measurement microphone (Behringer ECM8000, Earthworks M23).** Tempting, and wrong for this
  campaign. The impulse responses being measured are of the *product's* channel — a consumer speaker
  into a phone or a USB condenser — and a flat measurement microphone would produce responses of a
  channel nobody will ever use. Spec 10.2(c) wants the transducer curves *as they are*, to
  parameterise distortion-layer stage D5 with real curves rather than random biquads.
- **An anechoic chamber, or chamber time.** DeAR's varechoic-chamber numbers are precisely the
  numbers spec section 11 says are not comparable to a real deployment.
- **A turntable or robotic positioner.** Five distances by hand is an afternoon.

## Rooms

Three rooms, and their properties are measured and reported rather than assumed. Spec 10.1's targets:

| Role | Target RT60 | Target background | Typical space |
|---|---|---|---|
| `treated` | ~0.25 s | < 30 dBA | Small room with panels; a carpeted bedroom with soft furnishings gets close |
| `office` | ~0.45 s | 35–40 dBA | Ordinary office or study, hard desk, some carpet |
| `live` | ~0.8 s | 40–45 dBA | Kitchen, bathroom, stairwell, tiled or glazed room |

If the treated room measures above 0.6 s at 1.0 m, **K0 fires and the programme stops before
training** (spec 12.3). Measure it first, on day 1, before buying anything else on this list.

# Audio Callback Performance

Date: 2026-08-09

## Linux reference environment and method

- CPU: Intel Core i5-1145G7, 4 cores / 8 threads
- OS/architecture: Linux x86_64
- Compiler: rustc 1.97.1 (`8bab26f4f`, LLVM 22.1.6)
- Profile: Cargo `release`
- Baseline: current `HEAD` with only the strengthened fixture applied in a temporary clone

The current saturated fixture uses the worst-case assignable layout: all ten slots are Chord instruments, each with two overlapping four-note groups, every model-valid LFO destination, distortion/phaser/flanger, maximum delay and reverb sends, 10-second reverb, and high delay feedback. Each configuration runs five independent trials with 128 warm-up callbacks and 512 measured callbacks. The 2,560 measured durations are pooled before calculating statistics. The historical results below predate assignable instruments and remain baseline evidence only; a new result table must be recorded before making a performance claim for the expanded fixture.

Command:

```sh
cargo test --release audio::tests::saturated_callback_benchmark -- --ignored --nocapture
```

## Results

Loads are percentages of the callback deadline. `ns/frame` is the median render cost. The two runs were made consecutively on the same reference machine; isolated maximums remain sensitive to host scheduling.

| Rate | Frames | Baseline median / p95 / p99 / max | Updated median / p95 / p99 / max | Baseline ns/frame | Updated ns/frame |
|---:|---:|---:|---:|---:|---:|
| 44.1 kHz | 128 | 31.7 / 35.9 / 39.1 / 52.4% | 27.8 / 34.3 / 42.7 / 44.6% | 7194.3 | 6311.7 |
| 44.1 kHz | 256 | 31.8 / 34.7 / 41.3 / 51.7% | 27.8 / 30.6 / 41.7 / 44.9% | 7203.2 | 6299.6 |
| 44.1 kHz | 512 | 31.9 / 34.6 / 39.5 / 48.9% | 27.8 / 30.8 / 39.4 / 53.2% | 7226.6 | 6295.1 |
| 48 kHz | 128 | 34.6 / 39.4 / 50.6 / 56.6% | 30.2 / 31.4 / 37.9 / 61.2% | 7202.9 | 6293.0 |
| 48 kHz | 256 | 34.5 / 36.9 / 39.7 / 59.7% | 30.2 / 31.5 / 43.8 / 48.7% | 7192.3 | 6282.9 |
| 48 kHz | 512 | 34.5 / 37.0 / 46.9 / 67.6% | 30.2 / 32.4 / 36.8 / 47.3% | 7181.8 | 6289.5 |
| 96 kHz | 128 | 68.7 / 71.9 / 99.4 / 106.0% | 60.4 / 69.2 / 88.9 / 139.1% | 7157.0 | 6287.5 |
| 96 kHz | 256 | 68.8 / 78.3 / 97.9 / 105.1% | 60.4 / 68.2 / 84.7 / 140.0% | 7170.1 | 6289.8 |
| 96 kHz | 512 | 68.8 / 73.7 / 92.9 / 119.5% | 60.4 / 65.4 / 75.6 / 92.3% | 7162.6 | 6294.0 |

The updated median cost is approximately 12% lower. Every 44.1/48 kHz configuration meets the supported reference target of pooled p95 callback load no higher than 50%. The 96 kHz measurements are best-effort visibility and do not gate completion.

## Four-pole Bass filter follow-up

The saturated fixture was rerun after changing the dedicated Bass filter from three to four nonlinear stages and removing resonance-dependent post-filter makeup from Bass, Chord, and Lead. The additional pole does not materially change median callback cost, and every supported 44.1/48 kHz configuration remains below the 50% p95 target.

| Rate | Frames | Median / p95 / p99 / max | Median ns/frame |
|---:|---:|---:|---:|
| 44.1 kHz | 128 | 27.9 / 32.7 / 42.0 / 62.6% | 6320.4 |
| 44.1 kHz | 256 | 27.9 / 32.2 / 41.7 / 57.5% | 6315.8 |
| 44.1 kHz | 512 | 27.8 / 30.9 / 44.1 / 59.0% | 6311.8 |
| 48 kHz | 128 | 30.3 / 33.4 / 39.3 / 47.8% | 6322.6 |
| 48 kHz | 256 | 30.3 / 34.5 / 36.9 / 73.3% | 6304.5 |
| 48 kHz | 512 | 30.3 / 34.0 / 40.8 / 61.6% | 6320.0 |
| 96 kHz | 128 | 60.6 / 70.2 / 90.8 / 97.9% | 6311.8 |
| 96 kHz | 256 | 60.6 / 71.9 / 92.0 / 96.1% | 6315.3 |
| 96 kHz | 512 | 61.4 / 93.4 / 98.4 / 144.7% | 6400.4 |

The elevated 96 kHz tail latency remains best-effort host-scheduling evidence rather than a completion failure; median cost stays near 6.3 microseconds per frame.

## Assignable-instrument worst-case follow-up

On 2026-08-21 the release fixture was expanded to all ten slots running Chord, with two overlapping four-note groups per slot and the saturated effects/LFO configuration described above. The benchmark command completed successfully with finite output and no callback allocation/deallocation, but this deliberately maximal layout does not meet the historical supported-rate p95 target on this Linux x86_64 host.

| Rate | Frames | Median / p95 / p99 / max | Median ns/frame |
|---:|---:|---:|---:|---:|
| 44.1 kHz | 128 | 138.0 / 140.1 / 154.6 / 302.2% | 31296.1 |
| 44.1 kHz | 256 | 137.8 / 140.3 / 141.9 / 271.7% | 31239.8 |
| 44.1 kHz | 512 | 137.8 / 139.4 / 148.5 / 158.5% | 31238.9 |
| 48 kHz | 128 | 149.8 / 150.9 / 168.9 / 209.6% | 31200.9 |
| 48 kHz | 256 | 150.1 / 152.6 / 173.0 / 323.8% | 31263.0 |
| 48 kHz | 512 | 150.3 / 151.5 / 158.3 / 170.6% | 31302.4 |
| 96 kHz | 128 | 300.3 / 306.7 / 379.0 / 431.7% | 31279.1 |
| 96 kHz | 256 | 301.3 / 310.7 / 350.4 / 646.9% | 31386.3 |
| 96 kHz | 512 | 301.1 / 309.9 / 328.3 / 351.5% | 31369.4 |

These figures define a known performance ceiling, not a claim that ordinary projects regress to this cost. Functional support for arbitrary assignments remains deterministic and allocation-safe; a future optimization pass is required before claiming the 50% p95 target for the absolute all-voicing worst case.

## Evidence policy and device verification

Wall-clock timing is a controlled local engineering measurement, not an automated regression proof or a universal dropout guarantee. Ordinary tests enforce deterministic finite output and callback allocation/deallocation safety under saturated DSP, audition, replacement, Stop/Play, and retirement-queue pressure; they contain no host-dependent timing limit.

The earlier release binary completed a 5 minute 30 second saturated ALSA `null` run at 44.1 kHz and 512 frames with zero reported overruns. A 128-frame exploratory run reported two overruns. A new controlled ten-minute device run was not performed as part of this code-only pass; the acceptance procedure requires recording both 512-frame and, where supported, 128-frame results without claiming universal dropout freedom.

The application is also verified on macOS, where CPAL uses CoreAudio. The measurements above are Linux-only reference data and do not claim equivalent performance across every macOS device or configuration.

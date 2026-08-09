# Audio Callback Performance

Date: 2026-08-09

## Reference environment

- CPU: Intel Core i5-1145G7, 4 cores / 8 threads
- OS/architecture: Linux x86_64
- Compiler: rustc 1.97.1 (`8bab26f4f`, LLVM 22.1.6)
- Profile: Cargo `release`
- Baseline: commit `e2584c6` with only the strengthened benchmark fixture applied in a temporary tree

The fixture activates all drums, Bass, Lead, two overlapping four-voice Chord groups, track LFOs, all insert effects, maximum sends, long reverb, and high global feedback. It warms up for 64 callbacks and measures 64 callbacks. Ordinary tests report timing but deliberately contain no machine-dependent timing assertion.

Command:

```sh
cargo test --release audio::tests::worst_case_fixture_reports_callback_cost_without_a_brittle_limit -- --nocapture
```

## Comparable results

| Sample rate | Frames | Baseline p95 | Updated p95 | Change | Updated max |
|---:|---:|---:|---:|---:|---:|
| 44.1 kHz | 128 | 29.3% | 30.2% | +3.1% | 30.9% |
| 44.1 kHz | 256 | 29.5% | 30.2% | +2.4% | 31.1% |
| 44.1 kHz | 512 | 29.3% | 30.0% | +2.4% | 30.2% |
| 48 kHz | 128 | 31.6% | 32.7% | +3.5% | 33.2% |
| 48 kHz | 256 | 31.8% | 32.6% | +2.5% | 33.4% |
| 48 kHz | 512 | 31.7% | 32.6% | +2.8% | 32.9% |
| 96 kHz | 128 | 63.4% | 65.4% | +3.2% | 65.9% |
| 96 kHz | 256 | 63.5% | 65.1% | +2.5% | 66.4% |
| 96 kHz | 512 | 63.1% | 66.7% | +5.7% | 68.3% |

The feedback-aware effect lifecycle adds 2–6% relative p95 cost under this deliberately saturated fixture. Every configuration remains within the agreed 10% regression allowance and below its callback deadline. This is reference-machine evidence, not a guarantee for every device or system load.

## Allocation and device verification

The callback allocation tests separately cover active worst-case rendering, audition, project replacement, Stop/Play transitions, and retirement-queue saturation. All callback invocations perform zero allocations and deallocations.

The release binary played the saturated fixture through ALSA `null` for 5 minutes 30 seconds at the device-default 44.1 kHz sample rate and an explicit 512-frame buffer. With no competing build workload, the persistent UI telemetry reported zero callback overruns. An exploratory 128-frame run reported two overruns with a 123% maximum, so 128 frames is not claimed as a dropout-free reference configuration on this machine.

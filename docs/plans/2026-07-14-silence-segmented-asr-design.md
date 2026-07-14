# Silence-Segmented ASR Design

## Goal

Reduce the delay after releasing the push-to-talk key by recognizing completed speech segments during natural pauses without ending the recording session or injecting text early.

## Behavior

- Push-to-talk remains the only way to start and stop recording.
- After valid speech, 600 ms of continuous low volume closes the current segment.
- Completed segment text updates only the native overlay.
- Recording continues while the key remains held.
- Releasing the key recognizes the remaining tail, joins all segment results in order, and injects the combined text once.
- Model-test recording does not use silence segmentation.

## Silence Detection

- Silence detection runs in `audio-worker`, never in the CPAL callback.
- Use block RMS and a short initial calibration period to estimate the ambient noise floor.
- Derive a dynamic speech threshold from the noise floor with lower and upper bounds.
- Use hysteresis and minimum voiced duration to avoid repeated boundaries around the threshold.
- Retain a short pre-roll while waiting for speech so initial consonants are not lost.
- Pure silence never creates an ASR segment.

## Audio Protocol

- Audio samples and segment boundaries travel through the same audio-to-ASR channel so ordering is deterministic.
- A segment boundary flushes buffered audio before the boundary marker.
- The final stop still flushes the resampler and remaining audio before notifying the application.

## ASR Worker

- The ASR worker remains the sole model owner and performs at most one inference at a time.
- A silence boundary finalizes the current engine session, caches its text and metrics, emits cumulative text as a partial result, and starts the next engine session with the same hot words.
- The final request recognizes only the remaining tail and joins it with cached segment text.
- Intermediate results are never sent to the output worker.

## Failure Handling

- A segment inference failure reports an ASR error and cancels the outer recording session.
- Empty segment results are ignored.
- Stale session IDs and boundaries are ignored using the existing session checks.

## Verification

- Unit-test calibration, speech detection, 600 ms silence boundaries, and pure-silence behavior.
- Unit-test segment text joining for Chinese and ASCII words.
- Run `cargo fmt`, `cargo test`, and `cargo build`.

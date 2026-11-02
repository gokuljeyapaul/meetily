# This is a fork

`gokuljeyapaul/meetily` is Darpana's desktop capture agent, forked from
[`Zackriya-Solutions/meeting-minutes`](https://github.com/Zackriya-Solutions/meeting-minutes)
(MIT, upstream copyright retained in `LICENSE.md`).

**Fork point: `a2cb62e827da7ef59f65064c97233efb2313878e` — Release v0.4.1.**

Everything upstream has fixed since that commit is therefore answerable with:

```bash
git remote add upstream https://github.com/Zackriya-Solutions/meeting-minutes.git
git fetch upstream
git log --oneline a2cb62e..upstream/main
```

## What this fork is for

Darpana analyses 1:1 coaching conversations and links every claim it makes to a
playable span of the recording. Two of its guarantees are decided here, in the
recorder, and cannot be recovered later:

* **The coach and the coachee must be on separate tracks.** Upstream tags each
  captured chunk `DeviceType::Microphone` or `DeviceType::System`, then
  `audio/pipeline.rs` mixes the two for the recording path and relabels the
  result as `Microphone`, and `audio/recording_saver.rs` writes that single
  pre-mixed file. A mixed track has the coachee's voice inside the coach's
  channel, which makes erasure of coachee content impossible — no downstream
  code can separate them again.
* **No capture without consent, and none with a third participant present.**

Summarisation is removed: everything after capture is a different product, and
dead code that touches audio is the kind that gets switched back on.

## How upstream changes reach us (Darpana ADR-0022)

1. **The fork point is recorded above** and does not move silently.
2. **Deletions and additions live in separate commits**, so an upstream change
   to code we deleted conflicts with a pure deletion rather than with our work.
3. **New behaviour goes in new modules** invoked from upstream's seams wherever
   possible; the unavoidable edits are the recording path in `audio/pipeline.rs`
   and `audio/recording_saver.rs`.
4. **We pull security and audio-device fixes, not features** — reviewed once per
   Darpana initiative, never during a baseline measurement, because a recorder
   change moves the numbers. Every pull is an ordinary PR whose tests include the
   channel-purity harness: that harness is what notices an upstream audio change
   putting coachee speech back on the coach track.

## Building this fork

Upstream's instructions apply (`docs/BUILDING.md`, `frontend/clean_run.sh`,
`frontend/clean_run_windows.bat`).

Linux can type-check and run the test suite, which earlier notes in this file
denied: the macOS-only `cidre` dependency pinned at `rev = "a9587fa"` fails to
resolve on cargo 1.83 (`no matching package named cidre found`) but resolves on
1.99, so the obstacle was the toolchain rather than the platform. With
`pkg-config libglib2.0-dev libgtk-3-dev libwebkit2gtk-4.1-dev libsoup-3.0-dev
libjavascriptcoregtk-4.1-dev libasound2-dev libssl-dev cmake clang` installed,
`cargo check --locked --all-targets` and `cargo test --locked` both pass from
Linux.

What Linux still cannot do: compile the macOS and Windows capture paths
(ScreenCaptureKit, WASAPI loopback), which only their own platforms build, and
exercise real audio devices. Those stay with CI's macOS and Windows jobs and,
for device behaviour, a real machine.

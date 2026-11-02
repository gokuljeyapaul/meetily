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
`frontend/clean_run_windows.bat`). One known obstacle, not introduced by us: on
a non-macOS machine `cargo check` still resolves the macOS-only
`cidre` git dependency pinned at `rev = "a9587fa"` and fails with
`no matching package named cidre found`, so the Rust crate cannot be type-checked
from Linux at all. Rust changes here are verified on macOS and Windows.

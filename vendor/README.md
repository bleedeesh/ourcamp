# vendor

## livekit-0.9.7

The `livekit` crate 0.9.7 from crates.io (Apache-2.0, © LiveKit, Inc.), with changes marked
`OurCamp`:

- `src/room/participant/local_participant.rs`: a track whose source has two channels is published
  with `TF_STEREO`. Why: ADR-0055.
- `src/rtc_engine/peer_transport.rs`: a screen share sets a send cap for its connection, which makes
  the estimator probe up to it at once. Why: ADR-0062.

The first is the fix proposed upstream in https://github.com/livekit/rust-sdks/pull/1023; the second
has no such proposal yet, so a release with the first is not enough. **Remove this
copy** and the `[patch.crates-io]` and `exclude` entries in the root `Cargo.toml` once a release
carries that fix (0.10.0 does not); `the_sound_of_a_stream_reaches_a_viewer_in_stereo` tells.

The crate's warnings are silenced at the end of its `Cargo.toml`. Only its `.cargo-ok`,
`.cargo_vcs_info.json` and `Cargo.toml.orig` are left out.

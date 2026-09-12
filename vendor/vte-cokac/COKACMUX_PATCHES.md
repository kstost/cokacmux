# Local vte checkpoint support

Source: the unmodified vte 0.15.0 crate published at
https://crates.io/crates/vte/0.15.0 (upstream: https://github.com/alacritty/vte).
The upstream MIT and Apache-2.0 licenses are retained.

Local changes in src/lib.rs and src/params.rs:

- Make the decoder and parameter storage cloneable.
- Under the existing serde feature, with std enabled, serialize the decoder,
  parameter storage and state enum. This includes unfinished UTF-8, CSI, OSC
  and DCS input; replaying formatted visible cells cannot recover these.
- Validate all serialized slice indices and the incomplete UTF-8 prefix
  before the vt100 wrapper permits a restored decoder to consume input.

This is a private, versioned checkpoint format, not an alternate terminal
escape-sequence protocol. When updating vte, review the checkpoint validator
and bump vt100's checkpoint version if its representation or semantics change.
Do not deserialize a decoder and advance it without validation.

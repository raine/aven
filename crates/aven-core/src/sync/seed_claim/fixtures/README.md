# Sequence-zero genesis fixture

`genesis.json` contains public, artificial test secrets. Never use them outside
fixtures. It freezes the experimental genesis-only profile, not a released sync
protocol or security-approved membership format.

The fixture was generated with Python framing/SHA-256 and a separate locked Rust
primitive oracle using ed25519-dalek 3.0.0 and hpke 0.14.1. The application tests
reconstruct it with their own codec and actual strict Ed25519 and HPKE calls.
This is not independent second-library cryptographic verification.

Record commitment:
`6bd02c075f293f797521f8294f9fa6e38ae8d99fcf975a266f4c1228c021d522`.

The record is 902 bytes, the claim body 912 bytes. Membership sequence and
predecessor are zero. The state contains exactly one device and generation,
credential version one and generation boundary zero, with no bootstrap,
recovery or pending rotation. Core, state and attachments are respectively
229, 280 and 312 bytes. The HPKE plaintext is 175 bytes and ciphertext 191 bytes.

Tests also construct newly signed invalid records, verify strict parser and
cross-field rejection, and demonstrate that signed opaque self-package validity
must be checked by the client rather than inferred by the server.

## Same-generation membership

`membership.json` freezes exact AVID v2 declarations, unchanged PSK requests and
AVAD v2 / AVGS v4 / AVGA v4 admissions for a seed inviting a peer, then that peer
inviting a third device. Both records use the same admission format. Tests rebuild
and compare every byte, not only the stored SHA256 values.

The fixture uses the genesis inputs above and the publication framing fixture in
`publication/tests.rs`. It does not assert bootstrap ciphertext completeness.
Artificial peer inputs use repeated bytes: device 32/42, Ed25519 seed 33/43,
HPKE derive-keypair IKM 34/44, bearer 35/45, invitation PSK 31/41. Request sender
ChaCha20Rng seeds are repeated 36/46; grant sender seed is repeated 90. Expiries
are 2000000000/2000000001. These are test-only deterministic values, not production
entropy or independently reproduced interoperability evidence. Production uses
fallible OS-seeded single-shot HPKE; exact retries retain the resulting bytes.

The pure membership parser rejects old fixed first-peer enrollment versions.
Genesis/publication and unchanged request framing retain their original meaning.

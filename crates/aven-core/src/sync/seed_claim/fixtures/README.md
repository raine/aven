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

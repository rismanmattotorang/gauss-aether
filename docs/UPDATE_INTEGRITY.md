# UPDATE_INTEGRITY.md — chain-anchored updater wire format

> *Sprint 8 §2 of [`/ROADMAP.md`](../ROADMAP.md). Public spec —
> reference impl lives in `gaussclaw_desktop::updater`. Other Rust
> desktop apps are welcome to adopt the format unchanged.*

## 0. TL;DR

Every release artefact is bound to four independent integrity axes
that must all verify before the desktop runtime executes the
update. A single failed axis aborts the update and the trace lands
in the receipt chain. The four axes are deliberately overlapping
defences — an attacker has to compromise SHA-256 *and* the
publisher's Ed25519 key *and* the target-triple match *and* the
no-downgrade ledger to land a bad payload.

## 1. Wire shape

A release manifest is a JSON document:

```json
{
  "version":     "1.2.3",
  "target":      "x86_64-unknown-linux-gnu",
  "sha256":      "9b8c…7a",
  "signature":   "ed25519:b3f4…01",
  "publisher":   "ed25519:7a91…cd",
  "released_at": "2026-05-20T14:30:00Z",
  "prev_chain":  "00000…",
  "chain_index": 42
}
```

Every field is mandatory. The `signature` covers the canonical
signed message defined in §3.

### 1.1 Field semantics

| Field | Type | Semantic |
|---|---|---|
| `version` | semver string | release version |
| `target` | Rust target triple | platform binding (`x86_64-unknown-linux-gnu`, `aarch64-apple-darwin`, `x86_64-pc-windows-msvc`) |
| `sha256` | hex string (64 chars) | SHA-256 of the artefact bytes |
| `signature` | `ed25519:<hex>` | Ed25519 signature over the canonical message |
| `publisher` | `ed25519:<hex>` | Publisher public key (echoed for client convenience) |
| `released_at` | RFC3339 timestamp | publish wall-clock — used for replay-window scans |
| `prev_chain` | hex (64 chars) | previous chain head (links into the public anchor) |
| `chain_index` | u64 | monotonic chain position (no-downgrade gate) |

## 2. Four-axis verification

Every client (the desktop runtime + any third-party verifier) MUST
execute all four checks in order. The reference impl lives in
`gaussclaw_desktop::updater::verify_release_artifact`. A failed
axis returns a typed `UpdaterError` and writes a receipt with the
axis tag (`sha256_mismatch` / `signature_invalid` /
`target_mismatch` / `downgrade_refused`).

### Axis 1 — Target match

The client refuses an artefact whose `target` doesn't match the
running runtime's target triple. Hermes treats target as a hint;
we treat it as a hard binding.

### Axis 2 — SHA-256 binding

The client computes SHA-256 of the downloaded bytes and compares
against `manifest.sha256` byte-by-byte. Mismatch aborts.

### Axis 3 — Publisher signature

Compute the canonical signed message (§3) and verify
`manifest.signature` against `manifest.publisher` (Ed25519). The
client MUST also check the publisher key against an out-of-band
allowlist (the desktop runtime hard-codes the GaussClaw publisher
key).

### Axis 4 — No-downgrade chain link

`manifest.chain_index` must be strictly greater than the locally
stored "last installed" index. `manifest.prev_chain` must match
the locally stored chain head. Both gates close the "rollback
attack" vector: even with a valid publisher signature, an
attacker can't ship a strictly-older artefact.

After successful install, the client appends `(version, target,
sha256, chain_index)` to its local chain ledger and bumps the
stored head.

## 3. Canonical signed message

The Ed25519 signature covers the UTF-8 bytes of:

```
gaussclaw-update/v1
version=<version>
target=<target>
sha256=<sha256>
chain_index=<chain_index>
prev_chain=<prev_chain>
```

Each field is followed by `\n` (U+000A). The trailing newline is
present after `prev_chain`. The reference impl is
`gaussclaw_desktop::updater::canonical_signed_message`.

The message format is stable across versions; a future v2 spec
would mint a new `gaussclaw-update/v2` magic header and clients
would refuse anything else.

## 4. Receipts

Each update operation (download, verify, install, rollback) emits
one [`gauss_audit::Receipt`] entry. The receipt body is JSON:

```json
{
  "kind":        "update",
  "op":          "verify",
  "axis":        "sha256",
  "manifest_id": "<blake3 of canonical manifest bytes>",
  "outcome":     "ok",
  "timestamp":   "2026-05-20T14:32:11Z"
}
```

`axis` is non-empty only on failure (names the first axis that
refused). `outcome ∈ {"ok", "fail"}`.

The chain entry is signed by the local runtime's session key, NOT
the publisher key. An operator can replay the chain to prove "we
verified manifest X at time Y under publisher Z".

## 5. Public key distribution

The reference implementation hard-codes the GaussClaw publisher
public key at compile time. Third-party adopters of this spec MUST
either:

- hard-code their publisher key in the client binary, or
- distribute the key through an independently-anchored channel
  (TUF root, sigstore, ACME-style certificate transparency).

Network-fetched public keys are an explicit non-goal of this spec.

## 6. Reference implementation

- `gaussclaw_desktop::updater::ReleaseManifest` — typed manifest.
- `gaussclaw_desktop::updater::verify_release_artifact` — the
  four-axis verifier.
- `gaussclaw_desktop::updater::canonical_signed_message` — the
  string that gets signed.

A third-party Rust client only needs to depend on `ed25519-dalek`
and `sha2`; no GaussClaw runtime is required to verify a manifest.

## 7. Compatibility commitments

- The four-axis design is permanent. New axes can be added (a
  fifth axis would be opt-in); none of the existing four can be
  weakened without a new spec version.
- Field names + canonical message format are stable. New optional
  fields must default to "unverified" on legacy clients.
- The `chain_index` ledger is forward-only. A rollback requires
  operator action (re-anchor the ledger) and surfaces as an
  explicit audit-chain entry.

## 8. Threat model

Defends against:
- ✅ Network MitM swapping the artefact bytes (Axis 2).
- ✅ A compromised CDN serving a signed-but-old artefact (Axis 4).
- ✅ A cross-platform attack (e.g. shipping a Linux binary to a
   macOS client) (Axis 1).
- ✅ A forged manifest (Axis 3).

Does NOT defend against:
- ❌ A compromised publisher key — that's the operator's
  out-of-band trust assumption. Multi-key thresholds land in a
  future revision.
- ❌ A compromised local chain ledger — if an attacker can
  arbitrarily mutate the local "last installed" record, they can
  permit downgrade. The ledger lives inside the same trust
  boundary as the binary.

## 9. Bug bounty scope

The chain-anchored updater is in scope for the GaussClaw bug
bounty programme (see [`/docs/BUG_BOUNTY.md`](BUG_BOUNTY.md)).
Specific findings of interest:

- Bypass of any of the four axes that doesn't require key
  compromise.
- A failure mode where the receipt chain records "ok" when
  verification actually failed.
- Any spec gap that lets a publisher publish two artefacts with
  the same `(version, target)` but different `sha256` without a
  receipt chain entry.

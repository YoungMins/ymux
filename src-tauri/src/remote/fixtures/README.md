# Protocol fixtures

Test vectors for [PROTOCOL.md](../PROTOCOL.md), for the desktop and the PWA to check
themselves against. Every value is deterministic and made from the fixed test inputs in
`keys.json` (never use these keys for anything else):

- Signing keys: ECDSA P-256 private keys are 32 identical bytes (`0x01` desktop, `0x02` device,
  `0x03` push). Signatures use RFC 6979, so a given message always gives the same signature.
- Key agreement keys: ECDH P-256, private keys again 32 identical bytes: `0x04` for the device's
  `ek`, `0x05` for the desktop's `dk` (the one carried in the QR). Public keys are raw uncompressed
  SEC1 points (65 bytes) as b64u.
- Challenge nonce `0x00..0x1f`; pairing code 16 bytes of `0x42`; session id 16 bytes of `0x05`;
  previous (revoked) session id 16 bytes of `0x04`; `open` nonce 16 bytes of `0x31`; clock
  `now_ms` = 1731600000000; pairing check number `048213` (random in real use).
- Server origin `https://ymux.example.com` (what the challenge's `origin` says).
- AES-GCM nonces are not random here: sealed message *n* uses 12 bytes of `0x0n`
  (open `01`, session `02`, cmd `03`, reply `04`, panes `05`, reopen `06`, pair_check `07`,
  bad_sig reply `08`). A real sender uses a fresh random nonce every time.
- JSON plaintexts are what `serde_json` writes: keys sorted, no spaces. A sealed body is the
  bytes of the `plaintext` string given in the file, so you need not reproduce that
  serialisation, only check that you can open `body` and read the same JSON.

All `*.json` files are checked by `cargo test --test fixtures`, which recomputes each value
from the recorded inputs. Regenerate with
`YMUX_REGEN_FIXTURES=1 cargo test --test fixtures regenerate`.

## Files

Connection and pairing frames (plain JSON, as on the wire):

| File | What |
|---|---|
| `keys.json` | The test inputs above (including `ek`, `dk`, `pair_check`, `prev_session`), the signed strings, the pairing proof |
| `challenge.json` | Server challenge, with `origin` |
| `hello_desktop.json` | Desktop hello; `sig` signs `ymux-remote-v1\|desktop\|<desktop_id>\|<origin>\|<nonce>` (origin from `challenge.json`) |
| `hello_desktop_other_origin.json` | The same hello signed for `https://evil.example`; a server on `ymux.example.com` must answer `bad_sig` |
| `hello_device.json` | Device hello; signs `ymux-remote-v1\|device\|<device_id>\|<nonce>` |
| `ready_desktop.json`, `ready_device.json` | Replies to the hellos |
| `error.json` | An error frame |
| `device_online.json`, `device_offline.json`, `desktop_online.json`, `desktop_offline.json` | Presence hints |
| `pair_open.json`, `pair_close.json` | Desktop opens / closes the pairing window |
| `pair_request.json` | Device to server, with `ek`; `proof` = HMAC-SHA256(key = code, msg = `ymux-remote-v1\|pair\|<desktop_id>\|<device_id>\|<pubkey>\|<ek>`), `sig` signs `ymux-remote-v1\|pair\|<device_id>\|<nonce>` (it does not cover `ek`; the proof does) |
| `pair_request_forwarded.json` | What the desktop receives (device_id, pubkey, ek, name, proof) |
| `pair_challenge.json` | Desktop to server: `device_id` and a `body` that is `sealed_pair_check.json`'s |
| `pair_challenge_forwarded.json` | What the pairing socket of that device receives: `body` only |
| `pair_result_desktop.json`, `pair_result_device.json` | Desktop's decision, and what the device is told |
| `device_remove.json` | Desktop removes a device |
| `push_subscribe.json`, `push_unsubscribe.json`, `notify.json`, `push_payload.json` | Web Push frames |

Key agreement, key derivation and sealing (section 1 of the protocol):

| File | What |
|---|---|
| `link_key.json` | `K = HKDF-SHA256(ikm = x, salt = code, info)`, 32 bytes, `info = "ymux-remote-v1\|link\|<desktop_id>\|<device_id>\|<pubkey>\|<ek>\|<dk>"`, where `x` is the 32-byte x coordinate of `ECDH(ek private, dk public)`, equal to `ECDH(dk private, ek public)`. Records both private scalars and public points, `x` (`shared_secret_hex`), the salt, the exact `info`, and `K` as hex and b64u. Either side can verify from its own private key |
| `sealed_pair_check.json` | Desktop to device: `{"k":"pair_check","check":"048213"}` (AAD `d2p`), the body carried by `pair_challenge.json`. The number is random in real use and never shown on the desktop |
| `sealed_open.json` | Device to desktop: the `open` plaintext, the nonce, the AAD (`ymux-remote-v1\|p2d\|<desktop_id>\|<device_id>`), the resulting `body` string, and its `sig` (ECDSA over `ymux-remote-v1\|cmd\|<body>`) plus the full `cmd` frame |
| `sealed_session.json` | Desktop to device: the `session` reply to that `open` (AAD `d2p`), with `prev` = `{"session": <revoked session>, "seq": 3}` |
| `sealed_cmd.json` | Device to desktop: an `answer` command with `session`, `seq`, `ts_ms` and `menu_id`, body, `sig` and frame |
| `sealed_reply.json` | Desktop to device: the `reply` to it (carries `session`, `n` and `at_ms`) |
| `sealed_panes.json` | Desktop to device: an unsolicited `panes` list (`session`, `n`, `at_ms`) |
| `sealed_reopen.json` | Desktop to device: `reopen` (`session`, `n`, `at_ms`) |
| `sealed_bad_sig_reply.json` | Desktop to device: the `re:0` `bad_sig` failure for a command whose `sig` did not verify. Carries `session:""`, no `n` and no `at_ms` (a literal reading of section 5.1), and `ref` = b64u of the first 16 bytes of SHA-256 over the `sig` string as sent; the file records that string (`ref_sig`, here `sealed_cmd.json`'s sig) and the digest (`ref_sha256_hex`, `ref_first16_hex`) |
| `cmd_device.json`, `cmd_forwarded.json` | The `cmd` relay frame as the device sends it and as the desktop receives it (adds `device_id` and `conn`); `body` is the sealed command of `sealed_cmd.json` |
| `msg.json`, `msg_forwarded.json` | The `msg` relay frame as the desktop sends it (carries `sealed_reply.json`'s body) and as the device receives it (carries `sealed_panes.json`'s body) |

Each `sealed_*.json` holds `link_key_hex`, `aad`, `nonce_hex`, `plaintext` and `body`, where
`body = b64u(nonce || AES-256-GCM(K, nonce, aad, plaintext))` and the ciphertext includes the
16-byte tag. `direction` is `p2d` (device to desktop) or `d2p`. All of them use the `K` of
`link_key.json`.

## Checking your implementation

1. Derive the shared secret from `link_key.json` twice, from `ek` private + `dk` public and from
   `dk` private + `ek` public; both must equal `shared_secret_hex`. Then HKDF-SHA256 with the code
   as salt and the recorded `info`; the result must equal `key_hex`.
2. Compute the pair `proof` from `keys.json`'s inputs and compare with `pair_request.json`.
3. Open every `body` with `K` and its `aad`; you must get `plaintext` back. The same body
   must NOT open with the other direction's AAD.
4. Seal each `plaintext` with the recorded nonce; you must get `body` back exactly.
5. Verify every `sig` with the matching public key (WebCrypto P1363 form, 64 bytes).
6. Sign the desktop hello string yourself and verify your signature with the recorded public
   key. (The recorded `sig` values are RFC 6979 deterministic; WebCrypto's ECDSA is randomised,
   so compare by verifying, not by byte equality.)
7. For `sealed_bad_sig_reply.json`, hash `ref_sig` with SHA-256 and compare the first 16 bytes
   (b64u) with the plaintext's `ref`.

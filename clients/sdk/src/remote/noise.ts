// Noise_IK_25519_ChaChaPoly_BLAKE2s — the R2 device tunnel handshake, device
// (initiator) side, byte-compatible with the host's `tunnel::noise`.
//
// The device knows the host's static key in advance (pinned at pairing), so
// its first message already carries its own static key encrypted to the
// host; the host answers with its verdict. After the handshake each
// direction has its own cipher.

import { chacha20poly1305 } from "@noble/ciphers/chacha.js";
import { x25519 } from "@noble/curves/ed25519.js";
import { blake2s } from "@noble/hashes/blake2.js";
import { hmac } from "@noble/hashes/hmac.js";

export const NOISE_PROTOCOL = "Noise_IK_25519_ChaChaPoly_BLAKE2s";
const DH_LEN = 32;
export const TAG_LEN = 16;

export function concatBytes(...parts: Uint8Array[]): Uint8Array {
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let at = 0;
  for (const p of parts) {
    out.set(p, at);
    at += p.length;
  }
  return out;
}

/** One direction's cipher: ChaCha20-Poly1305 with a counter nonce. */
export class CipherState {
  private n = 0;

  constructor(private readonly key: Uint8Array | null = null) {}

  private nonce(): Uint8Array {
    // 32 zero bits, then the counter as a little-endian u64.
    const nonce = new Uint8Array(12);
    const view = new DataView(nonce.buffer);
    view.setUint32(4, this.n >>> 0, true);
    view.setUint32(8, Math.floor(this.n / 2 ** 32), true);
    return nonce;
  }

  encrypt(ad: Uint8Array, plaintext: Uint8Array): Uint8Array {
    if (!this.key) return plaintext;
    const out = chacha20poly1305(this.key, this.nonce(), ad).encrypt(plaintext);
    this.n++;
    return out;
  }

  /** Throws when the message fails authentication. */
  decrypt(ad: Uint8Array, ciphertext: Uint8Array): Uint8Array {
    if (!this.key) return ciphertext;
    const out = chacha20poly1305(this.key, this.nonce(), ad).decrypt(ciphertext);
    this.n++;
    return out;
  }
}

function hkdf(ck: Uint8Array, ikm: Uint8Array): [Uint8Array, Uint8Array] {
  const temp = hmac(blake2s, ck, ikm);
  const out1 = hmac(blake2s, temp, Uint8Array.of(1));
  const out2 = hmac(blake2s, temp, concatBytes(out1, Uint8Array.of(2)));
  return [out1, out2];
}

class SymmetricState {
  ck: Uint8Array;
  h: Uint8Array;
  cipher = new CipherState();

  constructor(prologue: Uint8Array) {
    // The protocol name is longer than the hash, so it is hashed.
    this.h = blake2s(new TextEncoder().encode(NOISE_PROTOCOL));
    this.ck = this.h;
    this.mixHash(prologue);
  }

  mixHash(data: Uint8Array): void {
    this.h = blake2s(concatBytes(this.h, data));
  }

  mixKey(ikm: Uint8Array): void {
    const [ck, key] = hkdf(this.ck, ikm);
    this.ck = ck;
    this.cipher = new CipherState(key);
  }

  encryptAndHash(plaintext: Uint8Array): Uint8Array {
    const ciphertext = this.cipher.encrypt(this.h, plaintext);
    this.mixHash(ciphertext);
    return ciphertext;
  }

  decryptAndHash(ciphertext: Uint8Array): Uint8Array {
    const plaintext = this.cipher.decrypt(this.h, ciphertext);
    this.mixHash(ciphertext);
    return plaintext;
  }

  split(): [CipherState, CipherState] {
    const [k1, k2] = hkdf(this.ck, new Uint8Array(0));
    return [new CipherState(k1), new CipherState(k2)];
  }
}

/** The ciphers of an established session, and the handshake hash. */
export interface NoiseSession {
  send: CipherState;
  recv: CipherState;
  handshakeHash: Uint8Array;
}

export interface InitiatorOptions {
  prologue: Uint8Array;
  /** Our X25519 static secret. */
  staticSecret: Uint8Array;
  /** The responder's X25519 static public key, known in advance. */
  remoteStatic: Uint8Array;
  /** Test vectors only: a fixed ephemeral secret. */
  ephemeralSecret?: Uint8Array;
}

/**
 * Start an IK handshake: the first message (`-> e, es, s, ss`) carrying
 * `payload`, and a function that reads the answer (`<- e, ee, se`).
 * Throws from `readReply` when the answer does not authenticate.
 */
export function initiate(
  options: InitiatorOptions,
  payload: Uint8Array = new Uint8Array(0),
): { message: Uint8Array; readReply(reply: Uint8Array): { payload: Uint8Array; session: NoiseSession } } {
  const ss = new SymmetricState(options.prologue);
  const s = options.staticSecret;
  const rs = options.remoteStatic;
  ss.mixHash(rs);

  const e = options.ephemeralSecret ?? x25519.utils.randomSecretKey();
  const ePub = x25519.getPublicKey(e);
  ss.mixHash(ePub);
  ss.mixKey(x25519.getSharedSecret(e, rs));
  const encryptedStatic = ss.encryptAndHash(x25519.getPublicKey(s));
  ss.mixKey(x25519.getSharedSecret(s, rs));
  const message = concatBytes(ePub, encryptedStatic, ss.encryptAndHash(payload));

  let read = false;
  return {
    message,
    readReply(reply: Uint8Array) {
      if (read) throw new Error("the reply was already read");
      read = true;
      if (reply.length < DH_LEN + TAG_LEN) throw new Error("short handshake message");
      const re = reply.subarray(0, DH_LEN);
      ss.mixHash(re);
      ss.mixKey(x25519.getSharedSecret(e, re));
      ss.mixKey(x25519.getSharedSecret(s, re));
      const replyPayload = ss.decryptAndHash(reply.subarray(DH_LEN));
      const [send, recv] = ss.split();
      return { payload: replyPayload, session: { send, recv, handshakeHash: ss.h } };
    },
  };
}

export interface ResponderOptions {
  prologue: Uint8Array;
  staticSecret: Uint8Array;
  ephemeralSecret?: Uint8Array;
}

/**
 * The host (responder) side, for tests and test hosts: read the first
 * message, learning the initiator's static key, then answer it.
 */
export function respond(
  options: ResponderOptions,
  message: Uint8Array,
): {
  remoteStatic: Uint8Array;
  payload: Uint8Array;
  reply(payload?: Uint8Array): { message: Uint8Array; session: NoiseSession };
} {
  const ss = new SymmetricState(options.prologue);
  const s = options.staticSecret;
  ss.mixHash(x25519.getPublicKey(s));
  if (message.length < DH_LEN + DH_LEN + 2 * TAG_LEN) throw new Error("short handshake message");
  const re = message.subarray(0, DH_LEN);
  ss.mixHash(re);
  ss.mixKey(x25519.getSharedSecret(s, re));
  const rs = ss.decryptAndHash(message.subarray(DH_LEN, 2 * DH_LEN + TAG_LEN));
  ss.mixKey(x25519.getSharedSecret(s, rs));
  const payload = ss.decryptAndHash(message.subarray(2 * DH_LEN + TAG_LEN));
  return {
    remoteStatic: rs,
    payload,
    reply(replyPayload = new Uint8Array(0)) {
      const e = options.ephemeralSecret ?? x25519.utils.randomSecretKey();
      const ePub = x25519.getPublicKey(e);
      ss.mixHash(ePub);
      ss.mixKey(x25519.getSharedSecret(e, re));
      ss.mixKey(x25519.getSharedSecret(e, rs));
      const out = concatBytes(ePub, ss.encryptAndHash(replyPayload));
      const [recv, send] = ss.split();
      return { message: out, session: { send, recv, handshakeHash: ss.h } };
    },
  };
}

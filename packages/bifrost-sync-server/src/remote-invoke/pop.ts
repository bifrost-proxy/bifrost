import crypto from 'crypto';

export interface PoPRequestEnvelope {
  ts: number;
  nonce: string;
  caller_pubkey: string;
  signature: string;
  [k: string]: unknown;
}

export interface VerifyPoPOptions {
  maxSkewMs?: number;
  expectedCallerPubkeyFp?: string;
}

export interface VerifyPoPResult {
  callerPubkey: string;
  callerPubkeyFp: string;
}

const DEFAULT_MAX_SKEW_MS = 30_000;

export function canonicalJson(value: unknown): string {
  return JSON.stringify(canonicalize(value));
}

export function ed25519FingerprintFromBase64(spkiB64: string): string {
  const der = decodeBase64Strict(spkiB64, 'caller_pubkey');
  try {
    const key = crypto.createPublicKey({ key: der, format: 'der', type: 'spki' });
    const details = key.asymmetricKeyDetails as { namedCurve?: string } | undefined;
    if (key.asymmetricKeyType !== 'ed25519' && details?.namedCurve !== 'ed25519') {
      throw new Error('invalid_caller_pubkey');
    }
    const exported = key.export({ format: 'der', type: 'spki' }) as Buffer;
    return crypto.createHash('sha256').update(exported).digest('hex');
  } catch {
    throw new Error('invalid_caller_pubkey');
  }
}

export async function verifyPoP(
  body: PoPRequestEnvelope,
  opts: VerifyPoPOptions,
  markNonce: (fp: string, nonce: string, seenAt: string) => boolean | Promise<boolean>,
): Promise<VerifyPoPResult> {
  if (!body || typeof body !== 'object') {
    throw new Error('signature_invalid');
  }
  if (!Number.isFinite(body.ts)) {
    throw new Error('timestamp_out_of_window');
  }
  const maxSkewMs = opts.maxSkewMs ?? DEFAULT_MAX_SKEW_MS;
  if (Math.abs(Date.now() - body.ts) > maxSkewMs) {
    throw new Error('timestamp_out_of_window');
  }
  if (typeof body.nonce !== 'string' || !/^[a-f0-9]{32}$/i.test(body.nonce)) {
    throw new Error('replay_detected');
  }

  const callerPubkeyFp = ed25519FingerprintFromBase64(body.caller_pubkey);
  if (opts.expectedCallerPubkeyFp && opts.expectedCallerPubkeyFp !== callerPubkeyFp) {
    throw new Error('caller_pubkey_mismatch');
  }
  if (!(await markNonce(callerPubkeyFp, body.nonce.toLowerCase(), new Date().toISOString()))) {
    throw new Error('replay_detected');
  }

  const signature = decodeBase64Strict(body.signature, 'signature');
  let key: crypto.KeyObject;
  try {
    key = crypto.createPublicKey({
      key: decodeBase64Strict(body.caller_pubkey, 'caller_pubkey'),
      format: 'der',
      type: 'spki',
    });
  } catch {
    throw new Error('invalid_caller_pubkey');
  }

  const payload = Buffer.from(canonicalJson(body), 'utf8');
  if (!crypto.verify(null, payload, key, signature)) {
    throw new Error('signature_invalid');
  }
  return { callerPubkey: body.caller_pubkey, callerPubkeyFp };
}

function canonicalize(value: unknown): unknown {
  if (Array.isArray(value)) {
    return value.map(canonicalize);
  }
  if (value && typeof value === 'object') {
    const out: Record<string, unknown> = {};
    for (const key of Object.keys(value as Record<string, unknown>).sort()) {
      if (key === 'signature') continue;
      out[key] = canonicalize((value as Record<string, unknown>)[key]);
    }
    return out;
  }
  if (typeof value === 'string') {
    return value.normalize('NFC');
  }
  return value;
}

function decodeBase64Strict(value: unknown, field: string): Buffer {
  if (typeof value !== 'string' || value.length === 0 || value.length % 4 !== 0) {
    throw new Error(field === 'caller_pubkey' ? 'invalid_caller_pubkey' : 'signature_invalid');
  }
  if (!/^[A-Za-z0-9+/]+={0,2}$/.test(value)) {
    throw new Error(field === 'caller_pubkey' ? 'invalid_caller_pubkey' : 'signature_invalid');
  }
  const decoded = Buffer.from(value, 'base64');
  if (decoded.length === 0 || decoded.toString('base64') !== value) {
    throw new Error(field === 'caller_pubkey' ? 'invalid_caller_pubkey' : 'signature_invalid');
  }
  return decoded;
}

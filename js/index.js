/**
 * Thin JS helpers on top of the wasm-bindgen exports.
 *
 * Browser / Node: call {@link init} once, then use {@link OhttpClient}.
 * `send` POSTs an encapsulated request with `fetch` and decapsulates.
 */

export {
  default as init,
  initSync,
  OhttpClient,
  EncapsulateBuilder,
  Encapsulated,
  OhttpResponse,
} from '../pkg/ohttp_client.js';

/**
 * POST `encapsulated` to the relay and decapsulate the response body.
 *
 * @param {import('../pkg/ohttp_client.js').Encapsulated} encapsulated
 * @param {typeof fetch} [fetchFn=globalThis.fetch]
 * @returns {Promise<import('../pkg/ohttp_client.js').OhttpResponse>}
 */
export async function send(encapsulated, fetchFn = globalThis.fetch) {
  const res = await fetchFn(encapsulated.url, {
    method: 'POST',
    headers: { 'content-type': encapsulated.content_type },
    body: encapsulated.body,
  });
  if (!res.ok) {
    const detail = await res.text().catch(() => '');
    throw new Error(`relay returned ${res.status}${detail ? `: ${detail}` : ''}`);
  }
  return encapsulated.decapsulate(new Uint8Array(await res.arrayBuffer()));
}

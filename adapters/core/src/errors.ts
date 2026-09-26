/**
 * The one error an adapter raises.
 *
 * `retryable` is what spec section 12 says an `adapter_error` carries. The
 * SDKs forward it across the runtime boundary (a thrown error with
 * `retryable = true` becomes a retryable `error` pause), and the JSONL
 * subprocess protocol of spec section 11.6 carries it as
 * `{"error": {"message", "retryable"}}`. A terminal hiccup is worth retrying;
 * a missing binary, a bad argument or a pane that no longer exists is not.
 */
export class AdapterError extends Error {
  readonly retryable: boolean

  constructor(message: string, retryable: boolean) {
    super(message)
    this.name = 'AdapterError'
    this.retryable = retryable
  }
}

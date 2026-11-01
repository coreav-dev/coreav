export const TRUSTED = Symbol("coreav.trusted");

/** Type-only module with one function type taking ref null exn (0x69), with no code or imports. */
export const EXNREF_PROBE = new Uint8Array([0, 97, 115, 109, 1, 0, 0, 0, 1, 5, 1, 96, 1, 105, 0]);

// Relaxed-SIMD feature probe for the browser packages.
//
// `pkg-relaxed` / `pkg-relaxed-threads` are built with `+relaxed-simd`; an
// engine that does not implement the proposal rejects such a module at
// compile time, so the pool must only pick them after `WebAssembly.validate`
// accepts a module that uses the same relaxed ops the decoder does. This file
// builds those modules byte-wise (no toolchain at runtime) and exposes the
// probe the pool consults.
//
// The opcode table is the relaxed-SIMD proposal (all 0xfd-prefixed,
// 0x100..0x113). Each probe module below is equivalent to:
//
//   (module
//     (func (export "f") (result v128)
//       v128.const i64x2 0 0   ;; x `operands` (zero is in range for every op)
//       <relaxed op>))
//
// so validation succeeds exactly when the engine knows the op.

const uleb = (n) => {
  const out = [];
  for (;;) {
    let b = n & 0x7f;
    n >>>= 7;
    if (n) b |= 0x80;
    out.push(b);
    if (!n) return out;
  }
};

const section = (id, content) => [id, ...uleb(content.length), ...content];

// One exported () -> v128 function: push `operands` zero v128 constants,
// apply the op, return it.
function probeModule(opcode, operands) {
  const body = [0x00]; // no locals
  for (let i = 0; i < operands; i++) body.push(0xfd, 0x0c, ...new Array(16).fill(0)); // v128.const
  body.push(0xfd, ...uleb(opcode), 0x0b);
  return new Uint8Array([
    0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00, // magic, version
    ...section(1, [0x01, 0x60, 0x00, 0x01, 0x7b]), // type: () -> v128
    ...section(3, [0x01, 0x00]), // function
    ...section(7, [0x01, 0x01, 0x66, 0x00, 0x00]), // export "f"
    ...section(10, [0x01, ...uleb(body.length), ...body]), // code
  ]);
}

const OPCODE_OPERANDS = [
  ['i8x16.relaxed_swizzle', 0x100, 2],
  ['i32x4.relaxed_trunc_f32x4_s', 0x101, 1],
  ['i32x4.relaxed_trunc_f32x4_u', 0x102, 1],
  ['i32x4.relaxed_trunc_f64x2_s_zero', 0x103, 1],
  ['i32x4.relaxed_trunc_f64x2_u_zero', 0x104, 1],
  ['f32x4.relaxed_madd', 0x105, 3],
  ['f32x4.relaxed_nmadd', 0x106, 3],
  ['f64x2.relaxed_madd', 0x107, 3],
  ['f64x2.relaxed_nmadd', 0x108, 3],
  ['i8x16.relaxed_laneselect', 0x109, 3],
  ['i16x8.relaxed_laneselect', 0x10a, 3],
  ['i32x4.relaxed_laneselect', 0x10b, 3],
  ['i64x2.relaxed_laneselect', 0x10c, 3],
  ['f32x4.relaxed_min', 0x10d, 2],
  ['f32x4.relaxed_max', 0x10e, 2],
  ['f64x2.relaxed_min', 0x10f, 2],
  ['f64x2.relaxed_max', 0x110, 2],
  ['i16x8.relaxed_q15mulr_s', 0x111, 2],
  ['i16x8.relaxed_dot_i8x16_i7x16_s', 0x112, 2],
  ['i32x4.relaxed_dot_i8x16_i7x16_add_s', 0x113, 3],
];

/** name -> probe module bytes, for every relaxed op in the proposal. */
export const RELAXED_PROBES = new Map(OPCODE_OPERANDS.map(([name, opcode, operands]) => [name, probeModule(opcode, operands)]));

/**
 * Validate every listed op (default: all of them) in the current engine.
 * @param {string[]} [ops] - subset to require; defaults to every relaxed op.
 * @returns {{ok: boolean, missing: string[]}}
 */
export function relaxedOpsSupport(ops = [...RELAXED_PROBES.keys()]) {
  const missing = [];
  for (const op of ops) {
    const probe = RELAXED_PROBES.get(op);
    if (!probe || !WebAssembly.validate(probe)) missing.push(op);
  }
  return { ok: missing.length === 0, missing };
}

// The ops the `relaxed` packages actually contain (kept in sync with the
// `#[cfg(target_feature = "relaxed-simd")]` arms in
// `imij/src/wasm_relaxed.rs`: lane select, clamped truncation, finite
// min/max). The pool only selects a relaxed package when every one of them
// validates, because a module fails to compile in an engine that lacks any
// single op.
export const RELAXED_REQUIRED_OPS = [
  'i16x8.relaxed_laneselect',
  'i32x4.relaxed_laneselect',
  'i32x4.relaxed_trunc_f32x4_s',
  'f32x4.relaxed_min',
  'f32x4.relaxed_max',
];

export function relaxedSimdSupported() {
  return relaxedOpsSupport(RELAXED_REQUIRED_OPS).ok;
}

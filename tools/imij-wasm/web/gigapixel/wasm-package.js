// The staged Explorer selects the native-only package after catalog validation.
// The source tree's general package also serves historical replay fixtures.
export const nativeOnly = false;

export function explorerWasm(variant, asset) {
  const root = nativeOnly ? '../dist/explorer/' : '../dist/';
  return new URL(`${root}pkg-${variant}/${asset}`, import.meta.url).href;
}

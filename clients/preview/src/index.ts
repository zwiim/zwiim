// The root entry is pure TypeScript with no renderer dependencies, safe to
// import from React Native. Renderers live under `@zwiim/preview/pdf`,
// `/sheet` and `/word`.
export * from "./kind";
export * from "./path";
export * from "./answer";
export * from "./errors";
export * from "./delimited";
export * from "./find";

/** A fresh preview origin id: 128 random bits as 32 lowercase hex chars. */
export function newPreviewId(): string {
  const bytes = new Uint8Array(16);
  crypto.getRandomValues(bytes);
  return Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
}

// @xterm/headless reads `navigator.userAgent` and `navigator.platform` as it
// loads, to tell browsers apart; React Native's `navigator` has neither. Give
// it empty ones before the emulator is imported (see ./screen).

const g = globalThis as unknown as { navigator?: Record<string, unknown> };
g.navigator ??= {};
g.navigator.userAgent ??= "";
g.navigator.platform ??= "";

export {};

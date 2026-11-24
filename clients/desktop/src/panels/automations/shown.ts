/**
 * EXPERIMENTAL: Automations (playbooks, runs and recording) are not ready to
 * ship. The engine gets most items right but not reliably enough for people
 * to use, so this release hides the sidebar row and the browser's Record
 * button. Turning this on exposes an experimental feature: test it on a dev
 * build only, and do not ship it on until the engine's results are good.
 */
export const AUTOMATIONS_SHOWN = false;

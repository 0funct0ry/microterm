// The version shown on the site: the workspace version in Cargo.toml, injected by
// astro.config.mjs at build time so a release redeploy needs no extra input (SPEC §15.4).
declare const __MT_VERSION__: string;

export const VERSION: string = __MT_VERSION__;

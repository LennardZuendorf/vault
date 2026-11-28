/// <reference types="vite/client" />

interface ImportMetaEnv {
  /** The commit a build made on this Mac came from (`release:mac`); unset in a release. */
  readonly VITE_LAYER_BUILD?: string;
}

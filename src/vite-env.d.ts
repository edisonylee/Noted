/// <reference types="vite/client" />

interface ImportMetaEnv {
  readonly VITE_NOTED_PROFILE?: "development" | "alpha" | "mobile";
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}

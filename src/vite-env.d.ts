/// <reference types="vite/client" />

interface ImportMetaEnv {
  readonly VITE_NOTED_PROFILE?: "development" | "alpha";
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}

/// <reference types="vite/client" />

interface ImportMetaEnv {
  readonly VITE_NOTED_PROFILE?: "development" | "alpha" | "mobile";
  readonly VITE_NOTED_IPHONE_COMPANION?: "1";
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}

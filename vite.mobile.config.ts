import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// A dedicated entry keeps desktop routes, model controls, recorder code, and
// assistant surfaces out of the iPhone bundle instead of merely hiding them.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  publicDir: false,
  root: "ios",
  server: {
    port: 1420,
    strictPort: true,
  },
  build: {
    outDir: "../dist-ios",
    emptyOutDir: true,
  },
});

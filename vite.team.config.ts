import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
export default defineConfig({
  plugins: [react()],
  server: {
    host: "127.0.0.1",
    port: 1422,
    strictPort: true,
    proxy: {
      "/__team": {
        target: "http://127.0.0.1:8790",
        rewrite: (path) => path.replace(/^\/__team/, ""),
      },
    },
    watch: { ignored: ["**/src-tauri/**"] },
  },
});

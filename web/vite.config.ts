import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import path from "node:path";

export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "src"),
    },
  },
  server: {
    host: "127.0.0.1",
    // Browser preview: `src/bridge/devTransport.ts` reads the local broker here.
    proxy: {
      "/broker": {
        target: "http://127.0.0.1:7331",
        rewrite: (path) => path.replace(/^\/broker/, ""),
      },
    },
  },
  build: {
    rollupOptions: {
      output: {
        manualChunks: {
          react: ["react", "react-dom", "react-dom/client"],
          refractor: ["refractor"],
        },
      },
    },
  },
});

import { defineConfig } from "vite";
import { resolve } from "path";

export default defineConfig({
  root: "src",
  build: {
    outDir: "../dist",
    emptyOutDir: true,
    rollupOptions: {
      input: {
        main: resolve(__dirname, "src/index.html"),
        preview: resolve(__dirname, "src/preview.html"),
      },
    },
  },
  server: { port: 1420, strictPort: true },
});

import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";
import { viteSingleFile } from "vite-plugin-singlefile";

// One self-contained HTML file: the desktop app embeds it and loads no other resource.
export default defineConfig({
  plugins: [react(), viteSingleFile()],
  build: {
    target: "safari16",
    outDir: "dist",
    assetsInlineLimit: 100_000_000,
    cssCodeSplit: false,
    reportCompressedSize: false,
  },
  test: { environment: "node" },
});

import { defineConfig } from "tsup";

export default defineConfig({
  entry: ["src/index.ts"],
  format: ["esm", "cjs"],
  dts: true,
  clean: true,
  sourcemap: true,
  target: "es2020",
  // Keep bundle minimal — no external deps to bundle anyway.
  external: [],
  // Ensure .js extensions are emitted in ESM output (required for NodeNext).
  esbuildOptions(options) {
    options.platform = "neutral";
  },
  // Separate type declarations for CJS consumers.
  cjsInterop: true,
  splitting: false,
  outDir: "dist",
});

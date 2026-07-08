import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

// Console 由 scootlensd 静态托管于 `/`（见 docs/07-web-console.md）。
// 相对 base 让产物可从任意挂载点加载。
export default defineConfig({
  base: "./",
  plugins: [svelte()],
  build: {
    outDir: "dist",
    target: "esnext",
  },
  server: {
    port: 5174,
  },
  test: {
    environment: "node",
    include: ["src/**/*.test.ts"],
    coverage: {
      provider: "v8",
      include: ["src/lib/**/*.ts"],
      exclude: ["src/**/*.test.ts"],
      reporter: ["text", "json-summary"],
      thresholds: {
        lines: 80,
        functions: 80,
        statements: 80,
        branches: 80,
      },
    },
  },
});

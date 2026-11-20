import { defineConfig } from "vitest/config";

/** The feature's pure logic, in Node. The Solid components are seen in the app. */
export default defineConfig({
  test: {
    environment: "node",
    include: ["src/**/*.{test,spec}.ts"],
    // The tests read the Spanish texts (the default language).
    setupFiles: ["./src/test-setup.ts"],
  },
});

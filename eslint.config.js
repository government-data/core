import js from "@eslint/js";
import tseslint from "typescript-eslint";
import prettier from "eslint-config-prettier";
import globals from "globals";

/**
 * Shared flat ESLint config for the whole workspace.
 *
 * ESLint 9 discovers this file from each package directory upward, so every
 * package inherits it — no per-package `.eslintrc` is needed. Formatting rules
 * are delegated to Prettier via `eslint-config-prettier` (kept last).
 */
export default tseslint.config(
  {
    ignores: [
      "**/dist/**",
      "**/node_modules/**",
      "**/coverage/**",
      // Watch mode's local state (`task dev:watch`), git-ignored.
      ".dev/**",
      // Los tipos que genera Astro al construir apps/sitio.
      "**/.astro/**",
      "**/*.config.js",
      "**/*.config.ts",
      // Generated from the backend OpenAPI spec (`pnpm gen:api`); not hand-edited.
      "packages/api-client/src/schema.ts",
    ],
  },
  js.configs.recommended,
  ...tseslint.configs.recommended,
  {
    languageOptions: {
      ecmaVersion: 2022,
      sourceType: "module",
      globals: {
        ...globals.browser,
        ...globals.node,
      },
    },
  },
  // Size limits (decision 0034): a file stops at 600 lines and a function at
  // 100, blank lines and comments aside. The targets are 300 and 60;
  // `cargo xtask sizes` checks the files' physical lines as well.
  {
    files: ["**/*.{js,jsx,ts,tsx}"],
    rules: {
      "max-lines": ["error", { max: 600, skipBlankLines: true, skipComments: true }],
      "max-lines-per-function": ["error", { max: 100, skipBlankLines: true, skipComments: true }],
      // An icon by its own path: lucide-solid's index pulls in every icon,
      // thousands of modules the dev server serves one by one.
      "no-restricted-imports": [
        "error",
        {
          paths: [
            {
              name: "lucide-solid",
              message: 'Import the icon by its path: `import X from "lucide-solid/icons/x"`.',
            },
          ],
        },
      ],
    },
  },
  // Tests have no file limit: they are split by use case.
  {
    files: ["**/*.test.{ts,tsx}", "**/*.spec.{ts,tsx}"],
    rules: {
      "max-lines": "off",
    },
  },
  prettier,
);

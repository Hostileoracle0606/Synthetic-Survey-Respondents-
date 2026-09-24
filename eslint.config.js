// @ts-check
// Uses @typescript-eslint/parser and /eslint-plugin directly rather than the `typescript-eslint`
// convenience package, which hard-refuses to load under TypeScript 7 (this project's compiler,
// via `tsc --noEmit`). The plugin's own TS-version check only warns, so this still works; revisit
// once typescript-eslint supports TS 7 (https://github.com/typescript-eslint/typescript-eslint/issues/10940).
// `pnpm install` runs scripts/link-eslint-typescript.mjs (see its header) to give these packages
// an isolated TypeScript 5.x for syntactic parsing; run it by hand after any manual `pnpm install`.
import js from "@eslint/js";
import tsParser from "@typescript-eslint/parser";
import tsPlugin from "@typescript-eslint/eslint-plugin";
import reactHooks from "eslint-plugin-react-hooks";
import reactRefresh from "eslint-plugin-react-refresh";
import globals from "globals";

export default [
  { ignores: ["dist", "src-tauri", "target", "src/types/gen", "node_modules"] },
  js.configs.recommended,
  {
    files: ["**/*.{ts,tsx}"],
    languageOptions: {
      ecmaVersion: 2022,
      globals: globals.browser,
      parser: tsParser,
      parserOptions: { ecmaFeatures: { jsx: true } },
    },
    plugins: {
      "@typescript-eslint": tsPlugin,
      "react-hooks": reactHooks,
      "react-refresh": reactRefresh,
    },
    rules: {
      ...tsPlugin.configs.recommended.rules,
      // The two long-standing hooks rules (correct hook usage; complete dependency arrays).
      // react-hooks v7's full "recommended" preset also bundles ~15 newer React Compiler
      // readiness checks (no ref writes during render, no setState in an effect body, etc.);
      // this app doesn't build against the Compiler, and several of those rules flag this
      // codebase's deliberate "ref holds latest callback" effect idiom, so they're left out.
      "react-hooks/rules-of-hooks": "error",
      "react-hooks/exhaustive-deps": "warn",
      "react-refresh/only-export-components": ["warn", { allowConstantExport: true }],
      "no-unused-vars": "off",
      "@typescript-eslint/no-unused-vars": ["error", { argsIgnorePattern: "^_" }],
    },
  },
  {
    // Plain Node build/tooling scripts, not part of the browser app bundle.
    files: ["*.config.{js,ts}", "scripts/**/*.{js,mjs}"],
    languageOptions: { ecmaVersion: 2022, sourceType: "module", globals: globals.node },
  },
];

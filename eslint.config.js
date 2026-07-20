import js from "@eslint/js";
import tseslint from "typescript-eslint";
import react from "eslint-plugin-react";
import reactHooks from "eslint-plugin-react-hooks";
import globals from "globals";

export default tseslint.config(
  {
    ignores: [
      "src-tauri/",
      "node_modules/",
      "dist/",
      "build/",
      "coverage/",
      "**/*.min.js",
    ],
  },
  {
    files: ["src/**/*.{ts,tsx}"],
    extends: [js.configs.recommended, ...tseslint.configs.recommended],
    plugins: {
      react,
      "react-hooks": reactHooks,
    },
    languageOptions: {
      ecmaVersion: "latest",
      sourceType: "module",
      globals: {
        ...globals.browser,
      },
      parserOptions: {
        ecmaFeatures: { jsx: true },
      },
    },
    rules: {
      ...reactHooks.configs.recommended.rules,
      // Track JSX variable usage for no-unused-vars
      "react/jsx-uses-react": "error",
      "react/jsx-uses-vars": "error",
      // Disable React Compiler rules — this project doesn't use the compiler
      "react-hooks/set-state-in-effect": "off",
      "react-hooks/preserve-manual-memoization": "off",
      // Use TS-aware no-unused-vars instead of base rule
      "no-unused-vars": "off",
      "@typescript-eslint/no-unused-vars": [
        "error",
        { argsIgnorePattern: "^_", varsIgnorePattern: "^_" },
      ],
    },
  },
);

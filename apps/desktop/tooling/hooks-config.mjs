import reactHooks from "eslint-plugin-react-hooks";
import tseslint from "typescript-eslint";

/** Only the two official Hooks rules; no stylistic / compiler extras. */
export function hooksPluginBlock() {
  return {
    linterOptions: { noInlineConfig: true },
    plugins: { "react-hooks": reactHooks },
    languageOptions: {
      parser: tseslint.parser,
      parserOptions: {
        ecmaVersion: 2022,
        sourceType: "module",
        ecmaFeatures: { jsx: true },
      },
    },
    rules: {
      "react-hooks/rules-of-hooks": "error",
      "react-hooks/exhaustive-deps": "error",
    },
  };
}

export const hooksFlatConfig = [
  {
    ignores: [
      "**/node_modules/**",
      "**/dist/**",
      "**/test-results/**",
      "src/bindings.ts",
    ],
  },
  {
    files: ["**/*.{ts,tsx}"],
    ...hooksPluginBlock(),
  },
];

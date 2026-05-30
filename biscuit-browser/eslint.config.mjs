import js from '@eslint/js'
import tseslint from 'typescript-eslint'

// Flat ESLint config. Intentionally pragmatic: tsc (strict) already does the
// heavy type checking, so ESLint here catches a focused set of real problems and
// leaves stylistic/duplicate checks to tsc + Prettier.
export default tseslint.config(
  {
    ignores: ['out/**', 'node_modules/**', 'dist/**', 'release/**', '.npm-cache/**', 'coverage/**']
  },
  js.configs.recommended,
  ...tseslint.configs.recommended,
  {
    rules: {
      // tsc handles undefined identifiers and gives correct DOM/Node lib globals.
      'no-undef': 'off',
      'no-control-regex': 'off',
      'no-empty': ['warn', { allowEmptyCatch: true }],
      // The `void promise` pattern is used deliberately to mark fire-and-forget.
      '@typescript-eslint/no-unused-expressions': 'off',
      '@typescript-eslint/no-explicit-any': 'off',
      // The page-world extractor scripts legitimately need @ts-nocheck.
      '@typescript-eslint/ban-ts-comment': 'off',
      '@typescript-eslint/no-unused-vars': ['warn', { argsIgnorePattern: '^_', varsIgnorePattern: '^_' }]
    }
  },
  {
    // Page-world scripts: serialized via .toString() and run in the browser, not
    // Node. They are dependency-free by design; relax unused-var noise here.
    files: ['src/main/agent-view/extract.ts', 'src/main/agent-view/signature.ts'],
    rules: {
      'no-unused-vars': 'off',
      '@typescript-eslint/no-unused-vars': 'off'
    }
  }
)

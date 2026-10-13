// A minimal configuration (ticket 28): the recommended rules of ESLint, typescript-eslint and the Svelte plugin,
// with type checking left to svelte-check. Formatting is Prettier's alone.
import js from '@eslint/js'
import svelte from 'eslint-plugin-svelte'
import globals from 'globals'
import ts from 'typescript-eslint'

export default ts.config(
  { ignores: ['dist/', '.svelte-check/'] },
  js.configs.recommended,
  ...ts.configs.recommended,
  ...svelte.configs.recommended,
  {
    languageOptions: { globals: { ...globals.browser } },
  },
  {
    files: ['**/*.svelte', '**/*.svelte.ts'],
    languageOptions: { parserOptions: { parser: ts.parser } },
  },
  {
    files: ['measure/**/*.mjs'],
    languageOptions: { globals: { ...globals.node } },
  },
)
